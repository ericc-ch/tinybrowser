//! Qualified-name comparisons for attributes and elements.
//!
//! Moved with the Blitz cutover: pure name logic with no tree dependency.

use markup5ever::QualName;

/// Whether `name`'s serialization (`prefix:local` or `local`) equals `query`.
#[must_use]
pub(crate) fn qualified_name_eq(name: &QualName, query: &str) -> bool {
    match name.prefix.as_ref() {
        Some(prefix) if !prefix.is_empty() => {
            let prefix = prefix.as_ref();
            query.len() == prefix.len() + 1 + name.local.len()
                && query.as_bytes().get(prefix.len()) == Some(&b':')
                && prefix == &query[..prefix.len()]
                && name.local.as_ref() == &query[prefix.len() + 1..]
        }
        _ => name.local.as_ref() == query,
    }
}
