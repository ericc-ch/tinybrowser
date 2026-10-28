//! One browser identity: the `User-Agent` string and the User-Agent Client
//! Hints.
//!
//! The header, the `Sec-CH-UA*` request headers, `navigator.userAgent`,
//! `navigator.userAgentData`, CSP reports, and CDP's `Browser.getVersion` all
//! read this one value, so a caller that overrides the identity cannot leave
//! one of them claiming another product
//! (<https://wicg.github.io/ua-client-hints/#http-ua-hints>).

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use url::Url;

use crate::HeaderError;

/// The built-in identity: the browser tinybrowser reports itself as.
pub const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/152.0.0.0 Safari/537.36";

/// A validated `User-Agent` field value.
///
/// Nothing in this crate builds one from an unchecked string: the CLI or the
/// embedder parses the value it was given, and every later reader only
/// serializes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserAgent(hyper::header::HeaderValue);

impl UserAgent {
    /// Wraps a value known to be valid at compile time, for a crate constant.
    ///
    /// # Panics
    ///
    /// When `value` contains a byte a field value cannot carry. A call in a
    /// `const` item makes that a compile error, which is how the built-in
    /// identity is checked.
    #[must_use]
    pub const fn from_static(value: &'static str) -> Self {
        Self(hyper::header::HeaderValue::from_static(value))
    }

    /// Parses one `User-Agent` value.
    ///
    /// # Errors
    ///
    /// [`HeaderError::InvalidValue`] when `value` is not a visible ASCII field
    /// value.
    pub fn parse(value: &str) -> Result<Self, HeaderError> {
        hyper::header::HeaderValue::from_str(value)
            .map(Self)
            .map_err(|_| HeaderError::InvalidValue(value.into()))
    }

    /// The value as a string. The type guarantees visible ASCII, so this is
    /// lossless.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.to_str().unwrap_or_default()
    }

    /// The typed field value, for the request header list.
    pub(crate) fn header(&self) -> &hyper::header::HeaderValue {
        &self.0
    }
}

impl Serialize for UserAgent {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(
            self.0
                .to_str()
                .map_err(|_| serde::ser::Error::custom("a user agent is visible ASCII"))?,
        )
    }
}

impl<'de> Deserialize<'de> for UserAgent {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

/// One brand in `Sec-CH-UA` or `navigator.userAgentData.brands`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Brand {
    /// The brand name, e.g. `Chromium`.
    pub brand: String,
    /// Its major version, e.g. `152`.
    pub version: String,
}

impl Brand {
    fn new(brand: &str, version: &str) -> Self {
        Self {
            brand: brand.to_owned(),
            version: version.to_owned(),
        }
    }
}

/// One browser identity: the `User-Agent` string and the client hints that
/// must agree with it
/// (<https://wicg.github.io/ua-client-hints/#http-ua-hints>).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    /// The `User-Agent` request header.
    pub user_agent: UserAgent,
    /// Low-entropy brands, `Sec-CH-UA` and `navigator.userAgentData.brands`.
    pub brands: Vec<Brand>,
    /// High-entropy `navigator.userAgentData.getHighEntropyValues` list.
    pub full_version_list: Vec<Brand>,
    /// `Sec-CH-UA-Platform`, without the quoted-string form, e.g. `Linux`.
    pub platform: String,
    /// `Sec-CH-UA-Mobile`.
    pub mobile: bool,
}

impl Default for Identity {
    fn default() -> Self {
        Self {
            user_agent: UserAgent(hyper::header::HeaderValue::from_static(DEFAULT_USER_AGENT)),
            brands: vec![
                Brand::new("Not_A Brand", "99"),
                Brand::new("Chromium", "152"),
                Brand::new("Google Chrome", "152"),
            ],
            full_version_list: vec![
                Brand::new("Not_A Brand", "99.0.0.0"),
                Brand::new("Chromium", "152.0.0.0"),
                Brand::new("Google Chrome", "152.0.0.0"),
            ],
            platform: "Linux".to_owned(),
            mobile: false,
        }
    }
}

impl Identity {
    /// The `Sec-CH-UA` field value: the brands in spec order, each as a
    /// quoted-string pair
    /// (<https://wicg.github.io/ua-client-hints/#sec-ch-ua>).
    #[must_use]
    pub fn sec_ch_ua(&self) -> String {
        self.brands
            .iter()
            .map(|brand| {
                format!(
                    "\"{}\";v=\"{}\"",
                    quote(&brand.brand),
                    quote(&brand.version)
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The `Sec-CH-UA-Mobile` field value.
    #[must_use]
    pub fn sec_ch_ua_mobile(&self) -> &'static str {
        if self.mobile { "?1" } else { "?0" }
    }

    /// The `Sec-CH-UA-Platform` field value, quoted.
    #[must_use]
    pub fn sec_ch_ua_platform(&self) -> String {
        format!("\"{}\"", quote(&self.platform))
    }

    /// Writes the client hints onto one request, when `url` is potentially
    /// trustworthy: a network attacker must not learn the exact build
    /// (<https://wicg.github.io/ua-client-hints/#sec-ch-ua>).
    ///
    /// # Errors
    ///
    /// [`HeaderError`] when a hint is not a valid field value. The parts come
    /// from this identity, so a built-in one cannot fail.
    pub fn write_client_hints(
        &self,
        headers: &mut crate::HeaderMap,
        url: &Url,
    ) -> Result<(), HeaderError> {
        if !crate::is_potentially_trustworthy(url) {
            return Ok(());
        }
        headers.insert("Sec-CH-UA", self.sec_ch_ua())?;
        headers.insert("Sec-CH-UA-Mobile", self.sec_ch_ua_mobile())?;
        headers.insert("Sec-CH-UA-Platform", self.sec_ch_ua_platform())?;
        Ok(())
    }
}

/// Escapes a brand or platform for a structured-field string.
fn quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len());
    for character in value.chars() {
        if character == '"' || character == '\\' {
            quoted.push('\\');
        }
        quoted.push(character);
    }
    quoted
}

#[cfg(test)]
mod tests {
    use super::{Brand, Identity, UserAgent};
    use url::Url;

    #[test]
    fn the_default_identity_serializes_its_brands_as_the_header() {
        let identity = Identity::default();
        assert_eq!(
            identity.sec_ch_ua(),
            r#""Not_A Brand";v="99", "Chromium";v="152", "Google Chrome";v="152""#
        );
        assert_eq!(identity.sec_ch_ua_mobile(), "?0");
        assert_eq!(identity.sec_ch_ua_platform(), r#""Linux""#);
    }

    #[test]
    fn a_brand_or_platform_cannot_break_out_of_its_quoted_string() {
        let identity = Identity {
            brands: vec![Brand {
                brand: "a\"b\\c".to_owned(),
                version: "1".to_owned(),
            }],
            platform: "x\"y".to_owned(),
            ..Identity::default()
        };
        assert_eq!(identity.sec_ch_ua(), r#""a\"b\\c";v="1""#);
        assert_eq!(identity.sec_ch_ua_platform(), r#""x\"y""#);
    }

    #[test]
    fn client_hints_reach_only_a_trustworthy_url() {
        let identity = Identity::default();
        let mut secure = crate::HeaderMap::new();
        identity
            .write_client_hints(&mut secure, &url("https://example.test/"))
            .expect("hints");
        assert_eq!(
            secure.get("sec-ch-ua"),
            Some(identity.sec_ch_ua().as_bytes()),
            "the brands reach an https request"
        );
        assert_eq!(secure.get("sec-ch-ua-mobile"), Some(b"?0".as_slice()));
        assert_eq!(
            secure.get("sec-ch-ua-platform"),
            Some(br#""Linux""#.as_slice())
        );

        let mut insecure = crate::HeaderMap::new();
        identity
            .write_client_hints(&mut insecure, &url("http://example.test/"))
            .expect("hints");
        assert!(
            insecure.get("sec-ch-ua").is_none(),
            "a network attacker must not learn the exact build"
        );
    }

    fn url(value: &str) -> Url {
        Url::parse(value).expect("url")
    }

    #[test]
    fn a_user_agent_is_validated_when_it_is_parsed() {
        assert!(UserAgent::parse("probe/1").is_ok());
        assert!(
            UserAgent::parse("bad\nvalue").is_err(),
            "a field value cannot carry a newline"
        );
    }
}
