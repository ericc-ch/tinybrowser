//! Browser-loadable tinybrowser page engine component.
//!
//! This is the browser side of tinybrowser, compiled as a WebAssembly
//! component. It owns pages, cookies, and the redirect policy of a dial; the
//! host owns the network transport and performs exactly one HTTP exchange per
//! `host.start-fetch` call.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::SystemTime;

use cookies::{CookieJar, CookieOp, InitiatorKind, RetrievalKind, schemeful_same_site};
use renderer::{
    BrowsingContextHost, DialCompletion, DialFailure, DialKind, DialOutcome, DialRequest,
    EmbeddedRenderer, MAX_RESPONSE_BODY_BYTES, MessagingHost, Mount, NetworkHost, RendererEvent,
    StorageChange, StorageHost, StorageKind,
};
use url::Url;
use webstorage::StorageArea;

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

/// Redirect hops a single dial may follow.
///
/// Matches the native transport's cap; see
/// <https://fetch.spec.whatwg.org/#http-redirect-fetch>.
const MAX_REDIRECTS: u32 = 20;

/// Statuses the redirect algorithm follows.
///
/// <https://fetch.spec.whatwg.org/#http-redirect-fetch>
const REDIRECT_STATUSES: [u16; 5] = [301, 302, 303, 307, 308];

struct Component;

impl Guest for Component {
    type Tab = Tab;

    fn complete_fetch(id: u64, result: Result<FetchResponse, FetchError>) -> bool {
        let Some(pending) = lock(pending_fetches()).remove(&id) else {
            return false;
        };
        let PendingFetch {
            owner,
            mut chain,
            completion,
        } = pending;
        let Some(completion) = completion else {
            return false;
        };
        match result {
            Err(error) => completion(Err(decode_fetch_error(error))),
            Ok(response) => {
                // A host that cannot see redirects answers with the exchange
                // it ended on, so attribute this response to the URL it
                // reports. Storing it against the requested hop would file one
                // site's cookies under another's name. Store before following:
                // the next hop's header depends on this response.
                let source = reported_url(&chain, &response);
                chain.cross_site_redirect |= !schemeful_same_site(&chain.url, &source);
                store_hop_cookies(&source, &chain, &response);
                // From here the dial stands where this response came from: a
                // relative `Location` resolves against that, not against the
                // hop that was requested before the host moved us.
                chain.url = source;
                match next_hop(&chain, &response) {
                    Ok(Some(next)) => {
                        chain.followed = chain.followed.saturating_add(1);
                        chain.cross_site_redirect |= !schemeful_same_site(&chain.url, &next);
                        chain.url = next;
                        start_hop(owner, chain, completion);
                    }
                    Ok(None) => completion(decode_fetch_result(response, &chain.url)),
                    Err(failure) => completion(Err(failure)),
                }
            }
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

    fn step(&self) -> Option<u64> {
        let mut renderer = self.renderer.borrow_mut();
        renderer.drain_ready();
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
                        event: to_event(event),
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

/// The renderer's event vocabulary, restricted to what this component emits.
fn to_event(event: RendererEvent) -> Event {
    match event {
        RendererEvent::Navigated { url } => Event::Navigated(url),
        RendererEvent::Load => Event::Load,
        RendererEvent::Timer(id) => Event::Timer(id),
        RendererEvent::Fetch { status } => Event::Fetch(status),
        RendererEvent::FetchFailed => Event::FetchFailed,
        RendererEvent::ScriptFailed => Event::ScriptFailed,
    }
}

/// One dial's state, kept across its redirect hops.
struct Chain {
    /// URL of the hop in flight.
    url: Url,
    /// Document URL the dial started from, for `SameSite`.
    initiator: Option<Url>,
    kind: DialKind,
    read_body: bool,
    /// Hops already followed.
    followed: u32,
    /// Whether any hop so far moved to another site.
    cross_site_redirect: bool,
}

struct PendingFetch {
    owner: u64,
    chain: Chain,
    /// Taken exactly once: by the last hop's completion, or by tab teardown.
    completion: Option<DialCompletion>,
}

/// Every exchange in flight, across all tabs of this component instance.
///
/// Entries appear only for dials the renderer asked for, and the renderer caps
/// concurrent dials per document and frames per engine, so a host that honors
/// its completion obligation cannot make this grow without bound.
fn pending_fetches() -> &'static Mutex<HashMap<u64, PendingFetch>> {
    static PENDING: OnceLock<Mutex<HashMap<u64, PendingFetch>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Cookie state shared by every tab in one component instance.
fn cookie_jar() -> &'static Mutex<CookieJar> {
    static JAR: OnceLock<Mutex<CookieJar>> = OnceLock::new();
    JAR.get_or_init(|| Mutex::new(CookieJar::default()))
}

/// The component's local storage areas, keyed by origin. Entries keep
/// insertion order: the storage proxy exposes enumeration and `key(index)`
/// directly, and a `HashMap` would make both unstable.
/// A browser tab has no profile on disk, so the areas die with the component.
fn local_storage() -> &'static Mutex<HashMap<String, StorageArea>> {
    static AREAS: OnceLock<Mutex<HashMap<String, StorageArea>>> = OnceLock::new();
    AREAS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Session storage namespaces keyed by component tab owner.
fn session_storage() -> &'static Mutex<HashMap<u64, HashMap<String, StorageArea>>> {
    static NAMESPACES: OnceLock<Mutex<HashMap<u64, HashMap<String, StorageArea>>>> =
        OnceLock::new();
    NAMESPACES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Locks a shared map or jar, recovering from a panicking holder.
///
/// A poisoned lock means some earlier operation panicked. The jar and the
/// pending map hold no cross-entry invariant a panic can break, so the next
/// caller can continue with the state that was committed.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Sends one hop to the host and records it until its completion arrives.
///
/// HTTP is GET-only here, so a redirect never changes the method or drops a
/// body.
fn start_hop(owner: u64, chain: Chain, completion: DialCompletion) {
    let id = NEXT_FETCH.fetch_add(1, Ordering::Relaxed);
    let request = FetchRequest {
        id,
        owner,
        kind: match chain.kind {
            DialKind::JsFetch => FetchKind::JsFetch,
            DialKind::ClassicScript => FetchKind::ClassicScript,
            DialKind::ModuleScript => FetchKind::ModuleScript,
            DialKind::Stylesheet => FetchKind::Stylesheet,
            DialKind::Image => FetchKind::Image,
            DialKind::FrameLoad => FetchKind::FrameLoad,
        },
        url: chain.url.to_string(),
        initiator: chain
            .initiator
            .as_ref()
            .map(Url::to_string)
            .unwrap_or_default(),
        read_body: chain.read_body,
        max_body_bytes: u64::try_from(MAX_RESPONSE_BODY_BYTES).unwrap_or(u64::MAX),
        cookie: cookie_header(&chain),
    };
    lock(pending_fetches()).insert(
        id,
        PendingFetch {
            owner,
            chain,
            completion: Some(completion),
        },
    );
    if !bindings::tinybrowser::browser::host::start_fetch(&request) {
        let pending = lock(pending_fetches()).remove(&id);
        if let Some(completion) = pending.and_then(|pending| pending.completion) {
            completion(Err(DialFailure::Connect));
        }
    }
}

/// The `Cookie` header for the hop in flight.
fn cookie_header(chain: &Chain) -> String {
    lock(cookie_jar()).cookie_string(CookieOp {
        url: &chain.url,
        now: SystemTime::now(),
        kind: RetrievalKind::Http,
        initiator_kind: InitiatorKind::Fetch,
        method_is_safe: true,
        initiator: chain.initiator.as_ref(),
        cross_site_redirect: chain.cross_site_redirect,
    })
}

/// Where a response came from, as far as the host can say.
///
/// The hop URL unless the host reports a different final URL, which is how a
/// host that followed redirects itself tells us the bytes are not from the
/// hop we asked for.
fn reported_url(chain: &Chain, response: &FetchResponse) -> Url {
    Url::parse(&response.final_url)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https"))
        .unwrap_or_else(|| chain.url.clone())
}

/// Stores one exchange's `Set-Cookie` lines against the URL that sent them.
///
/// <https://httpwg.org/http-extensions/draft-ietf-httpbis-rfc6265bis.html#name-the-set-cookie-header-field>
fn store_hop_cookies(url: &Url, chain: &Chain, response: &FetchResponse) {
    if response.set_cookies.is_empty() {
        return;
    }
    let now = SystemTime::now();
    let mut jar = lock(cookie_jar());
    for line in &response.set_cookies {
        jar.store(
            line,
            CookieOp {
                url,
                now,
                kind: RetrievalKind::Http,
                initiator_kind: InitiatorKind::Fetch,
                method_is_safe: true,
                initiator: chain.initiator.as_ref(),
                cross_site_redirect: chain.cross_site_redirect,
            },
        );
    }
}

/// The redirect this response asks for, if the dial may follow it.
///
/// Mirrors the native transport's `followable_location`, redirect cap, and
/// `resolve_location`; the fragment carry-over is
/// <https://fetch.spec.whatwg.org/#http-redirect-fetch>.
fn next_hop(chain: &Chain, response: &FetchResponse) -> Result<Option<Url>, DialFailure> {
    if !REDIRECT_STATUSES.contains(&response.status) {
        return Ok(None);
    }
    let Some(location) = response.location.as_deref() else {
        return Ok(None);
    };
    if chain.followed >= MAX_REDIRECTS {
        return Err(DialFailure::Limit);
    }
    let mut next = chain.url.join(location).map_err(|_| DialFailure::Connect)?;
    if !matches!(next.scheme(), "http" | "https") {
        return Err(DialFailure::Connect);
    }
    if next.fragment().is_none()
        && let Some(fragment) = chain.url.fragment()
    {
        next.set_fragment(Some(fragment));
    }
    Ok(Some(next))
}

fn decode_fetch_result(response: FetchResponse, hop_url: &Url) -> Result<DialOutcome, DialFailure> {
    if response.body.len() > MAX_RESPONSE_BODY_BYTES {
        return Err(DialFailure::Limit);
    }
    // A host that cannot see redirects reports the browser's final URL here.
    // Fall back to the hop URL rather than failing a dial over metadata.
    let final_url = Url::parse(&response.final_url)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https"))
        .unwrap_or_else(|| hop_url.clone());
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

impl NetworkHost for WasmServices {
    fn start_dial(&self, request: DialRequest, completion: DialCompletion) {
        // A dial the component cannot shape is not a transport question, so it
        // never reaches the host.
        let shaped = Url::parse(&request.url)
            .ok()
            .filter(|url| matches!(url.scheme(), "http" | "https"));
        let Some(url) = shaped else {
            completion(Err(DialFailure::Connect));
            return;
        };
        start_hop(
            self.owner,
            Chain {
                url,
                initiator: Url::parse(&request.initiator).ok(),
                kind: request.kind,
                read_body: request.read_body,
                followed: 0,
                cross_site_redirect: false,
            },
            completion,
        );
    }

    fn cookies_for(&self, url: &Url) -> String {
        lock(cookie_jar()).cookie_string(CookieOp {
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
        lock(cookie_jar()).store(
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

impl StorageHost for WasmServices {
    fn storage_get(&self, kind: StorageKind, origin: &str, key: &str) -> Option<String> {
        match kind {
            StorageKind::Local => lock(local_storage())
                .get(origin)
                .and_then(|area| area.get(key)),
            StorageKind::Session => lock(session_storage())
                .get(&self.owner)
                .and_then(|namespace| namespace.get(origin))
                .and_then(|area| area.get(key)),
        }
    }

    fn storage_keys(&self, kind: StorageKind, origin: &str) -> Vec<String> {
        match kind {
            StorageKind::Local => lock(local_storage())
                .get(origin)
                .map(StorageArea::keys)
                .unwrap_or_default(),
            StorageKind::Session => lock(session_storage())
                .get(&self.owner)
                .and_then(|namespace| namespace.get(origin))
                .map(StorageArea::keys)
                .unwrap_or_default(),
        }
    }

    fn storage_set(
        &self,
        kind: StorageKind,
        origin: &str,
        _url: &str,
        key: &str,
        value: &str,
        _source: renderer::FrameId,
    ) -> Result<Option<StorageChange>, renderer::StorageError> {
        match kind {
            StorageKind::Local => lock(local_storage())
                .entry(origin.to_owned())
                .or_default()
                .set(key, value),
            StorageKind::Session => lock(session_storage())
                .entry(self.owner)
                .or_default()
                .entry(origin.to_owned())
                .or_default()
                .set(key, value),
        }
    }

    fn storage_remove(
        &self,
        kind: StorageKind,
        origin: &str,
        _url: &str,
        key: &str,
        _source: renderer::FrameId,
    ) -> Option<StorageChange> {
        match kind {
            StorageKind::Local => lock(local_storage()).get_mut(origin)?.remove(key),
            StorageKind::Session => lock(session_storage())
                .get_mut(&self.owner)?
                .get_mut(origin)?
                .remove(key),
        }
    }

    fn storage_clear(
        &self,
        kind: StorageKind,
        origin: &str,
        _url: &str,
        _source: renderer::FrameId,
    ) -> Option<StorageChange> {
        match kind {
            StorageKind::Local => lock(local_storage()).get_mut(origin)?.clear(),
            StorageKind::Session => lock(session_storage())
                .get_mut(&self.owner)?
                .get_mut(origin)?
                .clear(),
        }
    }
}

impl BrowsingContextHost for WasmServices {
    fn window_open(&self, _url: &str, _name: &str, _features: &str) -> Option<u64> {
        None
    }

    fn window_close(&self, _tab: u64) {}

    fn window_opener(&self) -> Option<u64> {
        None
    }

    fn window_post_message(&self, _tab: u64, _payload: &str) {}

    fn remote_session_get(&self, _tab: u64, _origin: &str, _key: &str) -> Option<String> {
        None
    }
}

impl MessagingHost for WasmServices {
    fn broadcast_post(&self, _origin: &str, _name: &str, _payload: &str, _channel: u64) {}
}

impl Drop for WasmServices {
    fn drop(&mut self) {
        let mut cancelled = Vec::new();
        lock(pending_fetches()).retain(|id, fetch| {
            if fetch.owner != self.owner {
                return true;
            }
            if let Some(completion) = fetch.completion.take() {
                cancelled.push((*id, completion));
            }
            false
        });
        for (id, completion) in cancelled {
            bindings::tinybrowser::browser::host::cancel_fetch(id);
            completion(Err(DialFailure::Cancelled));
        }
        lock(session_storage()).remove(&self.owner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_failures_keep_their_kind() {
        for (error, expected) in [
            (FetchError::Dns, DialFailure::Dns),
            (FetchError::Connect, DialFailure::Connect),
            (FetchError::Tls, DialFailure::Tls),
            (FetchError::Timeout, DialFailure::Timeout),
            (FetchError::Limit, DialFailure::Limit),
            (FetchError::QueueFull, DialFailure::QueueFull),
            (FetchError::Cancelled, DialFailure::Cancelled),
        ] {
            assert_eq!(decode_fetch_error(error), expected);
        }
    }

    #[test]
    fn a_response_without_a_usable_final_url_keeps_the_hop_url() {
        let hop = Url::parse("https://example.test/docs").expect("valid url");
        let response = FetchResponse {
            status: 200,
            final_url: String::new(),
            content_type: None,
            content_language: None,
            set_cookies: Vec::new(),
            location: None,
            body: b"hi".to_vec(),
        };
        let outcome = decode_fetch_result(response, &hop).expect("decoded");
        assert_eq!(outcome.final_url, "https://example.test/docs");
    }
}
