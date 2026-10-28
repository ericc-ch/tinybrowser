//! One browser identity: the `User-Agent` string and the User-Agent Client
//! Hints.
//!
//! The header, the `Sec-CH-UA*` request headers, `navigator.userAgent`,
//! `navigator.userAgentData`, CSP reports, and CDP's `Browser.getVersion` all
//! read this one value, so a caller that overrides the identity cannot leave
//! one of them claiming another product
//! (<https://wicg.github.io/ua-client-hints/#http-ua-hints>).

use hyper::header::{HeaderName, HeaderValue};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use url::Url;

use crate::HeaderError;

/// The built-in identity's `User-Agent` header: the reduced Chrome 152 string.
pub const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/152.0.0.0 Safari/537.36";

/// The built-in identity's full build, as `Sec-CH-UA` and the client hints
/// report it.
const DEFAULT_FULL_VERSION: &str = "152.0.7977.82";

/// A validated `User-Agent` field value.
///
/// Nothing in this crate builds one from an unchecked string: the CLI or the
/// embedder parses the value it was given, and every later reader only
/// serializes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserAgent(HeaderValue);

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
        Self(HeaderValue::from_static(value))
    }

    /// Parses one `User-Agent` value.
    ///
    /// # Errors
    ///
    /// [`HeaderError::InvalidValue`] when `value` is not a visible ASCII field
    /// value.
    pub fn parse(value: &str) -> Result<Self, HeaderError> {
        HeaderValue::from_str(value)
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
    pub(crate) fn header(&self) -> &HeaderValue {
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

/// The built-in identity's user agent, checked at compile time.
const DEFAULT_IDENTITY_USER_AGENT: UserAgent = UserAgent::from_static(DEFAULT_USER_AGENT);

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

/// One browser identity: the `User-Agent` string and every User-Agent Client
/// Hint a page or a request can observe.
///
/// The fields mirror Chromium's `UserAgentMetadata`
/// (<https://chromium.googlesource.com/chromium/src/+/main/third_party/blink/public/common/user_agent/user_agent_metadata.h>),
/// so a page's low- and high-entropy hints, its `Navigator.platform`, and the
/// CDP product all name the same product.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    /// The `User-Agent` request header.
    pub user_agent: UserAgent,
    /// Low-entropy brands, `Sec-CH-UA` and `navigator.userAgentData.brands`.
    pub brands: Vec<Brand>,
    /// High-entropy `navigator.userAgentData.getHighEntropyValues` list.
    pub full_version_list: Vec<Brand>,
    /// The CDP product token, e.g. `Chrome/152.0.7977.82`.
    pub product: String,
    /// `Sec-CH-UA-Platform`, without the quoted-string form, e.g. `Linux`.
    pub platform: String,
    /// The high-entropy `platformVersion` hint.
    pub platform_version: String,
    /// The high-entropy `architecture` hint.
    pub architecture: String,
    /// The high-entropy `model` hint.
    pub model: String,
    /// `Sec-CH-UA-Mobile`.
    pub mobile: bool,
    /// The high-entropy `bitness` hint.
    pub bitness: String,
    /// The high-entropy `wow64` hint.
    pub wow64: bool,
    /// The high-entropy `formFactors` hint.
    pub form_factors: Vec<String>,
    /// The deprecated `Navigator.platform` string, e.g. `Linux x86_64`. It is
    /// not a client hint; the page still reads it.
    pub navigator_platform: String,
}

impl Default for Identity {
    fn default() -> Self {
        // The brand versions are the full version's major, so a build bump is
        // one edit. The reduced UA repeats the major because it is a `const`
        // string; `the_default_identity_names_one_build_everywhere` holds the
        // two together.
        let major = DEFAULT_FULL_VERSION
            .split('.')
            .next()
            .unwrap_or(DEFAULT_FULL_VERSION);
        Self {
            user_agent: DEFAULT_IDENTITY_USER_AGENT,
            brands: vec![
                Brand::new("Not_A Brand", "99"),
                Brand::new("Chromium", major),
                Brand::new("Google Chrome", major),
            ],
            full_version_list: vec![
                Brand::new("Not_A Brand", "99.0.0.0"),
                Brand::new("Chromium", DEFAULT_FULL_VERSION),
                Brand::new("Google Chrome", DEFAULT_FULL_VERSION),
            ],
            product: format!("Chrome/{DEFAULT_FULL_VERSION}"),
            platform: "Linux".to_owned(),
            platform_version: String::new(),
            architecture: "x86".to_owned(),
            model: String::new(),
            mobile: false,
            bitness: "64".to_owned(),
            wow64: false,
            form_factors: vec!["Desktop".to_owned()],
            navigator_platform: "Linux x86_64".to_owned(),
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

    /// The three client-hint field values, formatted once.
    ///
    /// # Errors
    ///
    /// [`HeaderError`] when a brand or the platform is not a valid field
    /// value. Callers that own the identity validate it at their boundary.
    pub fn client_hints(&self) -> Result<ClientHints, HeaderError> {
        let field = |value: String| {
            HeaderValue::from_str(&value).map_err(|_| HeaderError::InvalidValue(value.into()))
        };
        Ok(ClientHints {
            ua: field(self.sec_ch_ua())?,
            mobile: field(self.sec_ch_ua_mobile().to_owned())?,
            platform: field(self.sec_ch_ua_platform())?,
        })
    }

    /// Checks that every hint this identity sends is a valid field value.
    ///
    /// # Errors
    ///
    /// [`HeaderError`] naming the offending value.
    pub fn validate(&self) -> Result<(), HeaderError> {
        self.client_hints().map(|_| ())
    }
}

/// The `Sec-CH-UA`, `Sec-CH-UA-Mobile`, and `Sec-CH-UA-Platform` values, ready
/// to be written onto a request.
///
/// Built once per agent: a hop only clones three validated `HeaderValue`s
/// instead of formatting the brands again.
#[derive(Clone, Debug)]
pub struct ClientHints {
    ua: HeaderValue,
    mobile: HeaderValue,
    platform: HeaderValue,
}

impl ClientHints {
    /// Writes the hints onto one request, when `url` is potentially
    /// trustworthy: a network attacker must not learn the exact build
    /// (<https://wicg.github.io/ua-client-hints/#sec-ch-ua>).
    pub fn write(&self, headers: &mut crate::HeaderMap, url: &Url) {
        if !crate::is_potentially_trustworthy(url) {
            return;
        }
        headers.insert_wire(HeaderName::from_static("sec-ch-ua"), self.ua.clone());
        headers.insert_wire(
            HeaderName::from_static("sec-ch-ua-mobile"),
            self.mobile.clone(),
        );
        headers.insert_wire(
            HeaderName::from_static("sec-ch-ua-platform"),
            self.platform.clone(),
        );
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

    /// The reduced UA, the brands, the full version list, and the CDP product
    /// name one build.
    #[test]
    fn the_default_identity_names_one_build_everywhere() {
        let identity = Identity::default();
        assert!(
            identity.user_agent.as_str().contains("Chrome/152.0.0.0"),
            "the reduced UA carries the major version"
        );
        let chromium = identity
            .full_version_list
            .iter()
            .find(|brand| brand.brand == "Chromium")
            .expect("a Chromium brand");
        assert!(
            chromium.version.starts_with("152."),
            "the full version agrees with the UA major"
        );
        assert_eq!(identity.product, format!("Chrome/{}", chromium.version));
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
        let hints = identity.client_hints().expect("hints");
        let mut secure = crate::HeaderMap::new();
        hints.write(&mut secure, &url("https://example.test/"));
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
        hints.write(&mut insecure, &url("http://example.test/"));
        assert!(
            insecure.get("sec-ch-ua").is_none(),
            "a network attacker must not learn the exact build"
        );
    }

    /// The hints are field values, so a control byte in a brand is rejected
    /// where the identity is validated, not on a request mid-page.
    #[test]
    fn an_identity_with_a_broken_brand_fails_validation() {
        let identity = Identity {
            brands: vec![Brand {
                brand: "bad\nbrand".to_owned(),
                version: "1".to_owned(),
            }],
            ..Identity::default()
        };
        assert!(identity.validate().is_err());
        assert!(identity.client_hints().is_err());
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
