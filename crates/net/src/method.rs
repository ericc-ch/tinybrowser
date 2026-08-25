//! [`Method`]: the HTTP method token, parsed at the boundary.

use std::fmt;

use crate::token::is_token_char;

/// Why a method token was rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidMethod(Box<str>);

impl fmt::Display for InvalidMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid HTTP method: {:?}", self.0)
    }
}

impl std::error::Error for InvalidMethod {}

/// The closed set of methods net gives dedicated storage; anything else
/// legal page JavaScript sends stays an extension token.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Token {
    Get,
    Head,
    Post,
    Put,
    Delete,
    Options,
    Patch,
    Extension(Box<str>),
}

/// An HTTP method token (RFC 9110 §9.1).
///
/// Known methods are variants; arbitrary extension methods parse into
/// [`Token::Extension`] storage, kept **verbatim**. Normalization follows
/// WHATWG fetch §2.2.1 Methods (<https://fetch.spec.whatwg.org/#methods>):
/// only a byte-case-insensitive match for `DELETE`, `GET`, `HEAD`,
/// `OPTIONS`, `POST`, or `PUT` is byte-uppercased — that list is
/// exhaustive. `patch` stays `patch` on the wire ("using `patch` is highly
/// likely to result in a `405`" — the spec's own warning), and extension
/// tokens keep the caller's case ("`Egg` or `eGg` would be fine"). This is
/// wire behavior page JavaScript will produce through us, so it must match
/// Chrome byte for byte. The v1 backend is told to allow non-standard
/// methods so those tokens actually reach the request line.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Method(Token);

impl Method {
    /// `GET`
    pub const GET: Self = Self(Token::Get);
    /// `HEAD`
    pub const HEAD: Self = Self(Token::Head);
    /// `POST`
    pub const POST: Self = Self(Token::Post);
    /// `PUT`
    pub const PUT: Self = Self(Token::Put);
    /// `DELETE`
    pub const DELETE: Self = Self(Token::Delete);
    /// `OPTIONS`
    pub const OPTIONS: Self = Self(Token::Options);
    /// `PATCH` — exact spelling only. fetch's normalize list excludes
    /// `patch` on purpose (fetch #methods), so lowercase input stays
    /// lowercase all the way to the wire.
    pub const PATCH: Self = Self(Token::Patch);

    /// Parse a method token.
    ///
    /// Normalizes per WHATWG fetch §2.2.1 Methods
    /// (<https://fetch.spec.whatwg.org/#methods>): only the six listed
    /// methods become byte-uppercase, in any input casing; everything else
    /// is stored and sent verbatim. Validates against the RFC 9110 §9.1
    /// token grammar (`method = token`).
    ///
    /// # Errors
    ///
    /// [`InvalidMethod`] when the token is empty or contains characters
    /// outside the token set.
    pub fn parse(token: &str) -> Result<Self, InvalidMethod> {
        if token.is_empty() || !token.chars().all(is_token_char) {
            return Err(InvalidMethod(token.into()));
        }
        let uppercased = token.to_ascii_uppercase();
        let stored = match uppercased.as_str() {
            // The exhaustive normalize list (fetch #methods): any casing
            // of these six becomes the byte-uppercase form.
            "GET" => Token::Get,
            "HEAD" => Token::Head,
            "POST" => Token::Post,
            "PUT" => Token::Put,
            "DELETE" => Token::Delete,
            "OPTIONS" => Token::Options,
            // PATCH is deliberately outside that list — only its exact
            // uppercase spelling reaches dedicated storage; `patch`,
            // `Patch`, `pAtCh` ride verbatim.
            "PATCH" if uppercased == token => Token::Patch,
            // Extension tokens keep the caller's case ("`Egg` or `eGg`
            // would be fine" — fetch #methods).
            _ => Token::Extension(token.into()),
        };
        Ok(Self(stored))
    }

    /// The wire token: canonical uppercase for known methods, exactly what
    /// was parsed for extensions.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match &self.0 {
            Token::Get => "GET",
            Token::Head => "HEAD",
            Token::Post => "POST",
            Token::Put => "PUT",
            Token::Delete => "DELETE",
            Token::Options => "OPTIONS",
            Token::Patch => "PATCH",
            Token::Extension(ext) => ext,
        }
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
