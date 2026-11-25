//! the "types" report: every distinct field type string in a replay, marking the ones not
//! in the known list (haste_vartype/testdata/deadlock_types.txt) and the ones haste cannot
//! decode. works on replays haste cannot decode.
//!
//!   types <replay.dem> [--problems]
//!
//! prints tab-separated lines to stdout (problems first) and a summary to stderr.
//! --problems prints only new or unsupported types. exits with 1 if any type is unsupported.

use std::fs::File;
use std::io::BufReader;
use std::process::ExitCode;

use anyhow::{Context as _, Result};
use haste::demofile::DemoFile;
use haste::typereport::{known_deadlock_types, read_type_report};

fn main() -> Result<ExitCode> {
    let args: Vec<String> = std::env::args().collect();
    let usage = "usage: types <replay.dem> [--problems]";
    let path = args.get(1).context(usage)?;
    let only_problems = match args.get(2).map(String::as_str) {
        None => false,
        Some("--problems") => true,
        Some(other) => anyhow::bail!("unknown option {other}\n{usage}"),
    };

    let file = File::open(path).with_context(|| format!("could not open {path}"))?;
    let mut demo = DemoFile::start_reading(BufReader::new(file))?;
    let report = read_type_report(&mut demo, &known_deadlock_types())?;

    let mut types: Vec<_> = report.types.iter().collect();
    // unsupported first, then new, then the rest; alphabetical within each.
    types.sort_by_key(|t| (t.supported.is_ok(), t.known));

    println!("new\tsupported\tuses\ttype\tfirst_use\treason");
    for t in types {
        if only_problems && t.known && t.supported.is_ok() {
            continue;
        }
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}",
            if t.known { "no" } else { "yes" },
            if t.supported.is_ok() { "yes" } else { "no" },
            t.uses,
            t.var_type,
            t.first_use,
            t.supported.as_ref().err().map_or("", String::as_str),
        );
    }

    let unsupported = report.unsupported_types().count();
    let build = report
        .build
        .map_or_else(|| "unknown".to_string(), |b| b.to_string());
    eprintln!(
        "build {build}: {} types, {} new, {unsupported} unsupported",
        report.types.len(),
        report.new_types().count(),
    );
    Ok(if unsupported > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}
