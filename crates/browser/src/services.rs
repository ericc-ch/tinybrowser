//! [`renderer::BrowserServices`] over the browser-owned network service.

use std::sync::Arc;

use renderer::{BrowserServices, DialCompletion, DialRequest};
use url::Url;

use crate::network::FetchHandle;
use crate::site::Site;

pub(crate) struct FetchServices {
    fetch: FetchHandle,
    site: Site,
}

impl FetchServices {
    pub(crate) fn new(fetch: FetchHandle, site: Site) -> Self {
        Self { fetch, site }
    }
}

impl BrowserServices for FetchServices {
    fn start_dial(&self, request: DialRequest, completion: DialCompletion) {
        let Some(initiator) = self.site.authorize(&request.initiator) else {
            completion(None);
            return;
        };
        let fetch = self.fetch.clone();
        let worker = fetch.clone();
        let worker_completion = Arc::clone(&completion);
        if fetch
            .try_submit(move || {
                let outcome = worker.dial_request(&request, &initiator);
                worker_completion(outcome);
            })
            .is_err()
        {
            completion(None);
        }
    }

    fn cookies_for(&self, url: &Url) -> String {
        if self.site.allows(url) {
            self.fetch.cookies_for(url)
        } else {
            String::new()
        }
    }

    fn set_cookie(&self, value: &str, url: &Url) {
        if self.site.allows(url) {
            self.fetch.set_cookie(value, url);
        }
    }
}
