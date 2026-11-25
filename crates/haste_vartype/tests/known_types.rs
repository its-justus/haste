//! Every type string seen in real replays must parse. All failures are reported at once.

#[test]
fn deadlock_types_parse() {
    let mut types: Vec<&str> = haste_vartype::known_deadlock_types().into_iter().collect();
    types.sort();
    assert!(types.len() > 200, "the list looks truncated: {}", types.len());
    let failures: Vec<String> = types
        .into_iter()
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
