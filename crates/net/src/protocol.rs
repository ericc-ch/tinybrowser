use std::fmt;

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

/// Why a header name or value was rejected.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HeaderError {
    #[error("invalid header name: {0:?}")]
    InvalidName(Box<str>),
    #[error("invalid header value: {0:?}")]
    InvalidValue(Box<str>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    name: String,
    value: Vec<u8>,
}

/// Ordered HTTP fields. Names are compared case-insensitively; values are bytes.
///
/// `insert` appends. It does not replace earlier values of the same name.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct HeaderMap {
    entries: Vec<Entry>,
}

impl HeaderMap {
    /// Empty map.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends `name` / `value`. Existing fields with the same name are kept.
    ///
    /// # Errors
    ///
    /// [`HeaderError::InvalidName`] when `name` is empty or not an HTTP token.
    /// [`HeaderError::InvalidValue`] when `value` contains a CTL byte other than TAB.
    pub fn insert(&mut self, name: &str, value: impl Into<Vec<u8>>) -> Result<(), HeaderError> {
        if name.is_empty() || !name.chars().all(is_token_char) {
            return Err(HeaderError::InvalidName(name.into()));
        }
        let value = value.into();
        if value.iter().any(|&b| (b < 0x20 && b != b'\t') || b == 0x7F) {
            return Err(HeaderError::InvalidValue(
                String::from_utf8_lossy(&value).into_owned().into(),
            ));
        }
        self.entries.push(Entry {
            name: name.to_owned(),
            value,
        });
        Ok(())
    }

    /// Removes every field whose name matches `name`, case-insensitively.
    pub fn remove(&mut self, name: &str) {
        self.entries
            .retain(|entry| !entry.name.eq_ignore_ascii_case(name));
    }

    /// First value whose name matches `name`, case-insensitively.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.get_all(name).next()
    }

    /// Every value whose name matches `name`, in insertion order.
    pub fn get_all<'s>(&'s self, name: &str) -> impl Iterator<Item = &'s [u8]> {
        self.entries
            .iter()
            .filter(move |entry| entry.name.eq_ignore_ascii_case(name))
            .map(|entry| entry.value.as_slice())
    }

    /// All fields in insertion order. Names are the casing that was inserted.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.value.as_slice()))
    }

    /// Number of stored fields, counting duplicates.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no fields are stored.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// `token` is not an HTTP method token.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("invalid HTTP method: {0:?}")]
pub struct InvalidMethod(Box<str>);

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

/// HTTP request method.
///
/// `GET`, `HEAD`, `POST`, `PUT`, `DELETE`, and `OPTIONS` are stored in
/// canonical uppercase. `PATCH` stays uppercase only when the input was already
/// `PATCH`. Any other valid token is kept as typed.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Method(Token);

impl Method {
    pub const GET: Self = Self(Token::Get);
    pub const HEAD: Self = Self(Token::Head);
    pub const POST: Self = Self(Token::Post);
    pub const PUT: Self = Self(Token::Put);
    pub const DELETE: Self = Self(Token::Delete);
    pub const OPTIONS: Self = Self(Token::Options);
    pub const PATCH: Self = Self(Token::Patch);

    /// Parses an HTTP method token per [Fetch methods](https://fetch.spec.whatwg.org/#methods).
    ///
    /// # Errors
    ///
    /// [`InvalidMethod`] when `token` is empty or not an HTTP token.
    pub fn parse(token: &str) -> Result<Self, InvalidMethod> {
        if token.is_empty() || !token.chars().all(is_token_char) {
            return Err(InvalidMethod(token.into()));
        }
        let uppercased = token.to_ascii_uppercase();
        let stored = match uppercased.as_str() {
            "GET" => Token::Get,
            "HEAD" => Token::Head,
            "POST" => Token::Post,
            "PUT" => Token::Put,
            "DELETE" => Token::Delete,
            "OPTIONS" => Token::Options,
            "PATCH" if uppercased == token => Token::Patch,
            _ => Token::Extension(token.into()),
        };
        Ok(Self(stored))
    }

    /// Whether this is `GET`, `HEAD`, or `OPTIONS`.
    #[must_use]
    pub fn is_safe(&self) -> bool {
        matches!(self.0, Token::Get | Token::Head | Token::Options)
    }

    /// Wire spelling of this method.
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
