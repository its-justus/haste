//! Keeps `haste_protos` in step with SteamDatabase/Protobufs.
//!
//! Two jobs: copy the files the crate needs from a local checkout (`fetch`), and list how
//! the messages the readers decode differ between the pinned files and a checkout (`diff`).
//! Nothing here touches the network.

pub mod diff;
pub mod fetch;

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result};
use prost_types::FileDescriptorSet;

/// Reads a list file (`ROOTS`, `WATCHED`): one entry per line, `#` starts a comment.
pub fn read_list(path: &Path) -> Result<Vec<String>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {path:?}"))?;
    Ok(parse_list(&text))
}

pub fn parse_list(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The import statements of one .proto text (`import "x";`, `import public "x";`).
pub fn imports_of(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in source.lines() {
        let line = line.split("//").next().unwrap_or("").trim();
        let Some(rest) = line.strip_prefix("import") else {
            continue;
        };
        let rest = rest.trim_start();
        let rest = rest.strip_prefix("public").unwrap_or(rest);
        let rest = rest.strip_prefix("weak").unwrap_or(rest).trim_start();
        if let Some(rest) = rest.strip_prefix('"') {
            if let Some(end) = rest.find('"') {
                out.push(rest[..end].to_owned());
            }
        }
    }
    out
}

/// The roots and everything they import, transitively, as paths relative to `dir`.
pub fn closure(dir: &Path, roots: &[String]) -> Result<BTreeSet<String>> {
    let mut seen = BTreeSet::new();
    let mut todo: Vec<String> = roots.to_vec();
    while let Some(name) = todo.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        let path = dir.join(&name);
        let source = std::fs::read_to_string(&path)
            .with_context(|| format!("{name}: not found in {dir:?}"))?;
        todo.extend(imports_of(&source));
    }
    Ok(seen)
}

/// Compiles `roots` (and their imports) found under `dir`.
pub fn compile_dir(dir: &Path, roots: &[String]) -> Result<FileDescriptorSet> {
    protox::compile(roots, [dir]).map_err(|e| anyhow::anyhow!("compile {dir:?}: {e}"))
}
