//! Secure Contexts' "is url potentially trustworthy", shared by referrer
//! policy, client hints, and every other gate that asks whether a URL is a
//! secure context.
//!
//! <https://w3c.github.io/webappsec-secure-contexts/#is-url-trustworthy>

use url::{Host, Origin, Url};

/// Whether `url` is potentially trustworthy
/// (<https://w3c.github.io/webappsec-secure-contexts/#is-url-trustworthy>).
///
/// `about:blank`, `about:srcdoc`, `data:`, and `file:` are trustworthy by
/// definition; a `blob:` URL answers for the entry `URL.createObjectURL()`
/// minted; a tuple URL answers for its scheme and host. This is the URL rule,
/// not HTML's environment rule: a `data:` document is still never a secure
/// context, which [`is_secure_environment`] decides.
#[must_use]
pub fn is_potentially_trustworthy(url: &Url) -> bool {
    match url.scheme() {
        "about" => matches!(url.path(), "blank" | "srcdoc"),
        "data" | "file" => true,
        "blob" => is_trustworthy_origin(&url.origin()) || blob_creator_is_file(url),
        _ => is_trustworthy_origin(&url.origin()),
    }
}

/// Whether an environment whose document URL is `url` is a secure context,
/// given its creator's bit (`None` for a top-level environment)
/// (<https://html.spec.whatwg.org/multipage/webappapis.html#secure-contexts>).
///
/// This is HTML's environment algorithm rather than the URL rule: a `data:`
/// document is never a secure context even though its URL is potentially
/// trustworthy, and an insecure creator makes every descendant insecure.
#[must_use]
pub fn is_secure_environment(url: &Url, creator: Option<bool>) -> bool {
    url.scheme() != "data" && is_potentially_trustworthy(url) && creator.unwrap_or(true)
}

/// Whether `origin` is potentially trustworthy
/// (<https://w3c.github.io/webappsec-secure-contexts/#is-origin-trustworthy>).
///
/// An opaque origin is never trustworthy, and `https`/`wss` or a loopback
/// host is.
fn is_trustworthy_origin(origin: &Origin) -> bool {
    let Origin::Tuple(scheme, host, _) = origin else {
        return false;
    };
    matches!(scheme.as_str(), "https" | "wss") || is_localhost(host)
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

/// Whether `url` is a `blob:` URL minted on a `file:` URL.
///
/// Every other blob entry resolves through [`Url::origin()`]; a local file is
/// potentially trustworthy while the url crate reports its origin as opaque,
/// so this one creator needs the inner URL.
fn blob_creator_is_file(url: &Url) -> bool {
    url.as_str()
        .strip_prefix("blob:")
        .and_then(|inner| Url::parse(inner).ok())
        .is_some_and(|inner| inner.scheme() == "file")
}

#[cfg(test)]
mod tests {
    use super::{is_potentially_trustworthy, is_secure_environment};
    use url::Url;

    fn url(value: &str) -> Url {
        Url::parse(value).expect("url")
    }

    fn trustworthy(value: &str) -> bool {
        is_potentially_trustworthy(&url(value))
    }

    #[test]
    fn local_documents_without_an_http_response_are_trustworthy() {
        // The branches WPT's `secure-contexts/` cannot reach end to end.
        assert!(trustworthy("about:srcdoc"));
        assert!(trustworthy("file:///tmp/page.html"));
        assert!(trustworthy("blob:https://example.com/uuid"));
        assert!(trustworthy("blob:http://localhost/uuid"));
        assert!(trustworthy("blob:file:///tmp/page.html"));
    }

    #[test]
    fn a_data_url_is_trustworthy_but_never_a_secure_environment() {
        // "Is url potentially trustworthy" answers for the URL alone
        // (<https://w3c.github.io/webappsec-secure-contexts/#is-url-trustworthy>);
        // HTML excludes `data:` from secure contexts, not this predicate.
        assert!(trustworthy("data:,x"));
        assert!(!is_secure_environment(&url("data:,x"), None));
        assert!(!is_secure_environment(&url("data:,x"), Some(true)));
    }

    #[test]
    fn an_environment_is_secure_only_inside_a_secure_creator() {
        assert!(is_secure_environment(&url("https://a.test/"), None));
        assert!(is_secure_environment(&url("https://a.test/"), Some(true)));
        assert!(!is_secure_environment(&url("https://a.test/"), Some(false)));
        assert!(!is_secure_environment(&url("http://a.test/"), None));
        assert!(is_secure_environment(&url("http://localhost/"), None));
    }

    #[test]
    fn a_blob_url_answers_for_its_creator() {
        // The blob origin step parses the encoded URL and answers for that
        // URL's origin (<https://url.spec.whatwg.org/#origin>), so a creator
        // with no trustworthy origin does not make its blob trustworthy.
        assert!(!trustworthy("blob:data:,x"));
        assert!(!trustworthy("blob:http://a.test/uuid"));
    }
}
