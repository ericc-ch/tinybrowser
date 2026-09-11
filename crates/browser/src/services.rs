//! [`renderer::BrowserServices`] over the browser-owned network service.

use std::sync::Arc;

use renderer::{BrowserServices, DialCompletion, DialRequest};
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
    fn start_dial(&self, request: DialRequest, completion: DialCompletion) {
        let fetch = self.fetch.clone();
        let worker = fetch.clone();
        let worker_completion = Arc::clone(&completion);
        if fetch
            .try_submit(move || {
                let outcome = worker.dial_request(&request);
                worker_completion(outcome);
            })
            .is_err()
        {
            completion(None);
        }
    }

    fn cookies_for(&self, url: &Url) -> String {
        self.fetch.cookies_for(url)
    }

    fn set_cookie(&self, value: &str, url: &Url) {
        self.fetch.set_cookie(value, url);
    }
}
