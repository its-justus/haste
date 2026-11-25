//! `protowatch fetch`: copy `ROOTS` and their import closure from a checkout.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::{closure, read_list};

/// The full sha of the checkout's HEAD.
pub fn head_sha(checkout: &Path) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(checkout)
        .args(["rev-parse", "HEAD"])
        .output()
        .context("run git")?;
    if !out.status.success() {
        bail!("git rev-parse HEAD failed in {checkout:?}");
    }
    Ok(String::from_utf8(out.stdout)?.trim().to_owned())
}

/// Copies the closure of `crate_dir/ROOTS` from `checkout/deadlock` into
/// `crate_dir/protos`, deletes files no longer in it, and writes `crate_dir/PIN`.
pub fn fetch(checkout: &Path, crate_dir: &Path) -> Result<()> {
    let sha = head_sha(checkout)?;
    let src = checkout.join("deadlock");
    let dst = crate_dir.join("protos");
    let roots = read_list(&crate_dir.join("ROOTS"))?;
    let files = closure(&src, &roots)?;

    for name in &files {
        let to = dst.join(name);
        std::fs::create_dir_all(to.parent().context("no parent")?)?;
        std::fs::copy(src.join(name), &to).with_context(|| format!("copy {name}"))?;
    }
    remove_unwanted(&dst, &dst, &files)?;
    std::fs::write(crate_dir.join("PIN"), format!("{sha}\n"))?;
    println!("pinned {sha}: {} files in {dst:?}", files.len());
    Ok(())
}

fn remove_unwanted(
    root: &Path,
    dir: &Path,
    keep: &std::collections::BTreeSet<String>,
) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            remove_unwanted(root, &path, keep)?;
            if std::fs::read_dir(&path)?.next().is_none() {
                std::fs::remove_dir(&path)?;
            }
        } else {
            let rel = path
                .strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/");
            if !keep.contains(&rel) {
                std::fs::remove_file(&path)?;
                println!("removed {rel}");
            }
        }
    }
    Ok(())
}
