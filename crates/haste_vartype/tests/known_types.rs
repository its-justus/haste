//! Every type string seen in real replays must parse. All failures are reported at once.

const DEADLOCK_TYPES: &str = include_str!("../testdata/deadlock_types.txt");

#[test]
fn deadlock_types_parse() {
    let failures: Vec<String> = DEADLOCK_TYPES
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            haste_vartype::parse(line)
                .err()
                .map(|err| format!("  {line}\n    -> {err}"))
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{} type string(s) failed to parse:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
