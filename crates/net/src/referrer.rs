//! Referrer policy: the policy enum, its three delivery parsers, and the one
//! "Determine request's Referrer" implementation every dial goes through.
//!
//! The policy is a closed set of eight tokens plus "unset", which the request
//! construction resolves from the policy container before main fetch. Keeping
//! the type closed makes every policy branch exhaustive and keeps the legacy
//! `<meta name=referrer>` keywords out of headers and attributes.
//!
//! <https://w3c.github.io/webappsec-referrer-policy/>

use enumerated::Enumerated;
use serde::{Deserialize, Serialize};
use url::{Origin, Url};

use secure_context::is_potentially_trustworthy;

enumerated::enumerated! {
    /// A referrer policy
    /// (<https://w3c.github.io/webappsec-referrer-policy/#referrer-policies>).
    ///
    /// The empty string is not a variant: it means "no policy here, inherit one",
    /// so callers use `Option<ReferrerPolicy>` until the policy container or the
    /// user agent default supplies a value.
    #[derive(Default, Serialize, Deserialize)]
    #[serde(rename_all = "kebab-case")]
    pub ReferrerPolicy {
        /// Send no `Referer`.
        NoReferrer => "no-referrer",
        /// Send the full URL unless the target is a downgrade.
        NoReferrerWhenDowngrade => "no-referrer-when-downgrade",
        /// Send the full URL only to the same origin.
        SameOrigin => "same-origin",
        /// Send the origin to every origin.
        Origin => "origin",
        /// Send the origin, except on a downgrade.
        StrictOrigin => "strict-origin",
        /// Send the full URL same-origin, the origin cross-origin.
        OriginWhenCrossOrigin => "origin-when-cross-origin",
        /// Send the full URL same-origin, the origin cross-origin, and nothing on
        /// a downgrade. The user agent default.
        #[default]
        StrictOriginWhenCrossOrigin => "strict-origin-when-cross-origin",
        /// Send the full URL to every origin.
        UnsafeUrl => "unsafe-url",
    }
}

impl ReferrerPolicy {
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

/// Fetch's request referrer, resolved against the client environment the
/// request runs in
/// (<https://fetch.spec.whatwg.org/#concept-request-referrer>).
///
/// The `"client"` branch carries the environment instead of being a marker
/// beside a nullable one, so "client with no environment" cannot be built.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum Referrer {
    /// `"no-referrer"`: the determination is skipped entirely.
    #[default]
    NoReferrer,
    /// `"client"`: resolve against this request's client environment.
    Client(ReferrerClient),
    /// An explicit URL, already checked same-origin with the client by the
    /// browser boundary.
    Url(Url),
}

/// The request's client environment for the `"client"` referrer branch: the
/// document's origin (the opaque check) and the document URL the spec walks
/// `srcdoc` parents to.
#[derive(Clone, Debug, Eq, PartialEq)]
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
/// Later redirect hops pass the previous hop's referrer as
/// [`Referrer::Url`], which is what HTTP redirect fetch does, so the client
/// environment is read once per request rather than once per hop.
#[must_use]
pub fn determine_referrer(
    policy: ReferrerPolicy,
    referrer: &Referrer,
    target: &Url,
) -> Option<Url> {
    // Step 3: the referrer switch.
    let source = match referrer {
        Referrer::NoReferrer => return None,
        Referrer::Client(client) => {
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
    let referrer_url = strip_url(source, false);
    let referrer_origin = strip_url(source, true);
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
/// The serialized origin keeps a trailing `/` because an empty path
/// serializes that way.
fn strip_url(url: &Url, origin_only: bool) -> Url {
    let mut stripped = url.clone();
    let _ = stripped.set_username("");
    let _ = stripped.set_password(None);
    stripped.set_fragment(None);
    if origin_only {
        stripped.set_path("");
        stripped.set_query(None);
    }
    stripped
}
