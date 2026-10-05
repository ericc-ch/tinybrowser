//! Qualified-name comparisons for attributes and elements.
//!
//! Moved with the Blitz cutover: pure name logic with no tree dependency.

use markup5ever::QualName;

/// Whether `name`'s serialization (`prefix:local` or `local`) equals `query`.
#[must_use]
pub(crate) fn qualified_name_eq(name: &QualName, query: &str) -> bool {
    qualified_name_eq_by(name, query, str::eq)
}

/// The shared qualified-name comparison. Length and `:` position are checked
/// before slicing, so a multi-byte `query` can never hit a non-boundary slice.
fn qualified_name_eq_by(name: &QualName, query: &str, eq: impl Fn(&str, &str) -> bool) -> bool {
    match name.prefix.as_ref() {
        Some(prefix) if !prefix.is_empty() => {
            let prefix = prefix.as_ref();
            query.len() == prefix.len() + 1 + name.local.len()
                && query.as_bytes().get(prefix.len()) == Some(&b':')
                && eq(prefix, &query[..prefix.len()])
                && eq(name.local.as_ref(), &query[prefix.len() + 1..])
        }
        _ => eq(name.local.as_ref(), query),
    }
}
