//! [`renderer::BrowserServices`] over the browser-owned network service.

use renderer::{BrowserServices, DialOutcome, DialRequest};
use url::Url;

use crate::network::FetchHandle;

pub(crate) struct FetchServices {
    fetch: FetchHandle,
}

impl FetchServices {
    pub(crate) fn new(fetch: FetchHandle) -> Self {
        Self { fetch }
    }
}

impl BrowserServices for FetchServices {
    fn dial(&self, request: &DialRequest) -> Option<DialOutcome> {
        // Run the blocking dial on the browser-owned pool, not the renderer's
        // dial workers ([ADR 0010], [ADR 0011]).
        let fetch = self.fetch.clone();
        let worker = fetch.clone();
        let request = request.clone();
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        fetch
            .try_submit(move || {
                let outcome = worker.dial_request(&request);
                let _ = reply_tx.send(outcome);
            })
            .ok()?;
        reply_rx.recv().ok().flatten()
    }

    fn cookies_for(&self, url: &Url) -> String {
        self.fetch.cookies_for(url)
    }

    fn set_cookie(&self, value: &str, url: &Url) {
        self.fetch.set_cookie(value, url);
    }

    fn mark_dirty(&self) {
        self.fetch.mark_dirty();
    }
}
