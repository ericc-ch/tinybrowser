//! Secure Contexts' "is url potentially trustworthy", shared by referrer
//! policy, client hints, and every other gate that asks whether a URL is a
//! secure context.
//!
//! <https://w3c.github.io/webappsec-secure-contexts/#is-url-trustworthy>

use url::{Host, Url};

/// Whether `url` is potentially trustworthy
/// (<https://w3c.github.io/webappsec-secure-contexts/#is-url-trustworthy>).
///
/// `about:blank` and `about:srcdoc` inherit their creator's context, `data:`
/// is trustworthy by definition, and every other URL is judged by its origin:
/// HTTPS and WSS, file URLs, and loopback or `localhost` hosts over HTTP/WS.
/// The header, cookie, and client-hint paths share this one definition.
#[must_use]
pub fn is_potentially_trustworthy(url: &Url) -> bool {
    if url.scheme() == "about" && matches!(url.path(), "blank" | "srcdoc") {
        return true;
    }
    match url.scheme() {
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
        // `blob:` carries its creator's origin, so it is trustworthy exactly
        // when that origin is.
        "blob" => url
            .as_str()
            .strip_prefix("blob:")
            .and_then(|inner| Url::parse(inner).ok())
            .is_some_and(|inner| is_potentially_trustworthy(&inner)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::is_potentially_trustworthy;
    use url::Url;

    fn trustworthy(value: &str) -> bool {
        is_potentially_trustworthy(&Url::parse(value).expect("url"))
    }

    #[test]
    fn trustworthy_urls_cover_secure_contexts_loopback_and_blobs() {
        for value in [
            "https://example.com/",
            "wss://example.com/socket",
            "file:///tmp/page.html",
            "data:,hello",
            "about:blank",
            "about:srcdoc",
            "blob:https://example.com/uuid",
            "http://127.0.0.1/",
            "http://localhost/",
            "http://foo.localhost./",
            "http://[::1]/",
        ] {
            assert!(trustworthy(value), "{value} is potentially trustworthy");
        }
    }

    #[test]
    fn untrustworthy_urls_are_plain_http_and_other_creators_blobs() {
        for value in [
            "http://example.com/",
            "ws://example.com/socket",
            "blob:http://example.com/uuid",
            "http://192.0.2.1/",
            "ftp://example.com/",
        ] {
            assert!(!trustworthy(value), "{value} is not trustworthy");
        }
    }
}
