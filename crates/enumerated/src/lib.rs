//! One declaration form for an enumerated keyword set.
//!
//! The variant list is the only copy: it expands into the enum, `KEYWORDS`,
//! `keyword()`, `parse()`, and the `Enumerated` impl. A duplicate spelling is a
//! compile error, so a keyword cannot appear twice or be missing from the
//! table without the code saying so.

/// One enumerated keyword set.
pub trait Enumerated: Copy + 'static {
    /// Every keyword, in spec order.
    const KEYWORDS: &'static [Self];

    /// The canonical spelling of the keyword.
    fn keyword(self) -> &'static str;

    /// Parses one keyword, ASCII case-insensitively and untrimmed. Enumerated
    /// attributes and header list members trim in their own parsers, so a
    /// stray space is not a keyword here.
    fn parse(value: &str) -> Option<Self> {
        Self::KEYWORDS
            .iter()
            .copied()
            .find(|candidate| candidate.keyword().eq_ignore_ascii_case(value))
    }
}

/// Whether two keywords are the same spelling, ASCII case-insensitively.
///
/// The macro runs this at compile time, where `str::eq_ignore_ascii_case` is
/// not available.
#[doc(hidden)]
#[must_use]
pub const fn keywords_equal(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    let mut index = 0;
    while index < left.len() {
        if !left[index].eq_ignore_ascii_case(&right[index]) {
            return false;
        }
        index += 1;
    }
    true
}

/// Declares one keyword set.
///
/// The invocation supplies only the extra derives; `Clone`, `Copy`, `Debug`,
/// `Eq`, and `PartialEq` are part of the declaration.
#[macro_export]
macro_rules! enumerated {
    (
        $(#[$doc:meta])*
        $vis:vis $name:ident {
            $(
                $(#[$variant_doc:meta])*
                $variant:ident => $keyword:literal
            ),+ $(,)?
        }
    ) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        $vis enum $name {
            $(
                $(#[$variant_doc])*
                $variant,
            )+
        }

        impl $crate::Enumerated for $name {
            const KEYWORDS: &'static [Self] = &[$(Self::$variant),+];

            fn keyword(self) -> &'static str {
                match self {
                    $(Self::$variant => $keyword),+
                }
            }
        }

        const _: () = {
            let keywords = [$($keyword),+];
            let mut left = 0;
            while left < keywords.len() {
                let mut right = left + 1;
                while right < keywords.len() {
                    assert!(
                        !$crate::keywords_equal(keywords[left], keywords[right]),
                        "an enumerated set spells one keyword twice"
                    );
                    right += 1;
                }
                left += 1;
            }
        };
    };
}

#[cfg(test)]
mod tests {
    use super::{Enumerated, keywords_equal};

    enumerated! {
        #[derive(Default)]
        TestSet {
            #[default]
            First => "first",
            Second => "second",
        }
    }

    #[test]
    fn parse_is_ascii_case_insensitive_and_untrimmed() {
        assert_eq!(TestSet::parse("FIRST"), Some(TestSet::First));
        assert_eq!(TestSet::parse(" first"), None);
        assert_eq!(TestSet::parse("first "), None);
        assert_eq!(TestSet::parse(""), None);
    }

    #[test]
    fn every_keyword_parses_back_to_its_variant() {
        for value in TestSet::KEYWORDS {
            assert_eq!(TestSet::parse(value.keyword()), Some(*value));
        }
    }

    #[test]
    fn a_keyword_comparison_ignores_ascii_case_only() {
        assert!(keywords_equal("first", "FIRST"));
        assert!(!keywords_equal("first", "first "));
        assert!(!keywords_equal("first", "firs"));
    }
}
