//! Browser-loadable tinybrowser page engine component.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::SystemTime;

use renderer::{
    BrowserServices, DialCompletion, DialFailure, DialKind, DialOutcome, DialRequest,
    EmbeddedRenderer, MAX_RESPONSE_BODY_BYTES, Mount, TabEvent as RendererEvent,
};
use tinybrowser_cookie::{CookieJar, CookieOp, InitiatorKind, RetrievalKind};
use url::Url;

#[expect(
    unsafe_code,
    clippy::same_length_and_capacity,
    clippy::mem_forget,
    reason = "wit-bindgen maintains the generated component ABI implementation"
)]
mod bindings {
    use super::Component;

    wit_bindgen::generate!({ world: "browser" });
    export!(Component with_types_in self);
}

use bindings::exports::tinybrowser::browser::engine::{Event, FrameEvent, Guest, GuestTab};
use bindings::tinybrowser::browser::types::{FetchError, FetchKind, FetchRequest, FetchResponse};

struct Component;

impl Guest for Component {
    type Tab = Tab;

    fn complete_fetch(id: u64, result: Result<FetchResponse, FetchError>) -> bool {
        let pending = pending_fetches()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
        let Some(pending) = pending else {
            return false;
        };
        if let Ok(response) = &result {
            store_response_cookies(&pending.request, response);
        }
        if let Some(completion) = pending.completion {
            completion(decode_fetch_result(result));
        }
        true
    }
}

struct Tab {
    owner: u64,
    renderer: RefCell<EmbeddedRenderer>,
}

impl GuestTab for Tab {
    fn new(url: String, html: String) -> Result<Self, String> {
        let services = Arc::new(WasmServices::new());
        let owner = services.owner;
        let mut renderer = EmbeddedRenderer::new(services);
        let mount = Mount {
            url,
            content_type: Some("text/html; charset=utf-8".into()),
            content_language: None,
            body: html.into_bytes(),
        };
        renderer.mount(&mount).map_err(|error| error.to_string())?;
        Ok(Self {
            owner,
            renderer: RefCell::new(renderer),
        })
    }

    fn owner(&self) -> u64 {
        self.owner
    }

    fn eval(&self, source: String) -> Result<String, String> {
        self.with_renderer(|renderer| renderer.eval(&source))
    }

    fn pump(&self) -> Option<u64> {
        let mut renderer = self.renderer.borrow_mut();
        renderer.pump_ready();
        renderer.time_until_deadline().map(|delay| {
            let milliseconds = delay.as_nanos().saturating_add(999_999) / 1_000_000;
            u64::try_from(milliseconds).unwrap_or(u64::MAX)
        })
    }

    fn events(&self) -> Result<Vec<FrameEvent>, String> {
        self.renderer
            .borrow_mut()
            .take_events()
            .map(|events| {
                events
                    .into_iter()
                    .map(|(frame, event)| FrameEvent {
                        frame: frame.get(),
                        event: match event {
                            RendererEvent::Load => Event::Load,
                            RendererEvent::ChildLoad => Event::ChildLoad,
                            RendererEvent::Navigated => Event::Navigated,
                            RendererEvent::NavigationFailed => Event::NavigationFailed,
                            RendererEvent::Timer(id) => Event::Timer(id),
                            RendererEvent::Fetch { status } => Event::Fetch(status),
                            RendererEvent::FetchFailed => Event::FetchFailed,
                            RendererEvent::ScriptFailed => Event::ScriptFailed,
                        },
                    })
                    .collect()
            })
            .map_err(|error| error.to_string())
    }

    fn title(&self) -> Result<String, String> {
        self.with_renderer(|renderer| renderer.eval("document.title"))
    }

    fn text(&self) -> Result<String, String> {
        self.with_renderer(|renderer| {
            renderer.eval("document.body === null ? '' : document.body.textContent")
        })
    }
}

impl Tab {
    fn with_renderer(
        &self,
        operation: impl FnOnce(&mut EmbeddedRenderer) -> Result<String, renderer::TabError>,
    ) -> Result<String, String> {
        operation(&mut self.renderer.borrow_mut()).map_err(|error| error.to_string())
    }
}

struct PendingFetch {
    owner: u64,
    /// What cookie bookkeeping needs to know about the originating dial.
    request: RecordedRequest,
    /// Taken exactly once: by its completion, or by tab teardown.
    completion: Option<DialCompletion>,
}

/// The part of a dial the cookie jar keeps until its response arrives.
struct RecordedRequest {
    url: Url,
    initiator: Option<Url>,
    initiator_kind: InitiatorKind,
}

fn pending_fetches() -> &'static Mutex<HashMap<u64, PendingFetch>> {
    static PENDING: OnceLock<Mutex<HashMap<u64, PendingFetch>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Cookie state shared by every tab in one component instance.
fn cookie_jar() -> &'static Mutex<CookieJar> {
    static JAR: OnceLock<Mutex<CookieJar>> = OnceLock::new();
    JAR.get_or_init(|| Mutex::new(CookieJar::default()))
}

/// Stores one response's `Set-Cookie` lines in the shared jar.
fn store_response_cookies(request: &RecordedRequest, response: &FetchResponse) {
    if response.set_cookies.is_empty() {
        return;
    }
    let url = Url::parse(&response.final_url)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https"))
        .unwrap_or_else(|| request.url.clone());
    let mut jar = cookie_jar().lock().unwrap_or_else(PoisonError::into_inner);
    for line in &response.set_cookies {
        jar.store(
            line,
            CookieOp {
                url: &url,
                now: SystemTime::now(),
                kind: RetrievalKind::Http,
                initiator_kind: request.initiator_kind,
                method_is_safe: true,
                initiator: request.initiator.as_ref(),
                cross_site_redirect: false,
            },
        );
    }
}

static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);
static NEXT_FETCH: AtomicU64 = AtomicU64::new(1);

struct WasmServices {
    owner: u64,
}

impl WasmServices {
    fn new() -> Self {
        Self {
            owner: NEXT_OWNER.fetch_add(1, Ordering::Relaxed),
        }
    }
}

impl BrowserServices for WasmServices {
    fn start_dial(&self, request: DialRequest, completion: DialCompletion) {
        let url = Url::parse(&request.url).ok();
        let initiator = Url::parse(&request.initiator).ok();
        let initiator_kind = match request.kind {
            DialKind::JsFetch | DialKind::ClassicScript => InitiatorKind::Fetch,
        };
        let cookie = url
            .as_ref()
            .map(|url| {
                cookie_jar()
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .cookie_string(CookieOp {
                        url,
                        now: SystemTime::now(),
                        kind: RetrievalKind::Http,
                        initiator_kind,
                        method_is_safe: true,
                        initiator: initiator.as_ref(),
                        cross_site_redirect: false,
                    })
            })
            .unwrap_or_default();
        let id = NEXT_FETCH.fetch_add(1, Ordering::Relaxed);
        let host_request = FetchRequest {
            id,
            owner: self.owner,
            kind: match request.kind {
                DialKind::JsFetch => FetchKind::JsFetch,
                DialKind::ClassicScript => FetchKind::ClassicScript,
            },
            url: request.url,
            initiator: request.initiator,
            read_body: request.read_body,
            max_body_bytes: u64::try_from(MAX_RESPONSE_BODY_BYTES).unwrap_or(u64::MAX),
            cookie,
        };
        pending_fetches()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                id,
                PendingFetch {
                    owner: self.owner,
                    request: RecordedRequest {
                        url: url.unwrap_or_else(|| Url::parse("about:blank").expect("valid url")),
                        initiator,
                        initiator_kind,
                    },
                    completion: Some(completion),
                },
            );
        if !bindings::tinybrowser::browser::host::start_fetch(&host_request) {
            let pending = pending_fetches()
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&id);
            if let Some(completion) = pending.and_then(|pending| pending.completion) {
                completion(Err(DialFailure::Connect));
            }
        }
    }

    fn cookies_for(&self, url: &Url) -> String {
        cookie_jar()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .cookie_string(CookieOp {
                url,
                now: SystemTime::now(),
                kind: RetrievalKind::NonHttp,
                initiator_kind: InitiatorKind::Fetch,
                method_is_safe: true,
                initiator: Some(url),
                cross_site_redirect: false,
            })
    }

    fn set_cookie(&self, value: &str, url: &Url) {
        cookie_jar()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .store(
                value,
                CookieOp {
                    url,
                    now: SystemTime::now(),
                    kind: RetrievalKind::NonHttp,
                    initiator_kind: InitiatorKind::Fetch,
                    method_is_safe: true,
                    initiator: Some(url),
                    cross_site_redirect: false,
                },
            );
    }
}

impl Drop for WasmServices {
    fn drop(&mut self) {
        let mut cancelled = Vec::new();
        {
            let mut pending = pending_fetches()
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            pending.retain(|id, fetch| {
                if fetch.owner != self.owner {
                    return true;
                }
                if let Some(completion) = fetch.completion.take() {
                    cancelled.push((*id, completion));
                }
                false
            });
        }
        for (id, completion) in cancelled {
            bindings::tinybrowser::browser::host::cancel_fetch(id);
            completion(Err(DialFailure::Cancelled));
        }
    }
}

fn decode_fetch_result(
    result: Result<FetchResponse, FetchError>,
) -> Result<DialOutcome, DialFailure> {
    let response = result.map_err(decode_fetch_error)?;
    if response.body.len() > MAX_RESPONSE_BODY_BYTES {
        return Err(DialFailure::Limit);
    }
    let final_url = Url::parse(&response.final_url).map_err(|_| DialFailure::Connect)?;
    if !matches!(final_url.scheme(), "http" | "https") {
        return Err(DialFailure::Connect);
    }
    Ok(DialOutcome {
        status: response.status,
        final_url: final_url.into(),
        content_type: response.content_type,
        content_language: response.content_language,
        body: response.body,
    })
}

fn decode_fetch_error(error: FetchError) -> DialFailure {
    match error {
        FetchError::Dns => DialFailure::Dns,
        FetchError::Connect => DialFailure::Connect,
        FetchError::Tls => DialFailure::Tls,
        FetchError::Timeout => DialFailure::Timeout,
        FetchError::Limit => DialFailure::Limit,
        FetchError::QueueFull => DialFailure::QueueFull,
        FetchError::Cancelled => DialFailure::Cancelled,
    }
}
