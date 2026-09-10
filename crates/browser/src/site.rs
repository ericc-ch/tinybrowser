//! Renderer identity: one site instance per scheme plus registrable domain.
//!
//! [ADR 0011](../../../docs/adrs/0011-renderer-processes-per-site.md).

use url::Url;

use crate::actor::PageId;

/// Chrome-style site: scheme plus registrable domain (`https://example.co.uk`),
/// or an opaque per-page instance for non-HTTP(S) documents.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SiteKey(String);

impl SiteKey {
    /// Site for an HTTP(S) URL; `None` for opaque or non-HTTP URLs.
    #[must_use]
    pub fn for_url(url: &Url) -> Option<Self> {
        if url.scheme() == "http" || url.scheme() == "https" {
            net::site(url).map(Self)
        } else {
            None
        }
    }

    /// Opaque instance for one page's non-HTTP documents.
    #[must_use]
    pub fn opaque(page: PageId) -> Self {
        Self(format!("opaque:page-{}", page.get()))
    }

    /// Whether this key is a per-page opaque instance, which is never reusable
    /// by another page.
    #[must_use]
    pub fn is_opaque(&self) -> bool {
        self.0.starts_with("opaque:")
    }

    /// Display form, for diagnostics.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
