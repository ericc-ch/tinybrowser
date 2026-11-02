//! Renderer identity: one site instance per scheme plus registrable domain.

use url::Url;

use crate::actor::TabId;

/// Chrome-style site: scheme plus registrable domain (`https://example.co.uk`),
/// or an opaque per-tab instance for non-HTTP(S) documents.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Site(String);

impl Site {
    /// Site for an HTTP(S) URL; `None` for opaque or non-HTTP URLs.
    #[must_use]
    pub(crate) fn for_url(url: &Url) -> Option<Self> {
        if url.scheme() == "http" || url.scheme() == "https" {
            net::site(url).map(Self)
        } else {
            None
        }
    }

    /// Opaque instance for one tab's non-HTTP documents.
    #[must_use]
    pub(crate) fn opaque(tab: TabId) -> Self {
        Self(format!("opaque:tab-{}", tab.get()))
    }

    /// Whether a document URL belongs in a renderer locked to this site.
    ///
    /// HTTP(S) locks accept only their schemeful registrable domain. Opaque
    /// locks accept only non-network documents; they are unique per tab and
    /// cannot be used to claim authority for an HTTP(S) principal.
    /// Chromium applies the same browser-owned process-lock invariant before
    /// granting access to origin data:
    /// <https://chromium.googlesource.com/chromium/src/+/main/docs/process_model_and_site_isolation.md#process-locks>.
    #[must_use]
    pub(crate) fn allows(&self, url: &Url) -> bool {
        match Self::for_url(url) {
            Some(site) => site == *self,
            None => {
                self.0.starts_with("opaque:") && matches!(url.scheme(), "about" | "blob" | "data")
            }
        }
    }

    pub(crate) fn authorize(&self, spec: &str) -> Option<Url> {
        Url::parse(spec).ok().filter(|url| self.allows(url))
    }

    /// Whether a renderer-supplied origin string names an origin inside this
    /// lock.
    ///
    /// The renderer hosts every same-site frame, and a frame's origin differs
    /// from the top-level document's while staying inside the lock:
    /// `https://ads.example.test` inside `https://www.example.test`, or one
    /// host on another port (`site` ignores the port). The lock is the trust
    /// boundary — one renderer process is one authority — and it is the same
    /// bound `postMessage` and `BroadcastChannel` apply. Recording per-frame
    /// origins would not narrow it: the browser would learn them from the
    /// process it is checking, and every frame in that process is the same
    /// trust unit
    /// (<https://chromium.googlesource.com/chromium/src/+/main/docs/process_model_and_site_isolation.md#process-locks>).
    ///
    /// An opaque claim names no origin data: it has no storage keyed by a
    /// tuple, and `"null"` is shared by every opaque document
    /// (<https://html.spec.whatwg.org/multipage/browsers.html#serialized-origins>).
    #[must_use]
    pub(crate) fn authorizes_origin_claim(&self, spec: &str) -> bool {
        match renderer::SerializedOrigin::parse(spec) {
            Some(renderer::SerializedOrigin::Tuple(origin)) => self.allows_origin(&origin),
            Some(renderer::SerializedOrigin::Opaque) | None => false,
        }
    }

    /// Whether one message's claimed sender origin belongs in a renderer locked
    /// to this site.
    ///
    /// An opaque origin claims no authority, so it passes: the receiving
    /// document's target-origin check is what rejects a delivery it does not
    /// want. A tuple origin must belong to this site.
    #[must_use]
    pub(crate) fn allows_serialized_origin(&self, origin: &renderer::SerializedOrigin) -> bool {
        match origin {
            renderer::SerializedOrigin::Opaque => true,
            renderer::SerializedOrigin::Tuple(origin) => self.allows_origin(origin),
        }
    }

    /// Whether a parsed origin belongs in a renderer locked to this site.
    ///
    /// The origin's scheme and host stand in for a document URL: only an
    /// HTTP(S) site lock can accept a tuple origin, and an opaque origin names
    /// no principal to authorize.
    #[must_use]
    pub(crate) fn allows_origin(&self, origin: &url::Origin) -> bool {
        let url::Origin::Tuple(scheme, host, port) = origin else {
            return false;
        };
        let Ok(mut url) = Url::parse(&format!("{scheme}://{host}")) else {
            return false;
        };
        let _ = url.set_port(Some(*port));
        self.allows(&url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renderer_site_lock_rejects_other_principals() {
        let locked =
            Site::for_url(&Url::parse("https://a.example.test/").expect("url")).expect("site");
        assert!(locked.allows(&Url::parse("https://b.example.test/path").expect("url")));
        assert!(!locked.allows(&Url::parse("http://a.example.test/").expect("url")));
        assert!(!locked.allows(&Url::parse("https://other.test/").expect("url")));

        let opaque = Site::opaque(TabId::new(7));
        assert!(opaque.allows(&Url::parse("about:blank").expect("url")));
        assert!(!opaque.allows(&Url::parse("file:///etc/passwd").expect("url")));
        assert!(!opaque.allows(&Url::parse("https://example.test/").expect("url")));
    }

    /// A renderer's origin claim is bounded by the site, not by the top-level
    /// document's origin: every same-site frame lives in the same process, so
    /// its own origin is a legitimate claim while another site's is not
    /// (<https://chromium.googlesource.com/chromium/src/+/main/docs/process_model_and_site_isolation.md#process-locks>).
    #[test]
    fn a_same_site_frame_may_claim_its_own_origin() {
        let locked =
            Site::for_url(&Url::parse("https://www.example.test/").expect("url")).expect("site");
        for allowed in [
            "https://www.example.test",
            "https://ads.example.test",
            "https://example.test:8443",
        ] {
            assert!(locked.authorizes_origin_claim(allowed), "{allowed}");
        }
        for rejected in [
            "https://other.test",
            "http://ads.example.test",
            "null",
            "about:blank",
            "garbage",
        ] {
            assert!(!locked.authorizes_origin_claim(rejected), "{rejected}");
        }
        let opaque = Site::opaque(TabId::new(7));
        assert!(!opaque.authorizes_origin_claim("null"));
        assert!(!opaque.authorizes_origin_claim("https://www.example.test"));
    }
}
