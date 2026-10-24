use std::borrow::Cow;
use std::fmt;

use hyper::header::{HeaderName, HeaderValue};
use url::Url;

/// One invocation of "Determine request's referrer": `policy` is the
/// request's referrer policy, `source` the referrer source — the document's
/// URL on the first determination, and the referrer the previous hop sent on
/// every later one, because HTTP-redirect fetch re-invokes main fetch and the
/// algorithm's "a URL" branch makes the previous referrer the new source —
/// and `target` the request's current URL. Returns the `Referer` value, or
/// `None` for no referrer
/// (<https://w3c.github.io/webappsec-referrer-policy/#determine-requests-referrer>).
#[must_use]
pub fn referrer_for_policy(policy: &str, source: &Url, target: &Url) -> Option<String> {
    let policy = if policy.is_empty() {
        "strict-origin-when-cross-origin"
    } else {
        policy
    };
    if policy == "no-referrer" {
        return None;
    }
    if !matches!(source.scheme(), "http" | "https") {
        return None;
    }
    // The source with credentials and fragment stripped, for the full-URL
    // policies ("Strip url for use as a referrer").
    let mut stripped = source.clone();
    let _ = stripped.set_username("");
    let _ = stripped.set_password(None);
    stripped.set_fragment(None);
    // The source's origin, serialized as a URL: always with the `/` path,
    // as browsers send it (`Referer: https://example.com/`).
    let origin = format!("{}/", stripped.origin().ascii_serialization());
    // A serialized referrer URL longer than 4096 characters culls to the
    // origin, whatever the policy — even `unsafe-url`.
    let referrer_url = if stripped.as_str().len() > 4096 {
        origin.clone()
    } else {
        stripped.to_string()
    };
    let downgrade = source.scheme() == "https" && !matches!(target.scheme(), "https" | "wss");
    let same_origin = source.origin() == target.origin();
    match policy {
        "origin" => Some(origin),
        "unsafe-url" => Some(referrer_url),
        "strict-origin" => (!downgrade).then_some(origin),
        "same-origin" => same_origin.then_some(referrer_url),
        "origin-when-cross-origin" => Some(if same_origin { referrer_url } else { origin }),
        "no-referrer-when-downgrade" => (!downgrade).then_some(referrer_url),
        // `strict-origin-when-cross-origin` and anything unrecognized: the
        // strict default. Unknown tokens fail closed to the default rather
        // than leaking the full URL.
        _ if downgrade => None,
        _ if same_origin => Some(referrer_url),
        _ => Some(origin),
    }
}

#[must_use]
/// Joins every occurrence of a list header's values with `", "`, the way a
/// list header is extracted
/// (<https://fetch.spec.whatwg.org/#concept-header-list-get-decode-split>).
pub fn combined_header<'a>(values: impl Iterator<Item = &'a str>) -> Option<String> {
    let values: Vec<&str> = values.map(str::trim).filter(|v| !v.is_empty()).collect();
    if values.is_empty() {
        None
    } else {
        Some(values.join(", "))
    }
}

/// Reads a `Referrer-Policy` field as the last recognized policy token
/// (<https://w3c.github.io/webappsec-referrer-policy/#parse-referrer-policy-from-header>).
/// Unknown tokens and empty list members are ignored, so a trailing unknown
/// token never resets a policy the document already had.
#[must_use]
pub fn referrer_policy_from_header(value: &str) -> Option<String> {
    value
        .split(',')
        .map(str::trim)
        .rev()
        .find(|token| {
            !token.is_empty()
                && matches!(
                    token.to_ascii_lowercase().as_str(),
                    "no-referrer"
                        | "no-referrer-when-downgrade"
                        | "same-origin"
                        | "origin"
                        | "strict-origin"
                        | "origin-when-cross-origin"
                        | "strict-origin-when-cross-origin"
                        | "unsafe-url"
                        | "never"
                )
        })
        .map(str::to_owned)
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
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
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

#[cfg(test)]
mod tests {
    use hyper::header::{HeaderName, HeaderValue};

    use super::{HeaderMap, referrer_for_policy, referrer_policy_from_header};
    use url::Url;

    /// The `Referer` each hop of a redirect chain sends: the source of hop N
    /// is the referrer hop N-1 sent, so the chain demotes to the origin the
    /// moment it leaves the source's origin and never re-widens
    /// (<https://fetch.spec.whatwg.org/#main-fetch>,
    /// <https://w3c.github.io/webappsec-referrer-policy/#determine-requests-referrer>).
    #[test]
    fn redirect_chain_redetermines_from_the_previous_referrer() {
        let policy = "strict-origin-when-cross-origin";
        let doc = Url::parse("http://web-platform.test:8000/page.html").expect("doc");
        // same-origin start, swap-origin redirect to a cross origin: the
        // first hop sends the full URL, the cross hop demotes it to the
        // origin, and the referrer never re-widens.
        let hop1 = referrer_for_policy(policy, &doc, &Url::parse("http://web-platform.test:8000/x").expect("u"))
            .expect("referrer");
        assert_eq!(hop1, "http://web-platform.test:8000/page.html");
        let hop2 = referrer_for_policy(
            policy,
            &Url::parse(&hop1).expect("hop1"),
            &Url::parse("http://www1.web-platform.test:8000/x").expect("u"),
        )
        .expect("referrer");
        assert_eq!(hop2, "http://web-platform.test:8000/");
        // cross-origin start, swap-origin redirect back to the source's
        // origin: the origin referrer survives the same-origin hop unchanged.
        let hop1 = referrer_for_policy(
            policy,
            &doc,
            &Url::parse("http://www1.web-platform.test:8000/x").expect("u"),
        )
        .expect("referrer");
        assert_eq!(hop1, "http://web-platform.test:8000/");
        let hop2 = referrer_for_policy(
            policy,
            &Url::parse(&hop1).expect("hop1"),
            &Url::parse("http://web-platform.test:8000/x").expect("u"),
        )
        .expect("referrer");
        assert_eq!(hop2, "http://web-platform.test:8000/");
        // `no-referrer` never starts, and a determined `no referrer` stops
        // the chain because Fetch skips the determination on it.
        assert_eq!(
            referrer_for_policy("no-referrer", &doc, &Url::parse("http://web-platform.test/x").expect("u")),
            None
        );
        // The 4096-character cull applies before the policy switch, so even
        // `unsafe-url` sends the origin.
        let long = Url::parse(&format!("http://a.test/{}", "A".repeat(4200))).expect("long");
        assert_eq!(
            referrer_for_policy("unsafe-url", &long, &Url::parse("http://b.test/x").expect("u")),
            Some("http://a.test/".to_owned())
        );
    }

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
    fn referrer_policy_header_uses_the_last_recognized_token() {
        assert_eq!(
            referrer_policy_from_header("no-referrer, origin").as_deref(),
            Some("origin")
        );
        assert_eq!(
            referrer_policy_from_header("origin, not-a-policy").as_deref(),
            Some("origin")
        );
        assert_eq!(referrer_policy_from_header("not-a-policy").as_deref(), None);
        assert_eq!(
            referrer_policy_from_header("same-origin, , unsafe-url").as_deref(),
            Some("unsafe-url")
        );
    }

    #[test]
    fn insert_rejects_invalid_names_and_values() {
        let mut map = HeaderMap::new();
        assert!(map.insert("bad name", "v").is_err());
        assert!(map.insert("x", "bad\nvalue").is_err());
        assert!(map.is_empty());
    }
}
