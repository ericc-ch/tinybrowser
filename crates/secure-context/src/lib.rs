//! Secure Contexts' "is url potentially trustworthy", shared by referrer
//! policy, client hints, and every other gate that asks whether a URL is a
//! secure context.
//!
//! <https://w3c.github.io/webappsec-secure-contexts/#is-url-trustworthy>

use url::{Host, Origin, Url};

/// Whether `origin` is potentially trustworthy
/// (<https://w3c.github.io/webappsec-secure-contexts/#is-origin-trustworthy>).
///
/// An opaque origin is never trustworthy: a `data:` document, a sandboxed
/// document with an opaque origin, and a `blob:` URL whose creator has no
/// entry all answer false, whatever their scheme looks like.
#[must_use]
pub fn is_origin_potentially_trustworthy(origin: &Origin) -> bool {
    let Origin::Tuple(scheme, host, _) = origin else {
        return false;
    };
    if scheme.eq_ignore_ascii_case("https")
        || scheme.eq_ignore_ascii_case("wss")
        || scheme.eq_ignore_ascii_case("file")
    {
        return true;
    }
    is_localhost(host)
}

/// Whether `host` is a loopback address or a `localhost` name
/// (<https://w3c.github.io/webappsec-secure-contexts/#is-origin-trustworthy>).
fn is_localhost(host: &Host<String>) -> bool {
    match host {
        Host::Ipv4(address) => address.is_loopback(),
        Host::Ipv6(address) => address.is_loopback(),
        Host::Domain(host) => {
            let host = host.trim_end_matches('.').to_ascii_lowercase();
            host == "localhost" || host.ends_with(".localhost")
        }
    }
}

/// Whether `url` is potentially trustworthy
/// (<https://w3c.github.io/webappsec-secure-contexts/#is-url-trustworthy>).
///
/// The answer comes from the URL's origin: an opaque origin (`data:`) is not
/// trustworthy, a `blob:` URL answers for its creator's origin, and a tuple
/// origin answers for its scheme and host. `about:blank` and `about:srcdoc`
/// inherit their creator's context, which this predicate cannot see; a caller
/// that knows the creator passes the origin instead.
#[must_use]
pub fn is_potentially_trustworthy(url: &Url) -> bool {
    match url.scheme() {
        "about" => matches!(url.path(), "blank" | "srcdoc"),
        // A `file:` URL's origin is opaque in the URL standard, but a local
        // file is potentially trustworthy
        // (<https://w3c.github.io/webappsec-secure-contexts/#is-origin-trustworthy>).
        "file" => true,
        "blob" => blob_creator_url(url).is_some_and(|inner| is_potentially_trustworthy(&inner)),
        _ => is_origin_potentially_trustworthy(&url.origin()),
    }
}

/// The URL whose origin a `blob:` URL carries, per the URL standard's blob
/// origin step
/// (<https://url.spec.whatwg.org/#origin>).
///
/// Only an entry minted by `URL.createObjectURL()` exists, and its stored
/// origin serializes to `http`, `https`, or `file`; any other inner scheme —
/// including a nested `blob:` — names no entry, so its origin is opaque.
fn blob_creator_url(url: &Url) -> Option<Url> {
    let inner = Url::parse(url.as_str().strip_prefix("blob:")?).ok()?;
    matches!(inner.scheme(), "http" | "https" | "file").then_some(inner)
}

#[cfg(test)]
mod tests {
    use super::is_potentially_trustworthy;
    use url::Url;

    fn trustworthy(value: &str) -> bool {
        is_potentially_trustworthy(&Url::parse(value).expect("url"))
    }

    #[test]
    fn local_documents_without_an_http_response_are_trustworthy() {
        // The branches WPT's `secure-contexts/` cannot reach end to end.
        assert!(trustworthy("about:srcdoc"));
        assert!(trustworthy("file:///tmp/page.html"));
        assert!(trustworthy("blob:https://example.com/uuid"));
        assert!(trustworthy("blob:http://localhost/uuid"));
    }

    #[test]
    fn nested_blob_urls_have_an_opaque_origin() {
        // Only `URL.createObjectURL` mints an entry, and it serializes an
        // origin, never another `blob:` URL
        // (<https://url.spec.whatwg.org/#origin>).
        assert!(!trustworthy("blob:blob:https://example.com/uuid"));
        assert!(!trustworthy("blob:blob:blob:https://example.com/uuid"));
        assert!(!trustworthy("blob:data:,x"));
    }
}
