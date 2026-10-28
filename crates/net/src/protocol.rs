use std::borrow::Cow;
use std::fmt;

use hyper::header::{HeaderName, HeaderValue};

#[must_use]
/// Joins every occurrence of a list header's values with `", "`, the way a
/// list header is extracted
/// (<https://fetch.spec.whatwg.org/#concept-header-list-get-decode-split>).
pub fn combined_header<'a>(values: impl Iterator<Item = &'a str>) -> Option<String> {
    let values: Vec<&str> = values
        .map(|value| value.trim_matches([' ', '\t']))
        .filter(|value| !value.is_empty())
        .collect();
    if values.is_empty() {
        None
    } else {
        Some(values.join(", "))
    }
}

/// Joins every value of `name` in a decoded header list with `", "`, or
/// `None` when the header is absent
/// (<https://fetch.spec.whatwg.org/#concept-header-list-get>).
#[must_use]
pub fn combined_header_in<'a>(
    headers: impl IntoIterator<Item = (&'a str, &'a str)>,
    name: &str,
) -> Option<String> {
    combined_header(
        headers
            .into_iter()
            .filter(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value),
    )
}

/// Splits one header value into its members
/// (<https://fetch.spec.whatwg.org/#header-value-get-decode-and-split>).
///
/// Bytes are isomorphic-decoded, so `0x80..=0xFF` stay the same code points.
#[must_use]
pub fn split_header_value(value: &[u8]) -> Vec<String> {
    split_header_value_str(&isomorphic_decode(value))
}

/// Splits an already isomorphic-decoded header value into its members
/// (<https://fetch.spec.whatwg.org/#header-value-get-decode-and-split>).
///
/// A comma inside a quoted string does not split, the quoted text keeps its
/// quotes and escapes, and every member is stripped of leading and trailing
/// HTTP tab or space.
#[must_use]
pub fn split_header_value_str(value: &str) -> Vec<String> {
    let input: Vec<char> = value.chars().collect();
    let mut values = Vec::new();
    let mut member = String::new();
    let mut position = 0;
    loop {
        while position < input.len() && input[position] != '"' && input[position] != ',' {
            member.push(input[position]);
            position += 1;
        }
        if position < input.len() && input[position] == '"' {
            collect_quoted_string(&input, &mut position, &mut member);
            if position < input.len() {
                continue;
            }
        }
        values.push(member.trim_matches([' ', '\t']).to_owned());
        member.clear();
        if position >= input.len() {
            return values;
        }
        position += 1;
    }
}

/// Appends one HTTP quoted string, quotes and escapes included, and leaves
/// `position` just past it
/// (<https://fetch.spec.whatwg.org/#collect-an-http-quoted-string>).
fn collect_quoted_string(input: &[char], position: &mut usize, member: &mut String) {
    member.push(input[*position]);
    *position += 1;
    loop {
        while *position < input.len() && input[*position] != '"' && input[*position] != '\\' {
            member.push(input[*position]);
            *position += 1;
        }
        if *position >= input.len() {
            return;
        }
        let quote_or_backslash = input[*position];
        member.push(quote_or_backslash);
        *position += 1;
        if quote_or_backslash == '\\' {
            if *position >= input.len() {
                return;
            }
            member.push(input[*position]);
            *position += 1;
        } else {
            return;
        }
    }
}

/// The bytes a Web IDL `ByteString` names: every code point must be in
/// `0x00..=0xFF`, and each becomes that byte
/// (<https://webidl.spec.whatwg.org/#es-ByteString>,
/// <https://infra.spec.whatwg.org/#isomorphic-encode>). `None` when a code
/// point is outside the range, which the `ByteString` conversion rejects.
#[must_use]
pub fn isomorphic_encode(value: &str) -> Option<Vec<u8>> {
    value
        .chars()
        .map(|character| u8::try_from(character).ok())
        .collect()
}

/// Exposes each byte as the same code point, so a page never sees a
/// replacement character
/// (<https://infra.spec.whatwg.org/#isomorphic-decode>).
#[must_use]
pub fn isomorphic_decode(bytes: &[u8]) -> String {
    bytes.iter().map(|&byte| char::from(byte)).collect()
}

/// Gets `name` from `headers` and splits it into members
/// (<https://fetch.spec.whatwg.org/#header-value-get-decode-and-split>).
///
/// Repeated fields are combined with `", "` first, as getting a header does.
/// `None` means the header is absent; present headers with no value produce
/// one empty member, as the spec requires.
#[must_use]
pub fn get_decode_split(headers: &HeaderMap, name: &str) -> Option<Vec<String>> {
    let mut combined = Vec::new();
    let mut found = false;
    for value in headers.get_all(name) {
        if found {
            combined.extend_from_slice(b", ");
        }
        combined.extend_from_slice(value);
        found = true;
    }
    found.then(|| split_header_value(&combined))
}

/// Gets `name` from an already isomorphic-decoded header list and splits it
/// into members, combining repeated fields with `", "` first
/// (<https://fetch.spec.whatwg.org/#header-value-get-decode-and-split>).
///
/// `None` means the header is absent.
#[must_use]
pub fn get_decode_split_decoded<'a>(
    headers: impl IntoIterator<Item = (&'a str, &'a str)>,
    name: &str,
) -> Option<Vec<String>> {
    let values: Vec<&str> = headers
        .into_iter()
        .filter(|(header, _)| header.eq_ignore_ascii_case(name))
        .map(|(_, value)| value)
        .collect();
    if values.is_empty() {
        None
    } else {
        Some(split_header_value_str(&values.join(", ")))
    }
}

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

/// Ordered HTTP fields. Names are compared case-insensitively; values are
/// typed [`HeaderValue`]s.
///
/// `insert` appends. It does not replace earlier values of the same name.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct HeaderMap {
    entries: Vec<(HeaderName, HeaderValue)>,
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
    ///
    /// Validation uses `hyper`'s `HeaderName` / `HeaderValue` parsers, the same
    /// checks already applied when the map is written to the wire
    /// (<https://httpwg.org/specs/rfc9110.html#field.names>).
    pub fn insert(&mut self, name: &str, value: impl Into<Vec<u8>>) -> Result<(), HeaderError> {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| HeaderError::InvalidName(name.into()))?;
        let value = value.into();
        let value = HeaderValue::from_bytes(&value).map_err(|_| {
            HeaderError::InvalidValue(String::from_utf8_lossy(&value).into_owned().into())
        })?;
        self.entries.push((name, value));
        Ok(())
    }

    /// Appends an already-parsed field. Cannot fail because both halves came
    /// from `hyper`'s parsers.
    pub(crate) fn append(&mut self, name: HeaderName, value: HeaderValue) {
        self.entries.push((name, value));
    }

    /// Replaces every field named `name` with `value`, keeping the position of
    /// the first match. Appends when the name is absent.
    pub(crate) fn set(&mut self, name: HeaderName, value: HeaderValue) {
        let mut replacement = Some(value);
        self.entries.retain_mut(|(existing_name, existing_value)| {
            if existing_name != &name {
                return true;
            }
            match replacement.take() {
                Some(value) => {
                    *existing_name = name.clone();
                    *existing_value = value;
                    true
                }
                None => false,
            }
        });
        if let Some(value) = replacement {
            self.entries.push((name, value));
        }
    }

    /// Appends a field that came off the network. `hyper` parsed both halves
    /// while reading the response.
    pub(crate) fn insert_wire(&mut self, name: HeaderName, value: HeaderValue) {
        self.entries.push((name, value));
    }

    /// Removes every field whose name matches `name`, case-insensitively.
    pub fn remove(&mut self, name: &str) {
        self.entries
            .retain(|(field, _)| !field.as_str().eq_ignore_ascii_case(name));
    }

    /// First value whose name matches `name`, case-insensitively.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.get_all(name).next()
    }

    /// First typed value whose name matches `name`, case-insensitively.
    pub(crate) fn value(&self, name: &str) -> Option<&HeaderValue> {
        self.entries
            .iter()
            .find(|(field, _)| field.as_str().eq_ignore_ascii_case(name))
            .map(|(_, value)| value)
    }

    /// Every value whose name matches `name`, in insertion order.
    pub fn get_all<'s>(&'s self, name: &str) -> impl Iterator<Item = &'s [u8]> {
        self.entries
            .iter()
            .filter(move |(field, _)| field.as_str().eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_bytes())
    }

    /// All fields in insertion order, with typed names and values.
    pub(crate) fn typed_iter(&self) -> impl Iterator<Item = (&HeaderName, &HeaderValue)> {
        self.entries.iter().map(|(name, value)| (name, value))
    }

    /// All fields in insertion order. Names are serialized lowercase.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.entries
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_bytes()))
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

/// HTTP request method.
///
/// `GET`, `HEAD`, `POST`, `PUT`, `DELETE`, and `OPTIONS` are stored in
/// canonical uppercase. `PATCH` stays uppercase only when the input was already
/// `PATCH`. Any other valid token is kept as typed.
///
/// The wire spelling is the text [`Method::parse`] accepts, so a decoded
/// method is always a parsed method and neither side re-checks the grammar.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Method(Cow<'static, str>);

impl Method {
    pub const GET: Self = Self(Cow::Borrowed("GET"));
    pub const HEAD: Self = Self(Cow::Borrowed("HEAD"));
    pub const POST: Self = Self(Cow::Borrowed("POST"));
    pub const PUT: Self = Self(Cow::Borrowed("PUT"));
    pub const DELETE: Self = Self(Cow::Borrowed("DELETE"));
    pub const OPTIONS: Self = Self(Cow::Borrowed("OPTIONS"));
    pub const PATCH: Self = Self(Cow::Borrowed("PATCH"));

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
            "GET" => Cow::Borrowed("GET"),
            "HEAD" => Cow::Borrowed("HEAD"),
            "POST" => Cow::Borrowed("POST"),
            "PUT" => Cow::Borrowed("PUT"),
            "DELETE" => Cow::Borrowed("DELETE"),
            "OPTIONS" => Cow::Borrowed("OPTIONS"),
            "PATCH" if uppercased == token => Cow::Borrowed("PATCH"),
            _ => Cow::Owned(token.to_owned()),
        };
        Ok(Self(stored))
    }

    /// Whether this method is safe per
    /// [RFC 9110](https://www.rfc-editor.org/rfc/rfc9110#section-9.2.1):
    /// `GET`, `HEAD`, `OPTIONS`, or `TRACE`, matched case-sensitively.
    ///
    /// ```
    /// use net::Method;
    ///
    /// assert!(Method::GET.is_safe());
    /// assert!(Method::parse("TRACE").unwrap().is_safe());
    /// assert!(!Method::POST.is_safe());
    /// ```
    #[must_use]
    pub fn is_safe(&self) -> bool {
        matches!(self.0.as_ref(), "GET" | "HEAD" | "OPTIONS" | "TRACE")
    }

    /// Whether this method is idempotent per
    /// [RFC 9110](https://www.rfc-editor.org/rfc/rfc9110#section-9.2.2):
    /// `GET`, `HEAD`, `OPTIONS`, `TRACE`, `PUT`, and `DELETE`, matched
    /// case-sensitively like [`Method::is_safe`].
    ///
    /// ```
    /// use net::Method;
    ///
    /// assert!(Method::PUT.is_idempotent());
    /// assert!(!Method::POST.is_idempotent());
    /// ```
    #[must_use]
    pub fn is_idempotent(&self) -> bool {
        self.is_safe() || matches!(self.0.as_ref(), "PUT" | "DELETE")
    }

    /// Whether this method is forbidden to page scripts
    /// (<https://fetch.spec.whatwg.org/#forbidden-method>).
    ///
    /// Unlike the RFC predicates above, Fetch defines this one as a
    /// byte-case-insensitive match, because it rejects the spelling a page
    /// typed before normalization.
    ///
    /// ```
    /// use net::Method;
    ///
    /// assert!(Method::parse("TRACE").unwrap().is_forbidden());
    /// assert!(Method::parse("trace").unwrap().is_forbidden());
    /// assert!(!Method::POST.is_forbidden());
    /// ```
    #[must_use]
    pub fn is_forbidden(&self) -> bool {
        ["CONNECT", "TRACE", "TRACK"]
            .iter()
            .any(|forbidden| self.0.eq_ignore_ascii_case(forbidden))
    }

    /// Wire spelling of this method.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TryFrom<String> for Method {
    type Error = InvalidMethod;

    fn try_from(token: String) -> Result<Self, Self::Error> {
        Self::parse(&token)
    }
}

impl From<Method> for String {
    fn from(method: Method) -> Self {
        method.0.into_owned()
    }
}

#[cfg(test)]
mod tests {
    use hyper::header::{HeaderName, HeaderValue};

    use super::HeaderMap;

    #[test]
    fn insert_appends_and_set_replaces_in_place() {
        let mut map = HeaderMap::new();
        map.insert("x-one", "a").expect("valid field");
        map.insert("X-Two", "b").expect("valid field");
        map.insert("x-one", "c").expect("valid field");
        assert_eq!(
            map.iter().map(|(name, _)| name).collect::<Vec<_>>(),
            ["x-one", "x-two", "x-one"]
        );
        map.set(
            HeaderName::from_static("x-one"),
            HeaderValue::from_static("d"),
        );
        assert_eq!(
            map.iter().map(|(name, _)| name).collect::<Vec<_>>(),
            ["x-one", "x-two"]
        );
        assert_eq!(map.get_all("x-one").collect::<Vec<_>>(), [b"d"]);
    }

    #[test]
    fn insert_rejects_invalid_names_and_values() {
        let mut map = HeaderMap::new();
        assert!(map.insert("bad name", "v").is_err());
        assert!(map.insert("x", "bad\nvalue").is_err());
        assert!(map.is_empty());
    }

    #[test]
    fn splits_header_values_per_spec() {
        assert_eq!(super::split_header_value(b"nosniff,"), ["nosniff", ""]);
        assert_eq!(super::split_header_value(b""), [""]);
        assert_eq!(
            super::split_header_value(b"text/html;\", x/x"),
            ["text/html;\", x/x"]
        );
        assert_eq!(
            super::split_header_value(b"x/x;test=\"hi\",y/y"),
            ["x/x;test=\"hi\"", "y/y"]
        );
        assert_eq!(
            super::split_header_value(b"x / x,,,1"),
            ["x / x", "", "", "1"]
        );
        assert_eq!(super::split_header_value(b"\"1,2\", 3"), ["\"1,2\"", "3"]);
        // An escaped comma stays inside the member. Without the escape
        // branch the quoted string ends at the backslash and the comma
        // splits, so this pins the branch itself.
        assert_eq!(
            super::split_header_value(b"\"a\\,b\",c"),
            ["\"a\\,b\"", "c"]
        );
    }

    #[test]
    fn joins_repeated_fields_before_splitting() {
        let mut map = HeaderMap::new();
        map.insert("x-list", "a").expect("valid field");
        map.insert("x-list", "b").expect("valid field");
        assert_eq!(
            super::get_decode_split(&map, "x-list"),
            Some(vec!["a".to_owned(), "b".to_owned()])
        );
        assert_eq!(super::get_decode_split(&map, "x-missing"), None);
    }
}
