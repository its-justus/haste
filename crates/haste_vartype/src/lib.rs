//! overengineered piece of crap.

mod error;
mod parser;
mod span;
mod tokenizer;

pub use error::Error;
pub use parser::{parse, Expr, Lit};
pub use span::Span;
pub use tokenizer::{Token, TokenKind, Tokenizer};

/// the field type strings of known Deadlock replays (`testdata/deadlock_types.txt`).
/// new type strings in a replay are those not in this list.
pub fn known_deadlock_types() -> std::collections::HashSet<&'static str> {
    include_str!("../testdata/deadlock_types.txt")
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}
