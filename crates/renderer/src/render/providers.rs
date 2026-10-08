//! Blitz service providers bridged onto carrier dials.
//!
//! The renderer owns no network runtime of its own (the carrier runtime has
//! IO disabled, so our Tokio-based agent cannot run here): subresource
//! fetches enqueue into the owning document's dial queue through a channel,
//! and completions deliver Blitz handler bytes on the document's thread.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// One Blitz subresource fetch awaiting a carrier dial.
pub(crate) struct BlitzFetch {
    /// Handler id the document files the delivery target under.
    pub id: u64,
    /// Absolute URL to fetch.
    pub url: url::Url,
}

/// Delivery target for one fetch: Blitz's response handler, counted so the
/// resolve loop can tell settled from outstanding.
pub(crate) struct CountingHandler {
    inner: Option<Box<dyn blitz_traits::net::NetHandler>>,
    in_flight: std::sync::Arc<AtomicUsize>,
}

impl blitz_traits::net::NetHandler for CountingHandler {
    fn bytes(mut self: Box<Self>, resolved_url: String, bytes: blitz_traits::net::Bytes) {
        let Some(inner) = self.inner.take() else {
            return;
        };
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        inner.bytes(resolved_url, bytes);
    }
}

impl CountingHandler {
    /// The handler the document files, and the fetch record carrying its id.
    /// Delivery decrements the provider count exactly once: the document
    /// always calls the filed handler on completion or failure (failures
    /// deliver empty bytes, which Blitz reports as a failed resource and
    /// releases its critical hold).
    fn file(
        in_flight: &std::sync::Arc<AtomicUsize>,
        url: url::Url,
        handler: Box<dyn blitz_traits::net::NetHandler>,
    ) -> (Self, BlitzFetch) {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        in_flight.fetch_add(1, Ordering::SeqCst);
        let fetch = BlitzFetch { id, url };
        (
            Self {
                inner: Some(handler),
                in_flight: std::sync::Arc::clone(in_flight),
            },
            fetch,
        )
    }
}

/// Fetches Blitz subresources (stylesheets, images, fonts) through carrier
/// dials: `fetch` files the handler and enqueues the URL; the owning
/// document dials it and delivers the bytes back.
pub(crate) struct TinyNetProvider {
    tx: std::sync::mpsc::Sender<(BlitzFetch, CountingHandler)>,
    in_flight: std::sync::Arc<AtomicUsize>,
}

impl TinyNetProvider {
    /// Builds a provider enqueueing into `tx`, drained by the owning document.
    pub(crate) fn new(tx: std::sync::mpsc::Sender<(BlitzFetch, CountingHandler)>) -> Self {
        Self {
            tx,
            in_flight: std::sync::Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Count of fetches filed but not yet delivered.
    #[cfg(test)]
    pub(crate) fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::SeqCst)
    }
}

impl blitz_traits::net::NetProvider for TinyNetProvider {
    fn fetch(
        &self,
        _doc_id: usize,
        request: blitz_traits::net::Request,
        handler: Box<dyn blitz_traits::net::NetHandler>,
    ) {
        let (counting, fetch) = CountingHandler::file(&self.in_flight, request.url, handler);
        // The document drains the queue on its thread; a disconnected
        // receiver means teardown, and the count already covers the drop.
        let _ = self.tx.send((fetch, counting));
    }
}

/// No-op shell: one-shot screenshots re-resolve explicitly, never on redraw.
pub(crate) struct TinyShell;

impl blitz_traits::shell::ShellProvider for TinyShell {}

/// Records navigations for the carrier to drain; screenshots never navigate.
#[derive(Default)]
pub(crate) struct TinyNav {
    pending: std::sync::Mutex<Vec<url::Url>>,
}

impl TinyNav {
    /// Builds an empty recorder.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Takes every recorded navigation, leaving the recorder empty.
    pub(crate) fn drain(&self) -> Vec<url::Url> {
        std::mem::take(
            &mut self
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }
}

impl blitz_traits::navigation::NavigationProvider for TinyNav {
    fn navigate_to(&self, options: blitz_traits::navigation::NavigationOptions) {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(options.url);
    }
}

#[cfg(test)]
mod tests {
    use super::{CountingHandler, TinyNav, TinyNetProvider};
    use blitz_traits::navigation::{NavigationOptions, NavigationProvider};
    use blitz_traits::net::{NetHandler, NetProvider};

    struct VecHandler {
        tx: std::sync::mpsc::Sender<(String, Vec<u8>)>,
    }

    impl NetHandler for VecHandler {
        fn bytes(self: Box<Self>, resolved_url: String, bytes: blitz_traits::net::Bytes) {
            let _ = self.tx.send((resolved_url, bytes.to_vec()));
        }
    }

    #[test]
    fn fetch_enqueues_and_delivery_settles() {
        let (tx, rx) = std::sync::mpsc::channel();
        let provider = TinyNetProvider::new(tx);
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        provider.fetch(
            1,
            blitz_traits::net::Request::get("http://127.0.0.1/a.css".parse().expect("url")),
            Box::new(VecHandler { tx: reply_tx }),
        );
        assert_eq!(provider.in_flight(), 1);
        let (fetch, handler) = rx.try_recv().expect("enqueued");
        assert_eq!(fetch.url.as_str(), "http://127.0.0.1/a.css");
        Box::new(handler).bytes(
            fetch.url.as_str().to_owned(),
            blitz_traits::net::Bytes::from_static(b"div{color:red}"),
        );
        assert_eq!(provider.in_flight(), 0);
        let (resolved, bytes) = reply_rx.try_recv().expect("delivered");
        assert_eq!(resolved, "http://127.0.0.1/a.css");
        assert_eq!(bytes, b"div{color:red}");
    }

    #[test]
    fn second_delivery_is_noop() {
        use std::sync::Arc;
        use std::sync::atomic::AtomicUsize;
        let in_flight = Arc::new(AtomicUsize::new(1));
        let spent = CountingHandler {
            inner: None,
            in_flight: Arc::clone(&in_flight),
        };
        Box::new(spent).bytes(
            "http://127.0.0.1/b.css".to_owned(),
            blitz_traits::net::Bytes::from_static(b"x"),
        );
        assert_eq!(in_flight.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn nav_records_and_drains() {
        let nav = TinyNav::new();
        nav.navigate_to(NavigationOptions::new(
            "https://example.com/".parse().expect("url"),
            None,
            1,
        ));
        let pending = nav.drain();
        assert_eq!(pending.len(), 1);
        assert!(nav.drain().is_empty());
    }
}
