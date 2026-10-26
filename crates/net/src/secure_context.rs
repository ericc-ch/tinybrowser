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
    fn local_documents_without_an_http_response_are_trustworthy() {
        // The branches WPT's `secure-contexts/` cannot reach end to end.
        assert!(trustworthy("about:srcdoc"));
        assert!(trustworthy("file:///tmp/page.html"));
    }
}
