//! Renderer identity: one site instance per scheme plus registrable domain.
//!
//! [ADR 0011](../../../docs/adrs/0011-renderer-processes-per-site.md).

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
}
