//! Shared HTTP grammar helpers.
//!
//! One home for the lexical rules several modules apply at their
//! boundaries, so the byte sets never drift apart.

/// Is `c` a valid token character? RFC 9110 §5.1 `tchar` — the alphabet
/// shared by field names (§5.1) and methods (§9.1: `method = token`).
#[must_use]
pub(crate) fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            '!' | '#'
                | '$'
                | '%'
                | '&'
                | '\''
                | '*'
                | '+'
                | '-'
                | '.'
                | '^'
                | '_'
                | '`'
                | '|'
                | '~'
        )
}
