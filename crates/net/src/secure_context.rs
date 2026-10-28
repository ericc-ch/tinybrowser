//! Secure Contexts' "is url potentially trustworthy", shared by referrer
//! policy, client hints, and every other gate that asks whether a URL is a
//! secure context.
//!
//! <https://w3c.github.io/webappsec-secure-contexts/#is-url-trustworthy>

use url::{Host, Url};

/// Whether `url` is potentially trustworthy
/// (<https://w3c.github.io/webappsec-secure-contexts/#is-url-trustworthy>).
///
/// Every answer is decided by the scheme in one place: `about:blank` and
/// `about:srcdoc` inherit their creator's context, `data:` and `file:` are
/// trustworthy by definition, `blob:` follows its creator's origin, and
/// `http`/`ws` need a loopback or `localhost` host. The header, cookie, and
/// client-hint paths share this one definition.
#[must_use]
pub fn is_potentially_trustworthy(url: &Url) -> bool {
    match url.scheme() {
        "about" => matches!(url.path(), "blank" | "srcdoc"),
        "https" | "wss" | "file" | "data" => true,
        "http" | "ws" => match url.host() {
            Some(Host::Ipv4(address)) => address.is_loopback(),
            Some(Host::Ipv6(address)) => address.is_loopback(),
            Some(Host::Domain(host)) => {
                let host = host.trim_end_matches('.').to_ascii_lowercase();
                host == "localhost" || host.ends_with(".localhost")
            }
            None => false,
        },
        "blob" => blob_creator_url(url).is_some_and(|inner| is_potentially_trustworthy(&inner)),
        _ => false,
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
