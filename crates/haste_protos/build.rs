use std::fs;
use std::io;
use std::path::Path;

/// Reads `ROOTS`: one file name per line, `#` starts a comment.
fn read_roots(path: &Path) -> io::Result<Vec<String>> {
    Ok(fs::read_to_string(path)?
        .lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect())
}

fn main() -> io::Result<()> {
    println!("cargo::rerun-if-changed=protos");
    println!("cargo::rerun-if-changed=ROOTS");

    let roots = read_roots(Path::new("ROOTS"))?;
    let fds = protox::compile(&roots, ["protos"])
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;

    // The files have no package: prost names the output after the (empty) package.
    let mut config = prost_build::Config::new();
    config.default_package_filename("haste_protos");
    config.compile_fds(fds)
}
