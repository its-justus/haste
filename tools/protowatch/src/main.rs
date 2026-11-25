//! protowatch: keeps haste_protos in step with SteamDatabase/Protobufs.
//!
//!   protowatch fetch <checkout>   copy ROOTS and their imports from the checkout, write PIN
//!   protowatch diff <checkout>    list differences of the watched messages (Markdown);
//!                                 exit 0: none, 1: differences, 2: error
//!
//! `<checkout>` is a local clone of SteamDatabase/Protobufs. Both commands take
//! `--crate-dir <dir>` (default: crates/haste_protos of this workspace).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use protowatch::{
    compile_dir,
    diff::diff,
    fetch::{fetch, head_sha},
    read_list,
};

fn default_crate_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/haste_protos")
}

fn run(args: &[String]) -> Result<ExitCode> {
    let mut crate_dir = default_crate_dir();
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--crate-dir" {
            crate_dir = it.next().context("--crate-dir needs a value")?.into();
        } else {
            rest.push(a.as_str());
        }
    }
    match rest.as_slice() {
        ["fetch", checkout] => {
            fetch(Path::new(checkout), &crate_dir)?;
            Ok(ExitCode::SUCCESS)
        }
        ["diff", checkout] => run_diff(Path::new(checkout), &crate_dir),
        _ => bail!("usage: protowatch [--crate-dir <dir>] fetch|diff <checkout>"),
    }
}

fn run_diff(checkout: &Path, crate_dir: &Path) -> Result<ExitCode> {
    let roots = read_list(&crate_dir.join("ROOTS"))?;
    let watched = read_list(&crate_dir.join("WATCHED"))?;
    let pin = std::fs::read_to_string(crate_dir.join("PIN")).context("read PIN")?;
    let old = compile_dir(&crate_dir.join("protos"), &roots)?;
    let new = compile_dir(&checkout.join("deadlock"), &roots)?;
    let report = diff(&old, &new, &watched)?;
    if report.is_empty() {
        println!("No differences in the watched messages.");
        return Ok(ExitCode::SUCCESS);
    }
    println!("# Protobuf differences\n");
    println!(
        "Pinned `{}`, checkout `{}`.\n",
        pin.trim(),
        head_sha(checkout)?
    );
    print!("{}", report.to_markdown());
    Ok(ExitCode::from(1))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("protowatch: {err:#}");
            ExitCode::from(2)
        }
    }
}
