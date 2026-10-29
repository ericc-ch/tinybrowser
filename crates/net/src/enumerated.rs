//! One declaration form for an enumerated keyword set.
//!
//! The variant list is the only copy: it expands into the enum, `KEYWORDS`,
//! `keyword()`, `parse()`, and the `Enumerated` impl, so a variant cannot be
//! missing from the table or spelled two ways without the code saying so.

use std::fmt;

/// One enumerated attribute's keyword set.
pub trait Enumerated: Copy + 'static {
    /// Every keyword, in spec order.
    const KEYWORDS: &'static [Self];

    /// The canonical spelling of the keyword.
    fn keyword(self) -> &'static str;

    /// Parses one keyword, ASCII case-insensitively and untrimmed.
    fn parse(value: &str) -> Option<Self> {
        Self::KEYWORDS
            .iter()
            .copied()
            .find(|candidate| candidate.keyword().eq_ignore_ascii_case(value))
    }
}

/// A string that names no keyword in its set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnknownKeyword;

impl fmt::Display for UnknownKeyword {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("value is not a keyword")
    }
}

impl std::error::Error for UnknownKeyword {}

/// Declares one keyword set.
///
/// The invocation supplies the derives, because a set that crosses the wire
/// adds `Serialize`/`Deserialize` and one with a user-agent default adds
/// `Default`.
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
        $vis enum $name {
            $(
                $(#[$variant_doc])*
                $variant,
            )+
        }

        impl $name {
            /// The canonical spelling of the keyword.
            #[must_use]
            pub const fn keyword(self) -> &'static str {
                match self {
                    $(Self::$variant => $keyword),+
                }
            }

            /// Parses one keyword, ASCII case-insensitively and untrimmed.
            #[must_use]
            pub fn parse(value: &str) -> Option<Self> {
                $crate::Enumerated::parse(value)
            }
        }

        impl $crate::Enumerated for $name {
            const KEYWORDS: &'static [Self] = &[$(Self::$variant),+];

            fn keyword(self) -> &'static str {
                Self::keyword(self)
            }
        }

        impl From<$name> for &'static str {
            fn from(value: $name) -> Self {
                value.keyword()
            }
        }

        impl TryFrom<String> for $name {
            type Error = $crate::UnknownKeyword;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::parse(&value).ok_or($crate::UnknownKeyword)
            }
        }
    };
}
