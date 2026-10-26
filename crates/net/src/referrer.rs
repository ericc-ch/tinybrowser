//! Referrer policy: the policy enum, its three delivery parsers, and the one
//! "Determine request's Referrer" implementation every dial goes through.
//!
//! The policy is a closed set of eight tokens plus "unset", which the request
//! construction resolves from the policy container before main fetch. Keeping
//! the type closed makes every policy branch exhaustive and keeps the legacy
//! `<meta name=referrer>` keywords out of headers and attributes.
//!
//! <https://w3c.github.io/webappsec-referrer-policy/>

use serde::{Deserialize, Serialize};
use url::{Origin, Url};

use crate::secure_context::is_potentially_trustworthy;

/// A referrer policy
/// (<https://w3c.github.io/webappsec-referrer-policy/#referrer-policies>).
///
/// The empty string is not a variant: it means "no policy here, inherit one",
/// so callers use `Option<ReferrerPolicy>` until the policy container or the
/// user agent default supplies a value.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReferrerPolicy {
    /// Send no `Referer`.
    NoReferrer,
    /// Send the full URL unless the target is a downgrade.
    NoReferrerWhenDowngrade,
    /// Send the full URL only to the same origin.
    SameOrigin,
    /// Send the origin to every origin.
    Origin,
    /// Send the origin, except on a downgrade.
    StrictOrigin,
    /// Send the full URL same-origin, the origin cross-origin.
    OriginWhenCrossOrigin,
    /// Send the full URL same-origin, the origin cross-origin, and nothing on
    /// a downgrade. The user agent default.
    #[default]
    StrictOriginWhenCrossOrigin,
    /// Send the full URL to every origin.
    UnsafeUrl,
}

impl ReferrerPolicy {
    /// Every policy, in spec order
    /// (<https://w3c.github.io/webappsec-referrer-policy/#referrer-policies>).
    pub const ALL: [Self; 8] = [
        Self::NoReferrer,
        Self::NoReferrerWhenDowngrade,
        Self::SameOrigin,
        Self::Origin,
        Self::StrictOrigin,
        Self::OriginWhenCrossOrigin,
        Self::StrictOriginWhenCrossOrigin,
        Self::UnsafeUrl,
    ];

    /// The policy's keyword, the one spelling used by the header parser, meta
    /// content, and content-attribute reflection.
    #[must_use]
    pub const fn keyword(self) -> &'static str {
        match self {
            Self::NoReferrer => "no-referrer",
            Self::NoReferrerWhenDowngrade => "no-referrer-when-downgrade",
            Self::SameOrigin => "same-origin",
            Self::Origin => "origin",
            Self::StrictOrigin => "strict-origin",
            Self::OriginWhenCrossOrigin => "origin-when-cross-origin",
            Self::StrictOriginWhenCrossOrigin => "strict-origin-when-cross-origin",
            Self::UnsafeUrl => "unsafe-url",
        }
    }

    /// One policy token, ASCII case-insensitive and untrimmed. Enumerated
    /// attributes and header list members are trimmed by their own parsers,
    /// so a stray space is not a policy here.
    #[must_use]
    pub fn parse(token: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|policy| policy.keyword().eq_ignore_ascii_case(token))
    }

    /// The last recognized token of a `Referrer-Policy` response field list
    /// (<https://w3c.github.io/webappsec-referrer-policy/#parse-referrer-policy-from-header>).
    ///
    /// Step 1 is Fetch's "extract header list values": every field value has
    /// to parse as an RFC 9110 list of `ALPHA / "-"` tokens. A single
    /// malformed value makes the whole header unusable, so the response
    /// carries no policy and the next source (a redirect, the document's
    /// policy container, or the user agent default) decides. Unknown tokens
    /// are skipped so a newer policy can ship with an older fallback in the
    /// same field. The legacy meta keywords (`never`, `default`, `always`,
    /// `origin-when-crossorigin`) are not policies here.
    #[must_use]
    pub fn from_header<'a>(values: impl IntoIterator<Item = &'a str>) -> Option<Self> {
        let mut policy = None;
        for value in values {
            for token in header_list_members(value)? {
                if let Some(parsed) = Self::parse(token) {
                    policy = Some(parsed);
                }
            }
        }
        policy
    }

    /// `<meta name=referrer>` content, with the legacy keywords the HTML
    /// standard keeps for that element
    /// (<https://html.spec.whatwg.org/multipage/semantics.html#meta-referrer>).
    ///
    /// The content is stripped and compared ASCII case-insensitively.
    #[must_use]
    pub fn from_meta(content: &str) -> Option<Self> {
        let token = content.trim();
        if token.eq_ignore_ascii_case("never") {
            return Some(Self::NoReferrer);
        }
        if token.eq_ignore_ascii_case("default") {
            return Some(Self::StrictOriginWhenCrossOrigin);
        }
        if token.eq_ignore_ascii_case("always") {
            return Some(Self::UnsafeUrl);
        }
        if token.eq_ignore_ascii_case("origin-when-crossorigin") {
            return Some(Self::OriginWhenCrossOrigin);
        }
        Self::parse(token)
    }
}

/// One `Referrer-Policy` field value split into RFC 9110 list members
/// (<https://www.rfc-editor.org/rfc/rfc9110#section-5.6.1>).
///
/// `None` is the extraction failure that makes the whole field list unusable.
/// Empty members are legal and skipped; every other member must be one or
/// more `ALPHA` or `-` characters, which covers both `policy-token` and
/// `extension-token`.
fn header_list_members(value: &str) -> Option<Vec<&str>> {
    let mut members = Vec::new();
    for member in value.split(',') {
        let member = member.trim_matches([' ', '\t']);
        if member.is_empty() {
            continue;
        }
        if !member
            .bytes()
            .all(|byte| byte.is_ascii_alphabetic() || byte == b'-')
        {
            return None;
        }
        members.push(member);
    }
    Some(members)
}

/// Fetch's request referrer
/// (<https://fetch.spec.whatwg.org/#concept-request-referrer>).
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Referrer {
    /// `"no-referrer"`: the determination is skipped entirely.
    #[default]
    NoReferrer,
    /// `"client"`: resolve against the request's client environment.
    Client,
    /// An explicit URL, already checked same-origin with the client by the
    /// browser boundary.
    Url(Url),
}

impl Referrer {
    /// The wire/IDL spelling: `""`, `"about:client"`, or a serialized URL.
    #[must_use]
    pub fn from_idl(value: &str) -> Option<Self> {
        if value.is_empty() {
            return Some(Self::NoReferrer);
        }
        if value.eq_ignore_ascii_case("about:client") {
            return Some(Self::Client);
        }
        Url::parse(value).ok().map(Self::Url)
    }
}

/// The request's client environment for the `"client"` referrer branch: the
/// document's origin (the opaque check) and the document URL the spec walks
/// `srcdoc` parents to.
#[derive(Clone, Debug)]
pub struct ReferrerClient {
    /// The document's origin; opaque documents send no referrer.
    pub origin: Origin,
    /// The document's URL, already walked out of any `srcdoc` chain.
    pub url: Url,
}

/// "Determine request's Referrer": returns the `Referer` URL or `None` for
/// `no referrer`
/// (<https://w3c.github.io/webappsec-referrer-policy/#determine-requests-referrer>).
///
/// `client` is only read for [`Referrer::Client`]; later redirect hops pass
/// the previous hop's referrer as [`Referrer::Url`], which is what HTTP
/// redirect fetch does.
#[must_use]
pub fn determine_referrer(
    policy: ReferrerPolicy,
    client: Option<&ReferrerClient>,
    referrer: &Referrer,
    target: &Url,
) -> Option<Url> {
    // Step 3: the referrer switch. Fetch never calls this algorithm for
    // "no-referrer", so that branch is unreachable there, but treating it as
    // no referrer here keeps every caller safe.
    let source = match referrer {
        Referrer::NoReferrer => return None,
        Referrer::Client => {
            let client = client?;
            if matches!(client.origin, Origin::Opaque(_)) {
                return None;
            }
            &client.url
        }
        Referrer::Url(url) => url,
    };
    // "Strip url for use as a referrer" rejects local schemes. A document URL
    // outside HTTP(S) has no `Referer` to send in this engine either.
    if !matches!(source.scheme(), "http" | "https") {
        return None;
    }
    let referrer_url = strip_url(source, false)?;
    let referrer_origin = strip_url(source, true)?;
    // Step 6: a referrer serialized over 4096 characters culls to the origin
    // before the policy switch, even under `unsafe-url`.
    let referrer_url = if referrer_url.as_str().len() > 4096 {
        referrer_origin.clone()
    } else {
        referrer_url
    };
    let same_origin = referrer_url.origin() == target.origin();
    let source_trustworthy = is_potentially_trustworthy(&referrer_url);
    let target_trustworthy = is_potentially_trustworthy(target);
    let downgrade = source_trustworthy && !target_trustworthy;
    match policy {
        ReferrerPolicy::NoReferrer => None,
        ReferrerPolicy::Origin => Some(referrer_origin),
        ReferrerPolicy::UnsafeUrl => Some(referrer_url),
        ReferrerPolicy::StrictOrigin => (!downgrade).then_some(referrer_origin),
        ReferrerPolicy::StrictOriginWhenCrossOrigin => {
            if same_origin {
                Some(referrer_url)
            } else if downgrade {
                None
            } else {
                Some(referrer_origin)
            }
        }
        ReferrerPolicy::SameOrigin => same_origin.then_some(referrer_url),
        ReferrerPolicy::OriginWhenCrossOrigin => Some(if same_origin {
            referrer_url
        } else {
            referrer_origin
        }),
        ReferrerPolicy::NoReferrerWhenDowngrade => (!downgrade).then_some(referrer_url),
    }
}

/// "Strip url for use as a referrer"
/// (<https://w3c.github.io/webappsec-referrer-policy/#strip-url>): drop the
/// fragment, username, and password, and with `origin_only` the path and
/// query too.
///
/// A local scheme has no referrer. The serialized origin keeps a trailing
/// `/` because an empty path serializes that way.
fn strip_url(url: &Url, origin_only: bool) -> Option<Url> {
    if matches!(url.scheme(), "about" | "blob" | "data") {
        return None;
    }
    let mut stripped = url.clone();
    let _ = stripped.set_username("");
    let _ = stripped.set_password(None);
    stripped.set_fragment(None);
    if origin_only {
        stripped.set_path("");
        stripped.set_query(None);
    }
    Some(stripped)
}

#[cfg(test)]
mod tests {
    use super::ReferrerPolicy;

    #[test]
    fn policy_keywords_are_the_spec_strings() {
        for (policy, keyword) in [
            (ReferrerPolicy::NoReferrer, "no-referrer"),
            (
                ReferrerPolicy::NoReferrerWhenDowngrade,
                "no-referrer-when-downgrade",
            ),
            (ReferrerPolicy::SameOrigin, "same-origin"),
            (ReferrerPolicy::Origin, "origin"),
            (ReferrerPolicy::StrictOrigin, "strict-origin"),
            (
                ReferrerPolicy::OriginWhenCrossOrigin,
                "origin-when-cross-origin",
            ),
            (
                ReferrerPolicy::StrictOriginWhenCrossOrigin,
                "strict-origin-when-cross-origin",
            ),
            (ReferrerPolicy::UnsafeUrl, "unsafe-url"),
        ] {
            assert_eq!(policy.keyword(), keyword);
            assert_eq!(ReferrerPolicy::parse(keyword), Some(policy));
        }
    }

    #[test]
    fn parse_is_ascii_case_insensitive_and_untrimmed() {
        assert_eq!(
            ReferrerPolicy::parse("ORIGIN"),
            Some(ReferrerPolicy::Origin)
        );
        assert_eq!(ReferrerPolicy::parse(" origin"), None);
        assert_eq!(ReferrerPolicy::parse(""), None);
    }
}
