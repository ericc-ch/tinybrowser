//! Browser-owned live networking: one [`net::Agent`] behind a value-only handle.
//!
//! Tab coordinators receive [`TabNetworkHandle`]. They do not expose or own
//! [`net::Agent`].

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use net::{
    Agent, AgentOptions, InitiatorKind, Method, Request, UserAgent, isomorphic_decode,
    isomorphic_encode,
};
use renderer::{
    BodyDelivery, DialFailure, DialKind, DialOutcome, DialRequest, RequestCache, ResponseBody,
    StreamId,
};
use tokio::sync::Semaphore;
use tokio::sync::mpsc::UnboundedSender;
use url::Url;

use crate::wire::RendererAssignmentId;

/// Default per-call fetch timeout on a [`TabNetworkHandle`].
pub(crate) const PAGE_FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// In-flight keepalive request bytes across every realm in this process
/// (<https://fetch.spec.whatwg.org/#keepalive-flag>).
const KEEPALIVE_QUOTA: usize = 65_536;
static KEEPALIVE_IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

struct KeepaliveReservation(usize);

impl KeepaliveReservation {
    fn acquire(request: &DialRequest) -> Result<Option<Self>, DialFailure> {
        if !request.keepalive {
            return Ok(None);
        }
        let Ok(length) = usize::try_from(request.body_len) else {
            return Err(DialFailure::Limit);
        };
        if length > KEEPALIVE_QUOTA {
            return Err(DialFailure::Limit);
        }
        let mut current = KEEPALIVE_IN_FLIGHT.load(Ordering::Relaxed);
        loop {
            let Some(next) = current.checked_add(length) else {
                return Err(DialFailure::Limit);
            };
            if next > KEEPALIVE_QUOTA {
                return Err(DialFailure::Limit);
            }
            match KEEPALIVE_IN_FLIGHT.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Ok(Some(Self(length))),
                Err(observed) => current = observed,
            }
        }
    }
}

impl Drop for KeepaliveReservation {
    fn drop(&mut self) {
        KEEPALIVE_IN_FLIGHT.fetch_sub(self.0, Ordering::AcqRel);
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct CacheKey {
    method: net::Method,
    /// The request URL's canonical serialization. The dial parses it into a
    /// `Url` on the way out; a key built before that would only turn the same
    /// parse failure into a second, hidden one.
    url: String,
    /// The request's credentials mode. A response fetched with `include` is
    /// not the response an `omit` request may reuse.
    credentials: net::cors::CredentialsMode,
    initiator: url::Origin,
    partition: RendererAssignmentId,
}

/// A whole buffered response. The fetch body is already bounded by
/// `MAX_RESPONSE_BODY_BYTES`, so the cache holds at most that much per entry.
#[derive(Clone)]
struct CachedResponse {
    outcome: DialOutcome,
    stored_at: Instant,
    request_headers: Vec<(String, String)>,
}

type ResponseCache = Arc<Mutex<HashMap<CacheKey, CachedResponse>>>;

/// The HTTP cache decision for one dial.
enum CacheDecision {
    /// The stored outcome answers the request without a network dial.
    Hit(DialOutcome),
    /// The dial goes out. `stored` is what a 304 may merge into, and `etag` is
    /// the validator it may revalidate with.
    Miss {
        mode: RequestCache,
        stored: Option<CachedResponse>,
        etag: Option<String>,
    },
}

/// The answer from one dial once the response head is in.
enum DialProgress {
    /// The dial has its answer: a refused redirect or a 304 merged into the
    /// stored entry.
    Done(Box<DialOutcome>),
    /// The head is in and the body is still on the wire.
    Response {
        response: Box<net::Response>,
        permits: DialPermits,
    },
}

/// A response whose head is in and whose body is still on the wire, plus the
/// view its consumer reports.
struct PendingBody {
    response: net::Response,
    permits: DialPermits,
    visibility: ResponseVisibility,
}

/// Which response a dial reports.
///
/// A page `fetch`/XHR hands its response to script, so it gets the *filtered*
/// response: an opaque one has no status, URL, headers, or body
/// (<https://fetch.spec.whatwg.org/#concept-filtered-response>). Every other
/// dial's response is consumed by the engine itself — an image decoder, a
/// script compiler, a style sheet parser — and those algorithms read the
/// *unsafe* response, which keeps what the server sent
/// (<https://fetch.spec.whatwg.org/#concept-internal-response>).
#[derive(Clone, Copy, PartialEq, Eq)]
enum ResponseVisibility {
    /// Handed to page script.
    Page,
    /// Consumed by the engine.
    Internal,
}

/// Everything a dial outcome reports except its body. Each delivery path
/// fills in the body it actually has, so a streamed outcome is never built
/// from an empty buffered one
/// (<https://fetch.spec.whatwg.org/#concept-response>).
struct DialHead {
    status: u16,
    status_text: String,
    final_url: String,
    content_type: Option<String>,
    /// The response's header list, minus `Set-Cookie`: cookies live in the
    /// browser's jar, and an `HttpOnly` one must not reach the renderer.
    headers: Vec<(String, String)>,
    response_type: net::cors::ResponseType,
    crossed_origin: bool,
    redirect_count: u32,
    referrer: Option<String>,
}

impl DialHead {
    /// Reads one response head, from the view `visibility` names.
    ///
    /// Both dial paths and a document navigation go through this: one place
    /// decides which headers, status, and URL a response reports.
    fn read(response: &net::Response, visibility: ResponseVisibility) -> Self {
        let response_type = response.response_type();
        let page = visibility == ResponseVisibility::Page;
        // A page-visible response is always the filtered one: a `cors`
        // response exposes only its safelisted and exposed fields, and an
        // opaque one exposes nothing. An internal consumer reads the unsafe
        // response's list.
        let visible = if page {
            std::borrow::Cow::Owned(response.exposed_headers())
        } else {
            std::borrow::Cow::Borrowed(response.internal_headers())
        };
        let headers: Vec<(String, String)> = visible
            .iter()
            .filter(|(name, _)| !net::cors::is_forbidden_response_header(name))
            .map(|(name, value)| (name.to_owned(), isomorphic_decode(value)))
            .collect();
        // An opaque filtered response has status 0, an empty status message,
        // an empty header list, a null body, and a URL list of either nothing
        // (`opaque`) or the request URL (`opaqueredirect`). It must not reveal
        // where the request landed
        // (<https://fetch.spec.whatwg.org/#concept-filtered-response-opaque>,
        // <https://fetch.spec.whatwg.org/#concept-filtered-response-opaque-redirect>).
        let opaque = page && response_type.is_opaque();
        let hide_url = page && response_type == net::cors::ResponseType::Opaque;
        Self {
            status: if opaque { 0 } else { response.status() },
            status_text: if opaque {
                String::new()
            } else {
                response.status_text().to_owned()
            },
            final_url: if hide_url {
                String::new()
            } else {
                response.final_url().to_string()
            },
            content_type: content_type_header(&visible),
            headers,
            response_type,
            crossed_origin: response.crossed_origin(),
            // `Response.redirected` is "the URL list has more than one entry",
            // and an opaque response's list holds one
            // (<https://fetch.spec.whatwg.org/#dom-response-redirected>).
            redirect_count: if opaque { 0 } else { response.redirect_count() },
            referrer: response.referrer().map(ToString::to_string),
        }
    }

    /// The document head a navigation mounts, from the same fields a fetch
    /// outcome reports.
    ///
    /// The top-level and child-frame paths cannot disagree about how a header
    /// becomes policy, endpoints, or the nosniff flag: both read the header
    /// list here and hand it to one derivation
    /// (<https://html.spec.whatwg.org/multipage/document-lifecycle.html#initialise-the-document-object>).
    fn into_document_head(self) -> renderer::ResponseHead {
        renderer::ResponseHead::http(
            self.status,
            self.final_url,
            self.content_type,
            &self.headers,
        )
    }

    /// The outcome this head answers with, once its body is known.
    fn into_outcome(self, body: ResponseBody) -> DialOutcome {
        DialOutcome {
            status: self.status,
            status_text: self.status_text,
            final_url: self.final_url,
            content_type: self.content_type,
            headers: self.headers,
            body,
            response_type: self.response_type,
            crossed_origin: self.crossed_origin,
            redirect_count: self.redirect_count,
            referrer: self.referrer,
        }
    }
}

/// The browser-wide and per-tab permits one dial holds until its body is done.
struct DialPermits {
    _global: tokio::sync::OwnedSemaphorePermit,
    _tab: tokio::sync::OwnedSemaphorePermit,
}

const CACHE_MAX_ENTRIES: usize = 64;
const CACHE_MAX_BYTES: usize = 64 * 1024 * 1024;

const DEFAULT_USER_AGENT: UserAgent = UserAgent::from_static(crate::USER_AGENT);

/// The bytes a cache entry holds. Only buffered bodies are ever stored: a
/// streamed response is buffered into the store as it arrives, and an
/// undecodable one has nothing to reuse.
fn stored_bytes(outcome: &DialOutcome) -> usize {
    match &outcome.body {
        ResponseBody::Bytes(bytes) => bytes.len(),
        ResponseBody::Undecodable | ResponseBody::Streamed(_) => 0,
    }
}

fn insert_cached(
    cache: &mut HashMap<CacheKey, CachedResponse>,
    key: CacheKey,
    entry: CachedResponse,
) {
    let body_len = stored_bytes(&entry.outcome);
    if body_len > renderer::MAX_RESPONSE_BODY_BYTES {
        return;
    }
    let mut total = cache
        .values()
        .map(|entry| stored_bytes(&entry.outcome))
        .sum::<usize>();
    while cache.len() >= CACHE_MAX_ENTRIES || total.saturating_add(body_len) > CACHE_MAX_BYTES {
        let Some(oldest) = cache
            .iter()
            .min_by_key(|(_, entry)| entry.stored_at)
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        if let Some(removed) = cache.remove(&oldest) {
            total = total.saturating_sub(stored_bytes(&removed.outcome));
        }
    }
    cache.insert(key, entry);
}

enum StreamMessage {
    Chunk(Vec<u8>),
    End,
    Error(DialFailure),
}

/// Chunks a body task may buffer ahead of the renderer's next read.
const STREAM_CHUNK_DEPTH: usize = 16;

struct StreamEntry {
    owner: RendererAssignmentId,
    rx: Arc<tokio::sync::Mutex<tokio::sync::mpsc::Receiver<StreamMessage>>>,
    task: tokio::task::JoinHandle<()>,
}

type StreamMap = Arc<Mutex<HashMap<StreamId, StreamEntry>>>;

/// Aborts the body task of a stream removed from the table, so the task stops
/// reading and releases its dial permits.
fn abort_stream(entry: Option<StreamEntry>) {
    if let Some(entry) = entry {
        entry.task.abort();
    }
}

fn cache_key(request: &DialRequest, initiator: &Url, partition: RendererAssignmentId) -> CacheKey {
    CacheKey {
        method: request.method.clone(),
        url: request.url.clone(),
        credentials: request.policy.credentials(),
        initiator: initiator.origin(),
        partition,
    }
}

fn has_page_conditional(request: &DialRequest) -> bool {
    request.headers.iter().any(|(name, _)| {
        name.eq_ignore_ascii_case("if-none-match")
            || name.eq_ignore_ascii_case("if-modified-since")
            || name.eq_ignore_ascii_case("if-match")
            || name.eq_ignore_ascii_case("if-unmodified-since")
            || name.eq_ignore_ascii_case("if-range")
    })
}

fn response_forbids_storage(outcome: &DialOutcome) -> bool {
    outcome.headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("cache-control")
            && value.to_ascii_lowercase().contains("no-store")
    })
}

fn vary_matches(entry: &CachedResponse, request: &DialRequest) -> bool {
    let vary = net::get_decode_split_decoded(
        entry
            .outcome
            .headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
        "vary",
    );
    let Some(vary) = vary else {
        return true;
    };
    vary.iter().filter(|field| !field.is_empty()).all(|field| {
        if field == "*" {
            return false;
        }
        let stored = combined_request_header(&entry.request_headers, field);
        stored == combined_request_header(&request.headers, field)
    })
}

/// Every value of `name` in a decoded header list joined with `", "`, the way
/// getting a header does
/// (<https://fetch.spec.whatwg.org/#concept-header-list-get>).
fn combined_request_header(headers: &[(String, String)], name: &str) -> Option<String> {
    net::combined_header(
        headers
            .iter()
            .filter(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str()),
    )
}

fn fresh(entry: &CachedResponse) -> bool {
    let age = entry.stored_at.elapsed().as_secs();
    let mut saw_max_age = false;
    let cache_control = net::get_decode_split_decoded(
        entry
            .outcome
            .headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
        "cache-control",
    );
    for directive in cache_control.iter().flatten() {
        let Some((name, value)) = directive.split_once('=') else {
            continue;
        };
        if name
            .trim_matches([' ', '\t'])
            .eq_ignore_ascii_case("max-age")
        {
            saw_max_age = true;
            // Freshness lasts for `max-age` seconds from the store, so an
            // entry stored at second 0 is stale once the whole second has
            // passed: at `age == max_age` the lifetime is spent
            // (<https://www.rfc-editor.org/rfc/rfc9111#section-4.2>).
            if value
                .trim_matches([' ', '\t'])
                .parse::<u64>()
                .is_ok_and(|max_age| age < max_age)
            {
                return true;
            }
        }
    }
    if saw_max_age {
        return false;
    }
    entry.outcome.headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("expires")
            && httpdate::parse_http_date(value)
                .is_ok_and(|expires| expires > std::time::SystemTime::now())
    })
}

fn etag(outcome: &DialOutcome) -> Option<String> {
    outcome
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("etag"))
        .map(|(_, value)| value.clone())
}

/// Reuses the stored body for a validated 304 and applies the new headers.
///
/// Only a page `fetch` stores a response, so the merged header list is the one
/// the page sees; the internal list would carry `Set-Cookie` back into it.
fn merge_not_modified(cached: &DialOutcome, response: &net::Response) -> DialOutcome {
    let mut merged = cached.clone();
    merged.status = 200;
    merged.final_url = response.final_url().to_string();
    merged.referrer = response.referrer().map(ToString::to_string);
    merged.crossed_origin = response.crossed_origin();
    let exposed = response.exposed_headers();
    for (name, value) in exposed.iter() {
        let value = isomorphic_decode(value);
        if let Some((_, existing)) = merged
            .headers
            .iter_mut()
            .find(|(existing, _)| existing.eq_ignore_ascii_case(name))
        {
            *existing = value;
        } else {
            merged.headers.push((name.to_owned(), value));
        }
    }
    merged
}

fn should_store(request: &DialRequest, outcome: &DialOutcome, mode: RequestCache) -> bool {
    request.policy.exposes_response()
        && !has_page_conditional(request)
        && request.method == net::Method::GET
        // A body that failed to decode has nothing to reuse. Storing it would
        // replay the failure on every hit and every 304 merge, and a
        // transient truncation would become permanent
        // (<https://fetch.spec.whatwg.org/#concept-http-network-fetch>).
        && !matches!(outcome.body, ResponseBody::Undecodable)
        && (200..300).contains(&outcome.status)
        // An opaque response has no readable body and cannot satisfy a later
        // CORS request, so keeping it only creates a response-type mismatch.
        && !outcome.response_type.is_opaque()
        && mode != RequestCache::NoStore
        && !response_forbids_storage(outcome)
}

/// Stores one complete response for `key`, evicting older entries as needed.
fn store_response(
    cache: &ResponseCache,
    key: CacheKey,
    outcome: DialOutcome,
    request_headers: Vec<(String, String)>,
) {
    let mut cache = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    insert_cached(
        &mut cache,
        key,
        CachedResponse {
            outcome,
            stored_at: Instant::now(),
            request_headers,
        },
    );
}

/// Rechecks the request-mode rules that produced a cached filtered response.
/// The cache partition keeps other origins away, but one document can still
/// switch modes between requests.
fn cache_usable(request: &DialRequest, initiator: &Url, entry: &CachedResponse) -> bool {
    if entry.outcome.response_type.is_opaque() {
        return false;
    }
    let Ok(url) = Url::parse(&request.url) else {
        return false;
    };
    match request.policy.mode() {
        net::cors::RequestMode::SameOrigin => url.origin() == initiator.origin(),
        net::cors::RequestMode::NoCors => {
            !matches!(entry.outcome.response_type, net::cors::ResponseType::Cors)
        }
        net::cors::RequestMode::Cors => true,
        // Only a page `fetch` reads the cache, and its mode comes from the
        // `Request` constructor, which rejects `navigate`
        // (<https://fetch.spec.whatwg.org/#dom-request>).
        net::cors::RequestMode::Navigate => false,
    }
}

/// One completed navigation dial: the response head the tab mounts plus the
/// streaming body.
pub(crate) struct NavOutcome {
    pub head: renderer::ResponseHead,
    pub body: net::Body,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

/// Live network state for one storage partition.
pub(crate) struct NetworkContext {
    agent: Agent,
    permits: NetworkPermits,
    cache: ResponseCache,
    streams: StreamMap,
    next_stream: Arc<AtomicU64>,
}

impl NetworkContext {
    /// Builds the partition network from caller `options`. Defaults fill only
    /// where the caller left a field `None`: caller wins on `user_agent` and
    /// `timeout_per_call`.
    pub(crate) fn new(mut options: AgentOptions) -> Self {
        if options.user_agent.is_none() {
            options.user_agent = Some(DEFAULT_USER_AGENT);
        }
        if options.timeout_per_call.is_none() {
            options.timeout_per_call = Some(PAGE_FETCH_TIMEOUT);
        }
        Self {
            agent: Agent::new(options),
            permits: NetworkPermits::new(),
            cache: Arc::new(Mutex::new(HashMap::new())),
            streams: Arc::new(Mutex::new(HashMap::new())),
            next_stream: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Network capability for one tab, with its own per-tab cap.
    #[must_use]
    pub(crate) fn tab_handle(&self) -> TabNetworkHandle {
        TabNetworkHandle {
            agent: self.agent.clone(),
            permits: self.permits.clone(),
            tab: Arc::new(Semaphore::new(MAX_TAB_DIALS)),
            cache: Arc::clone(&self.cache),
            streams: Arc::clone(&self.streams),
            next_stream: Arc::clone(&self.next_stream),
            sheets: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub(crate) fn agent(&self) -> Agent {
        self.agent.clone()
    }

    /// Cookies visible to `url`, including session and `HttpOnly` cookies.
    pub(crate) fn cookie_records(&self, url: &Url) -> Vec<net::CookieRecord> {
        self.agent.cookie_records(url)
    }

    /// Drops every cookie from the live jar.
    pub(crate) fn clear_cookies(&self) {
        self.agent.clear_cookies();
    }

    /// Stores one `Set-Cookie` line for `url` with HTTP-level rules,
    /// returning whether it was stored.
    pub(crate) fn add_cookie(&self, cookie: &str, url: &Url) -> bool {
        self.agent.store_cookie_http(cookie, url)
    }
}

/// Limits for in-flight dials: browser-wide and per reserved class.
#[derive(Clone)]
struct NetworkPermits {
    global: Arc<Semaphore>,
    navigation: Arc<Semaphore>,
}

impl NetworkPermits {
    fn new() -> Self {
        Self {
            global: Arc::new(Semaphore::new(MAX_GLOBAL_DIALS)),
            navigation: Arc::new(Semaphore::new(MAX_NAVIGATION_DIALS)),
        }
    }
}

const MAX_GLOBAL_DIALS: usize = 32;
const MAX_NAVIGATION_DIALS: usize = 8;
const MAX_TAB_DIALS: usize = 8;

/// Cloneable network capability for one tab.
///
/// Completions return to the tab coordinator as renderer events.
#[derive(Clone)]
pub(crate) struct TabNetworkHandle {
    agent: Agent,
    permits: NetworkPermits,
    tab: Arc<Semaphore>,
    cache: ResponseCache,
    streams: StreamMap,
    next_stream: Arc<AtomicU64>,
    /// Style sheet URLs this tab's network served: final URL and request URL,
    /// fragments stripped. A request a sheet initiates sends the sheet's URL
    /// as its referrer, which is cross-origin to both the document and the
    /// target, so the browser authorizes an explicit referrer against this set
    /// instead of the document's origin. Only URLs that really were style
    /// sheets for this tab can be forged into a referrer, so a renderer cannot
    /// claim an arbitrary origin's URL
    /// (<https://w3c.github.io/webappsec-referrer-policy/#integration-with-css>).
    sheets: Arc<Mutex<HashSet<Url>>>,
}

impl TabNetworkHandle {
    /// `document.cookie` getter for `url`.
    #[must_use]
    pub(crate) fn cookies_for(&self, url: &Url) -> String {
        self.agent.cookies_for(url)
    }

    /// `document.cookie` setter for `url`.
    pub(crate) fn set_cookie(&self, value: &str, url: &Url) {
        self.agent.set_cookie(value, url);
    }

    /// Spawns one navigation dial and returns its task handle. The caller
    /// aborts the handle to cancel the request. The completion returns on
    /// `reply` tagged with `epoch`.
    pub(crate) fn dial_navigation(
        &self,
        epoch: u64,
        url: Url,
        initiator: Url,
        reply: UnboundedSender<(u64, Result<NavOutcome, DialFailure>)>,
        mut cancel: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        let network = self.clone();
        tokio::spawn(async move {
            let result = tokio::select! {
                biased;
                _ = cancel.changed() => Err(DialFailure::Cancelled),
                result = network.navigate(&url, &initiator) => result,
            };
            let _send_result = reply.send((epoch, result));
        })
    }

    async fn navigate(&self, url: &Url, initiator: &Url) -> Result<NavOutcome, DialFailure> {
        // Browser-seam timeline for `profile` in tools/wpt: a stalled test is
        // read from these records. The macro tests the level before it formats
        // anything, so an off level costs one check and the clock read below.
        let started = Instant::now();
        logging::trace!(target: "browser::net", "nav start {url}");
        let deadline = started + PAGE_FETCH_TIMEOUT;
        let permits = Self::acquire(&self.permits.navigation, deadline).await?;
        let mut request = Request::new(Method::GET, url.clone());
        chrome_navigation_request(&mut request, url);
        request.initiator_kind = InitiatorKind::Navigation;
        request.initiator = Some(initiator.clone());
        request.cors = net::cors::CorsSettings {
            mode: net::cors::RequestMode::Navigate,
            credentials: net::cors::CredentialsMode::Include,
            redirect: net::cors::RedirectMode::Follow,
            origin: initiator.origin(),
        };
        request.deadline = Some(deadline);
        let response = match self.agent.send(request).await {
            Ok(response) => response,
            Err(error) => {
                logging::trace!(
                    target: "browser::net",
                    "nav failed {url} in {:?}: {error:?}",
                    started.elapsed()
                );
                return Err(dial_failure(&error));
            }
        };
        let status = response.status();
        let final_url = response.final_url().clone();
        logging::trace!(
            target: "browser::net",
            "nav head {url} -> {status} {final_url} in {:?}",
            started.elapsed()
        );
        // One head reader for both dial paths, so the top-level document and a
        // fetch outcome cannot disagree about which header becomes policy,
        // endpoints, or the nosniff flag
        // (<https://html.spec.whatwg.org/multipage/document-lifecycle.html#initialise-the-document-object>).
        let head = DialHead::read(&response, ResponseVisibility::Internal).into_document_head();
        let body = response.into_body().await;
        Ok(NavOutcome {
            head,
            body,
            _permit: permits,
        })
    }

    /// Async HTTP request for one renderer service call.
    pub(crate) async fn dial_request(
        &self,
        request: &DialRequest,
        initiator: &Url,
        owner: RendererAssignmentId,
    ) -> Result<DialOutcome, DialFailure> {
        let started = Instant::now();
        let _keepalive = KeepaliveReservation::acquire(request)?;
        let (mode, stored, etag) = match self.cache_read(request, initiator, owner)? {
            CacheDecision::Hit(hit) => {
                trace_dial_result(request, started, Ok(&hit));
                return Ok(hit);
            }
            CacheDecision::Miss { mode, stored, etag } => (mode, stored, etag),
        };
        logging::trace!(
            target: "browser::net",
            "request start {:?} {}",
            request.policy.kind(),
            request.url
        );
        let visibility = if request.policy.exposes_response() {
            ResponseVisibility::Page
        } else {
            ResponseVisibility::Internal
        };
        let result = async {
            match self
                .send_head(request, initiator, stored.as_ref(), etag.as_deref())
                .await?
            {
                DialProgress::Done(outcome) => {
                    self.remember_response(request, initiator, owner, mode, &outcome);
                    Ok(*outcome)
                }
                DialProgress::Response { response, permits } => match request.delivery {
                    BodyDelivery::Buffer => {
                        let _permits = permits;
                        let head = DialHead::read(&response, visibility);
                        // A page-visible opaque response has a null body
                        // (<https://fetch.spec.whatwg.org/#concept-filtered-response-opaque>).
                        // An internal consumer still reads the bytes.
                        let body = if visibility == ResponseVisibility::Page
                            && head.response_type.is_opaque()
                        {
                            ResponseBody::Bytes(Vec::new())
                        } else {
                            let mut reader =
                                BodyReader::new(*response, request.integrity.clone(), true).await;
                            loop {
                                match reader.next().await {
                                    BodyEvent::Chunk(_) => {}
                                    BodyEvent::Done(body) => {
                                        break ResponseBody::Bytes(body);
                                    }
                                    BodyEvent::DecodeError => {
                                        break ResponseBody::Undecodable;
                                    }
                                    BodyEvent::Failed(failure) => return Err(failure),
                                }
                            }
                        };
                        let outcome = head.into_outcome(body);
                        self.remember_response(request, initiator, owner, mode, &outcome);
                        Ok(outcome)
                    }
                    BodyDelivery::Stream => Ok(self.start_stream(
                        request,
                        initiator,
                        owner,
                        mode,
                        PendingBody {
                            response: *response,
                            permits,
                            visibility,
                        },
                    )),
                },
            }
        }
        .await;
        if let Ok(outcome) = &result {
            self.remember_sheet(request, outcome.status, &outcome.final_url);
        }
        trace_dial_result(request, started, result.as_ref());
        result
    }

    /// Sends the request and reads the response head.
    async fn send_head(
        &self,
        request: &DialRequest,
        initiator: &Url,
        stored: Option<&CachedResponse>,
        etag: Option<&str>,
    ) -> Result<DialProgress, DialFailure> {
        let deadline = Instant::now() + Self::dial_budget(request);
        let global = Self::acquire(&self.permits.global, deadline).await?;
        let tab = Self::acquire(&self.tab, deadline).await?;
        let mut outbound = self.build_outbound(request, initiator, deadline)?;
        if let Some(etag) = etag.and_then(net::isomorphic_encode) {
            // A validator that cannot be written back as bytes is one the
            // server sent outside the isomorphic range; skipping the
            // conditional request is the only honest answer, since the stored
            // entry still answers a 200.
            outbound
                .headers
                .insert("If-None-Match", etag)
                .map_err(|_| DialFailure::Connect)?;
        }
        let blocked_redirect = watch_redirects(request, initiator, &mut outbound);
        let response = match self.agent.send(outbound).await {
            Ok(response) => response,
            Err(error) => {
                let done =
                    refused_redirect(&blocked_redirect).ok_or_else(|| dial_failure(&error))?;
                return Ok(DialProgress::Done(Box::new(done)));
            }
        };
        if request.integrity.is_some() && response.response_type().is_opaque() {
            return Err(DialFailure::Connect);
        }
        if response.status() == 304
            && let Some(stored) = stored
        {
            return Ok(DialProgress::Done(Box::new(merge_not_modified(
                &stored.outcome,
                &response,
            ))));
        }
        Ok(DialProgress::Response {
            response: Box::new(response),
            permits: DialPermits {
                _global: global,
                _tab: tab,
            },
        })
    }

    /// Registers the body stream with a task that owns the permits and the
    /// cache write for its bytes.
    fn start_stream(
        &self,
        request: &DialRequest,
        initiator: &Url,
        owner: RendererAssignmentId,
        mode: RequestCache,
        pending: PendingBody,
    ) -> DialOutcome {
        let PendingBody {
            response,
            permits,
            visibility,
        } = pending;
        let head = DialHead::read(&response, visibility);
        // A page-visible opaque response has a null body
        // (<https://fetch.spec.whatwg.org/#concept-filtered-response-opaque>).
        if visibility == ResponseVisibility::Page && head.response_type.is_opaque() {
            return head.into_outcome(ResponseBody::Bytes(Vec::new()));
        }
        let mut outcome = head.into_outcome(ResponseBody::Bytes(Vec::new()));
        let (tx, rx) = tokio::sync::mpsc::channel(STREAM_CHUNK_DEPTH);
        let store = should_store(request, &outcome, mode).then(|| PendingStore {
            outcome: outcome.clone(),
            cache: Arc::clone(&self.cache),
            key: cache_key(request, initiator, owner),
            request_headers: request.headers.clone(),
        });
        let task = spawn_stream_body(StreamJob {
            response,
            integrity: request.integrity.clone(),
            store,
            permits,
            kind: request.policy.kind(),
            url: request.url.clone(),
            tx,
        });
        let id = StreamId::mint(self.next_stream.fetch_add(1, Ordering::Relaxed));
        self.streams
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                id,
                StreamEntry {
                    owner,
                    rx: Arc::new(tokio::sync::Mutex::new(rx)),
                    task,
                },
            );
        outcome.body = ResponseBody::Streamed(id);
        outcome
    }

    pub(crate) async fn read_stream(
        &self,
        owner: RendererAssignmentId,
        id: StreamId,
    ) -> Result<Option<Vec<u8>>, DialFailure> {
        let rx = self
            .streams
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&id)
            .filter(|entry| entry.owner == owner)
            .map(|entry| Arc::clone(&entry.rx));
        let Some(rx) = rx else {
            return Ok(None);
        };
        let mut rx = rx.lock().await;
        match rx.recv().await {
            Some(StreamMessage::Chunk(bytes)) => Ok(Some(bytes)),
            Some(StreamMessage::End) | None => {
                self.remove_stream(id);
                Ok(None)
            }
            Some(StreamMessage::Error(failure)) => {
                self.remove_stream(id);
                Err(failure)
            }
        }
    }

    pub(crate) fn cancel_stream(&self, owner: RendererAssignmentId, id: StreamId) {
        use std::collections::hash_map::Entry;
        let mut streams = self
            .streams
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Entry::Occupied(entry) = streams.entry(id)
            && entry.get().owner == owner
        {
            abort_stream(Some(entry.remove()));
        }
    }

    fn remove_stream(&self, id: StreamId) {
        let mut streams = self
            .streams
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        abort_stream(streams.remove(&id));
    }

    /// Aborts every stream an assignment owns, so a released renderer leaves
    /// no body task reading into memory for nobody.
    pub(crate) fn release_streams(&self, owner: RendererAssignmentId) {
        let mut streams = self
            .streams
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let abandoned: Vec<StreamId> = streams
            .iter()
            .filter(|(_, entry)| entry.owner == owner)
            .map(|(id, _)| *id)
            .collect();
        for id in abandoned {
            abort_stream(streams.remove(&id));
        }
    }

    fn cache_read(
        &self,
        request: &DialRequest,
        initiator: &Url,
        owner: RendererAssignmentId,
    ) -> Result<CacheDecision, DialFailure> {
        let mode = request.cache;
        let cached = if has_page_conditional(request) {
            None
        } else {
            self.cached_hit(request, initiator, owner)
        };
        if let Some(hit) = Self::cached_response(request, mode, cached.as_ref())? {
            return Ok(CacheDecision::Hit(hit));
        }
        // A default or no-cache request revalidates a stored response when it
        // has a validator; the 304 path reuses the stored body. A page-supplied
        // validator is the caller's own conditional request and must surface
        // its 304
        // (<https://fetch.spec.whatwg.org/#http-network-or-cache-fetch>).
        let etag = matches!(mode, RequestCache::Default | RequestCache::NoCache)
            .then(|| cached.as_ref().and_then(|entry| etag(&entry.outcome)))
            .flatten();
        Ok(CacheDecision::Miss {
            mode,
            stored: cached,
            etag,
        })
    }

    fn cached_integrity_ok(request: &DialRequest, entry: &CachedResponse) -> bool {
        let Some(integrity) = request.integrity.as_deref() else {
            return true;
        };
        match &entry.outcome.body {
            ResponseBody::Bytes(body) => renderer::integrity_verifies(integrity, body),
            ResponseBody::Undecodable | ResponseBody::Streamed(_) => false,
        }
    }

    fn cached_hit(
        &self,
        request: &DialRequest,
        initiator: &Url,
        partition: RendererAssignmentId,
    ) -> Option<CachedResponse> {
        if !request.policy.exposes_response() {
            return None;
        }
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&cache_key(request, initiator, partition))
            .filter(|entry| vary_matches(entry, request) && cache_usable(request, initiator, entry))
            .cloned()
    }

    fn cached_response(
        request: &DialRequest,
        mode: RequestCache,
        cached: Option<&CachedResponse>,
    ) -> Result<Option<DialOutcome>, DialFailure> {
        if !request.policy.exposes_response() {
            return Ok(None);
        }
        let hit = match (mode, cached) {
            (RequestCache::OnlyIfCached, None) => return Err(DialFailure::Connect),
            (RequestCache::OnlyIfCached, Some(cached)) => {
                if let (Ok(request_url), Ok(final_url)) = (
                    Url::parse(&request.url),
                    Url::parse(&cached.outcome.final_url),
                ) && request_url.origin() != final_url.origin()
                {
                    return Err(DialFailure::Connect);
                }
                cached
            }
            (RequestCache::ForceCache, Some(cached)) => cached,
            (RequestCache::Default, Some(cached)) if fresh(cached) => cached,
            (
                RequestCache::Default
                | RequestCache::ForceCache
                | RequestCache::NoStore
                | RequestCache::Reload
                | RequestCache::NoCache,
                _,
            ) => return Ok(None),
        };
        if !Self::cached_integrity_ok(request, hit) {
            return Err(DialFailure::Connect);
        }
        Ok(Some(hit.outcome.clone()))
    }

    fn remember_response(
        &self,
        request: &DialRequest,
        initiator: &Url,
        partition: RendererAssignmentId,
        mode: RequestCache,
        outcome: &DialOutcome,
    ) {
        if !should_store(request, outcome, mode) {
            return;
        }
        store_response(
            &self.cache,
            cache_key(request, initiator, partition),
            outcome.clone(),
            request.headers.clone(),
        );
    }

    /// The deadline budget one page dial gets: the per-call cap, lowered by
    /// the request's own `timeout_ms` when it set one.
    fn dial_budget(request: &DialRequest) -> Duration {
        if request.timeout_ms == 0 {
            PAGE_FETCH_TIMEOUT
        } else {
            PAGE_FETCH_TIMEOUT.min(Duration::from_millis(request.timeout_ms))
        }
    }

    /// Records a successful style sheet response's URLs, so a later explicit
    /// referrer naming the sheet can be authorized. A sheet that failed or
    /// never arrived is not recorded: nothing it contained can issue a request
    /// (<https://w3c.github.io/webappsec-referrer-policy/#integration-with-css>).
    fn remember_sheet(&self, request: &DialRequest, status: u16, final_url: &str) {
        if request.policy.kind() != DialKind::Stylesheet || !(200..300).contains(&status) {
            return;
        }
        let mut sheets = self
            .sheets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for spec in [request.url.as_str(), final_url] {
            if let Ok(mut url) = Url::parse(spec) {
                // A referrer never carries a fragment, and the sheet URL the
                // renderer names is the one it was loaded from
                // (<https://w3c.github.io/webappsec-referrer-policy/#strip-url>).
                url.set_fragment(None);
                sheets.insert(url);
            }
        }
    }

    /// Builds one outbound request with this tab's style sheet set as the
    /// explicit-referrer authority. The lock is taken and released inside the
    /// call, so no guard lives across an await.
    fn build_outbound(
        &self,
        request: &DialRequest,
        initiator: &Url,
        deadline: Instant,
    ) -> Result<Request, DialFailure> {
        let sheets = self
            .sheets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        outbound_request(request, initiator, deadline, &sheets)
    }

    async fn acquire(
        semaphore: &Arc<Semaphore>,
        deadline: Instant,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, DialFailure> {
        let permit = tokio::time::timeout_at(
            tokio::time::Instant::from_std(deadline),
            Arc::clone(semaphore).acquire_owned(),
        )
        .await;
        match permit {
            Ok(Ok(permit)) => Ok(permit),
            Ok(Err(_)) => Err(DialFailure::Cancelled),
            Err(_) => Err(DialFailure::QueueFull),
        }
    }
}

/// Records a redirect the policy refuses, and refuses the hop.
///
/// The stored URL comes back as a status-0 outcome so the renderer can fire
/// `securitypolicyviolation` and drop the body.
/// <https://w3c.github.io/webappsec-csp/#should-request-be-blocked-by-content-security-policy>
fn watch_redirects(
    request: &DialRequest,
    initiator: &Url,
    outbound: &mut Request,
) -> Arc<std::sync::Mutex<Option<Url>>> {
    let blocked = Arc::new(std::sync::Mutex::new(None));
    if let Some(csp) = request.csp.clone() {
        let document_url = initiator.clone();
        let blocked = Arc::clone(&blocked);
        outbound.on_redirect = Some(Arc::new(move |next, redirects| {
            if csp.allows(&document_url, next, redirects) {
                return true;
            }
            *blocked
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(next.clone());
            false
        }));
    }
    blocked
}

/// Builds the `net` request for one page dial: URL, method, CORS parameters,
/// headers, body, and initiator.
/// JS header values are `ByteString`s: each UTF-16 code unit is one byte. The
/// renderer carries them as Rust `String`, so UTF-8 would turn `0x80..=0xFF`
/// into two wire bytes.
fn outbound_request(
    request: &DialRequest,
    initiator: &Url,
    deadline: Instant,
    sheets: &HashSet<Url>,
) -> Result<Request, DialFailure> {
    let url = Url::parse(&request.url).map_err(|_| DialFailure::Connect)?;
    let kind = request.policy.kind();
    // https://fetch.spec.whatwg.org/#forbidden-method
    if kind == DialKind::JsFetch && request.method.is_forbidden() {
        return Err(DialFailure::Connect);
    }
    // A `no-cors` request may only use CORS-safelisted methods and headers:
    // the page-side `request-no-cors` guard is advisory, so the network
    // boundary enforces it here. Otherwise a page could send an unpreflighted,
    // state-changing cross-origin request with an arbitrary method and headers
    // (<https://fetch.spec.whatwg.org/#concept-headers-guard>).
    let no_cors = request.policy.mode() == net::cors::RequestMode::NoCors;
    if no_cors && !net::cors::is_cors_safelisted_method(&request.method) {
        return Err(DialFailure::Connect);
    }
    let mut outbound = Request::new(request.method.clone(), url);
    // The origin comes from the site-authorized initiator, so a compromised
    // renderer cannot claim an origin outside its renderer's site lock
    // (<https://fetch.spec.whatwg.org/#concept-request-origin>). It can still
    // name a different origin *within* that site, which shares the renderer
    // process; deriving the origin from a per-frame record the browser owns is
    // a known gap.
    outbound.cors = net::cors::CorsSettings {
        mode: request.policy.mode(),
        credentials: request.policy.credentials(),
        redirect: request.policy.redirect(),
        origin: initiator.origin(),
    };
    if let Some(content_type) = &request.content_type {
        let content_type = isomorphic_encode(content_type).ok_or(DialFailure::Connect)?;
        outbound
            .headers
            .insert("Content-Type", content_type)
            .map_err(|_| DialFailure::Connect)?;
    }
    for (name, value) in &request.headers {
        let encoded = isomorphic_encode(value).ok_or(DialFailure::Connect)?;
        if kind == DialKind::JsFetch && net::cors::is_forbidden_request_header(name, &encoded) {
            continue;
        }
        if no_cors && !net::cors::is_no_cors_safelisted_request_header(name, &encoded) {
            continue;
        }
        outbound
            .headers
            .insert(name, encoded)
            .map_err(|_| DialFailure::Connect)?;
    }
    let destination = match kind {
        DialKind::JsFetch => "empty",
        DialKind::ClassicScript | DialKind::ModuleScript => "script",
        DialKind::Stylesheet => "style",
        DialKind::Image => "image",
        DialKind::Font => "font",
        DialKind::FrameLoad => "iframe",
        DialKind::CspReport => "report",
    };
    static_header(&mut outbound, "Sec-Fetch-Dest", destination);
    let mode = match request.policy.mode() {
        net::cors::RequestMode::Navigate => "navigate",
        net::cors::RequestMode::NoCors => "no-cors",
        net::cors::RequestMode::SameOrigin => "same-origin",
        net::cors::RequestMode::Cors => "cors",
    };
    static_header(&mut outbound, "Sec-Fetch-Mode", mode);
    if !request.body.is_empty() {
        outbound.body = Some(request.body.clone());
    }
    outbound.initiator_kind = InitiatorKind::Fetch;
    outbound.initiator = Some(initiator.clone());
    outbound.deadline = Some(deadline);
    // The renderer picks the referrer kind and policy; the browser owns the
    // client environment and the origin check. A referrer URL that is neither
    // the authorized initiator's origin nor a style sheet this tab loaded is a
    // renderer bug or forgery, so it fails closed to no referrer instead of
    // leaking. A sheet-initiated request names the sheet's URL, which can be
    // cross-origin to both the document and the target
    // (<https://fetch.spec.whatwg.org/#concept-request-referrer>,
    // <https://w3c.github.io/webappsec-referrer-policy/#integration-with-css>).
    let referrer_client = Url::parse(&request.referrer_url)
        .ok()
        .filter(|url| url.origin() == initiator.origin())
        .map(|url| net::ReferrerClient {
            origin: initiator.origin(),
            url,
        });
    outbound.referrer = match &request.referrer {
        net::Referrer::Url(url) if referrer_authorized(url, initiator, sheets) => {
            net::Referrer::Url(url.clone())
        }
        net::Referrer::Url(_) => net::Referrer::NoReferrer,
        referrer => referrer.clone(),
    };
    outbound.referrer_policy = request.referrer_policy;
    outbound.referrer_client = referrer_client;
    Ok(outbound)
}

/// Whether an explicit referrer URL is one the renderer may name: same origin
/// as the authorized initiator, or a style sheet this tab's network served.
/// The sheet arm is what authorizes CSS-initiated requests, whose referrer is
/// the sheet's URL on whatever host the sheet came from
/// (<https://w3c.github.io/webappsec-referrer-policy/#integration-with-css>).
fn referrer_authorized(referrer: &Url, initiator: &Url, sheets: &HashSet<Url>) -> bool {
    if referrer.origin() == initiator.origin() {
        return true;
    }
    let mut stripped = referrer.clone();
    stripped.set_fragment(None);
    sheets.contains(&stripped)
}

/// The cache write a streamed body performs once its bytes are complete.
struct PendingStore {
    outcome: DialOutcome,
    cache: ResponseCache,
    key: CacheKey,
    request_headers: Vec<(String, String)>,
}

/// One event from [`BodyReader`].
enum BodyEvent {
    /// A decoded chunk. The reader has already collected it when the delivery
    /// mode needs the whole body.
    Chunk(Vec<u8>),
    /// The body ended; the collected bytes are what integrity checked.
    Done(Vec<u8>),
    /// The body could not be decoded for its `Content-Encoding`.
    DecodeError,
    /// The transfer failed before the body ended.
    Failed(DialFailure),
}

/// Reads one decoded response body. Both delivery modes drive this reader, so
/// the size cap, integrity check, and error mapping exist once.
struct BodyReader {
    body: net::Body,
    integrity: Option<String>,
    collect: bool,
    collected: Vec<u8>,
    total: usize,
}

impl BodyReader {
    async fn new(response: net::Response, integrity: Option<String>, collect: bool) -> Self {
        Self {
            body: response.into_body().await,
            integrity,
            collect,
            collected: Vec::new(),
            total: 0,
        }
    }

    async fn next(&mut self) -> BodyEvent {
        match self.body.read_chunk().await {
            Ok(Some(chunk)) => {
                self.total = self.total.saturating_add(chunk.len());
                if self.total > renderer::MAX_RESPONSE_BODY_BYTES {
                    return BodyEvent::Failed(DialFailure::Limit);
                }
                if self.collect {
                    self.collected.extend_from_slice(&chunk);
                }
                BodyEvent::Chunk(chunk)
            }
            Ok(None) => {
                if let Some(integrity) = self.integrity.as_deref()
                    && !renderer::integrity_verifies(integrity, &self.collected)
                {
                    return BodyEvent::Failed(DialFailure::Connect);
                }
                BodyEvent::Done(std::mem::take(&mut self.collected))
            }
            Err(net::NetError::Decode(_)) => BodyEvent::DecodeError,
            Err(error) => BodyEvent::Failed(dial_failure(&error)),
        }
    }
}

/// Everything one streamed body task owns until its bytes are done: the
/// response, the permits it borrows, and the cache write it may finish.
struct StreamJob {
    response: net::Response,
    integrity: Option<String>,
    store: Option<PendingStore>,
    permits: DialPermits,
    kind: DialKind,
    url: String,
    tx: tokio::sync::mpsc::Sender<StreamMessage>,
}

fn spawn_stream_body(job: StreamJob) -> tokio::task::JoinHandle<()> {
    let StreamJob {
        response,
        integrity,
        store,
        permits,
        kind,
        url,
        tx,
    } = job;
    tokio::spawn(async move {
        let _permits = permits;
        let collect = store.is_some() || integrity.is_some();
        let mut reader = BodyReader::new(response, integrity, collect).await;
        loop {
            match reader.next().await {
                BodyEvent::Chunk(chunk) => {
                    if tx.send(StreamMessage::Chunk(chunk)).await.is_err() {
                        return;
                    }
                }
                BodyEvent::Done(body) => {
                    if let Some(mut store) = store {
                        store.outcome.body = ResponseBody::Bytes(body);
                        store_response(
                            &store.cache,
                            store.key,
                            store.outcome,
                            store.request_headers,
                        );
                    }
                    let _ = tx.send(StreamMessage::End).await;
                    logging::trace!(target: "browser::net", "stream done {kind:?} {url}");
                    return;
                }
                BodyEvent::DecodeError => {
                    let _ = tx.send(StreamMessage::Error(DialFailure::Connect)).await;
                    logging::trace!(target: "browser::net", "stream decode error {kind:?} {url}");
                    return;
                }
                BodyEvent::Failed(failure) => {
                    let _ = tx.send(StreamMessage::Error(failure)).await;
                    logging::trace!(
                        target: "browser::net",
                        "stream failed {kind:?} {url}: {failure:?}"
                    );
                    return;
                }
            }
        }
    })
}

fn trace_dial_result(
    request: &DialRequest,
    started: Instant,
    result: Result<&DialOutcome, &DialFailure>,
) {
    match result {
        Ok(outcome) => logging::trace!(
            target: "browser::net",
            "request done {:?} {} -> {} in {:?}",
            request.policy.kind(),
            request.url,
            outcome.status,
            started.elapsed()
        ),
        Err(failure) => logging::trace!(
            target: "browser::net",
            "request failed {:?} {} in {:?}: {failure:?}",
            request.policy.kind(),
            request.url,
            started.elapsed()
        ),
    }
}

fn refused_redirect(blocked: &std::sync::Mutex<Option<Url>>) -> Option<DialOutcome> {
    let url = blocked
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()?;
    Some(DialOutcome {
        status: 0,
        status_text: String::new(),
        final_url: url.to_string(),
        content_type: None,
        headers: Vec::new(),
        body: ResponseBody::Bytes(Vec::new()),
        response_type: net::cors::ResponseType::Basic,
        crossed_origin: false,
        redirect_count: 0,
        referrer: None,
    })
}

pub(crate) fn dial_failure(error: &net::NetError) -> DialFailure {
    use net::{NetError, TransportError};
    match error {
        NetError::Transport(TransportError::Dns(_)) => DialFailure::Dns,
        NetError::Transport(TransportError::Tls(_)) => DialFailure::Tls,
        NetError::Transport(TransportError::Timeout(_)) => DialFailure::Timeout,
        NetError::Limit(_) => DialFailure::Limit,
        NetError::Transport(TransportError::Connect(_) | TransportError::Io(_))
        | NetError::Protocol(_)
        | NetError::Decode(_) => DialFailure::Connect,
    }
}

/// Navigation identity headers for Chrome emulation.
///
/// `Upgrade-Insecure-Requests` is the document navigation preference
/// (<https://www.w3.org/TR/upgrade-insecure-requests/#preference>).
/// `Accept` and Fetch Metadata (`Sec-Fetch-*`) match Chrome's document
/// navigation request
/// (<https://w3c.github.io/webappsec-fetch-metadata/>).
/// Low-entropy UA client hints are sent only to potentially trustworthy URLs
/// (<https://wicg.github.io/ua-client-hints/#sec-ch-ua>,
/// <https://w3c.github.io/webappsec-secure-contexts/#is-url-trustworthy>).
fn chrome_navigation_request(request: &mut Request, url: &Url) {
    static_header(request, "Upgrade-Insecure-Requests", "1");
    static_header(
        request,
        "Accept",
        "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8",
    );
    static_header(request, "Sec-Fetch-Site", "none");
    static_header(request, "Sec-Fetch-Mode", "navigate");
    static_header(request, "Sec-Fetch-User", "?1");
    static_header(request, "Sec-Fetch-Dest", "document");
    if net::is_potentially_trustworthy(url) {
        for (name, value) in [
            ("Sec-CH-UA", crate::SEC_CH_UA),
            ("Sec-CH-UA-Mobile", crate::SEC_CH_UA_MOBILE),
            ("Sec-CH-UA-Platform", crate::SEC_CH_UA_PLATFORM),
            (
                "Sec-CH-Prefers-Color-Scheme",
                crate::SEC_CH_PREFERS_COLOR_SCHEME,
            ),
        ] {
            static_header(request, name, value);
        }
    }
}

/// Sets a request header this process chose, not the page. The value is a
/// static HTTP token, so a rejected one is a programming error, and the panic
/// names the header that has none.
fn static_header(request: &mut Request, name: &str, value: &str) {
    request
        .headers
        .insert(name, value)
        .unwrap_or_else(|_| panic!("{name}: {value:?} is not a header value"));
}

/// Every `Content-Type` value from the response as one string, the way
/// getting a list header does: values combined with `", "`, each byte the
/// same code point
/// (<https://fetch.spec.whatwg.org/#concept-header-list-get-decode-split>,
/// <https://infra.spec.whatwg.org/#isomorphic-decode>). The renderer extracts
/// the MIME type from it
/// (<https://fetch.spec.whatwg.org/#concept-header-extract-mime-type>).
fn content_type_header(headers: &net::HeaderMap) -> Option<String> {
    let values: Vec<String> = headers
        .get_all("content-type")
        .map(isomorphic_decode)
        .collect();
    net::combined_header(values.iter().map(String::as_str))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn js_fetch(mode: renderer::CorsMode, url: &str) -> DialRequest {
        DialRequest {
            policy: renderer::RequestPolicy::fetch(renderer::CorsRequest {
                mode,
                credentials: net::cors::CredentialsMode::SameOrigin,
                redirect: net::cors::RedirectMode::Follow,
            }),
            url: url.to_owned(),
            initiator: "http://a.test/page".to_owned(),
            delivery: BodyDelivery::Buffer,
            method: net::Method::GET,
            body: Vec::new(),
            content_type: None,
            headers: Vec::new(),
            referrer: net::Referrer::NoReferrer,
            referrer_url: "http://a.test/page".to_owned(),
            timeout_ms: 0,
            csp: None,
            keepalive: false,
            body_len: 0,
            integrity: None,
            cache: RequestCache::Default,
            referrer_policy: net::ReferrerPolicy::default(),
        }
    }

    fn cached(response_type: net::cors::ResponseType) -> CachedResponse {
        CachedResponse {
            outcome: DialOutcome {
                status: 200,
                status_text: "OK".to_owned(),
                final_url: "http://a.test/".to_owned(),
                content_type: None,
                headers: Vec::new(),
                body: ResponseBody::Bytes(Vec::new()),
                response_type,
                crossed_origin: false,
                redirect_count: 0,
                referrer: None,
            },
            stored_at: Instant::now(),
            request_headers: Vec::new(),
        }
    }

    #[test]
    fn cache_partitions_by_document_and_initiator_origin() {
        let request = js_fetch(renderer::CorsMode::Cors, "http://a.test/data");
        let initiator = Url::parse("http://a.test/page").expect("page URL");
        let other_initiator = Url::parse("http://b.test/page").expect("other URL");
        let first = cache_key(&request, &initiator, RendererAssignmentId::new(1));
        let same = cache_key(&request, &initiator, RendererAssignmentId::new(1));
        let other_document = cache_key(&request, &initiator, RendererAssignmentId::new(2));
        let other_origin = cache_key(&request, &other_initiator, RendererAssignmentId::new(1));
        assert_eq!(first, same);
        assert_ne!(first, other_document);
        assert_ne!(first, other_origin);
    }

    #[test]
    fn cached_cors_response_is_not_reused_as_no_cors_across_origins() {
        let request = js_fetch(renderer::CorsMode::NoCors, "http://b.test/data");
        let initiator = Url::parse("http://a.test/page").expect("page URL");
        assert!(!cache_usable(
            &request,
            &initiator,
            &cached(net::cors::ResponseType::Cors)
        ));
        assert!(!cache_usable(
            &request,
            &initiator,
            &cached(net::cors::ResponseType::Opaque)
        ));
        let same_origin = js_fetch(renderer::CorsMode::NoCors, "http://a.test/data");
        assert!(cache_usable(
            &same_origin,
            &initiator,
            &cached(net::cors::ResponseType::Basic)
        ));
    }

    #[test]
    fn cached_response_follows_the_request_cache_mode() {
        let request = js_fetch(renderer::CorsMode::Cors, "http://a.test/data");
        let mut entry = cached(net::cors::ResponseType::Basic);
        assert_eq!(
            TabNetworkHandle::cached_response(&request, RequestCache::OnlyIfCached, None)
                .expect_err("only-if-cached needs an entry"),
            DialFailure::Connect
        );
        let mut cross_origin = entry.clone();
        cross_origin.outcome.final_url = "http://b.test/data".to_owned();
        assert!(
            TabNetworkHandle::cached_response(
                &request,
                RequestCache::OnlyIfCached,
                Some(&cross_origin)
            )
            .is_err(),
            "only-if-cached refuses a cross-origin final URL"
        );
        assert!(
            TabNetworkHandle::cached_response(&request, RequestCache::OnlyIfCached, Some(&entry))
                .expect("hit")
                .is_some()
        );
        assert!(
            TabNetworkHandle::cached_response(&request, RequestCache::ForceCache, Some(&entry))
                .expect("stale hit")
                .is_some(),
            "force-cache uses a stale entry"
        );
        assert!(
            TabNetworkHandle::cached_response(&request, RequestCache::ForceCache, None)
                .expect("miss")
                .is_none()
        );
        assert!(
            TabNetworkHandle::cached_response(&request, RequestCache::Default, Some(&entry))
                .expect("stale miss")
                .is_none(),
            "default does not use a stale entry"
        );
        entry
            .outcome
            .headers
            .push(("cache-control".to_owned(), "max-age=60".to_owned()));
        assert!(
            TabNetworkHandle::cached_response(&request, RequestCache::Default, Some(&entry))
                .expect("fresh hit")
                .is_some(),
            "default uses a fresh entry"
        );
        for mode in [
            RequestCache::NoStore,
            RequestCache::Reload,
            RequestCache::NoCache,
        ] {
            assert!(
                TabNetworkHandle::cached_response(&request, mode, Some(&entry))
                    .expect("miss")
                    .is_none(),
                "{mode:?} must not answer from the cache"
            );
        }
    }

    #[tokio::test]
    async fn a_cancelled_stream_releases_its_permit_for_the_next_dial() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let mut context = NetworkContext::new(AgentOptions::default());
        context.permits.global = Arc::new(Semaphore::new(1));
        let handle = context.tab_handle();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        let origin = format!("http://{address}");
        let server = tokio::spawn(async move {
            for index in 0..2 {
                let (mut socket, _) = listener.accept().await.expect("accept");
                tokio::spawn(async move {
                    let mut buffer = [0u8; 1024];
                    let _ = socket.read(&mut buffer).await;
                    if index == 0 {
                        socket
                            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1024\r\n\r\npartial")
                            .await
                            .expect("stalled head");
                        tokio::time::sleep(Duration::from_secs(30)).await;
                    } else {
                        socket
                            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                            .await
                            .expect("complete head");
                    }
                });
            }
        });
        let initiator = Url::parse(&format!("{origin}/page")).expect("page URL");
        let owner = RendererAssignmentId::new(1);
        let mut stream_request =
            js_fetch(renderer::CorsMode::SameOrigin, &format!("{origin}/stream"));
        stream_request.initiator = initiator.origin().ascii_serialization();
        stream_request.referrer_url = initiator.to_string();
        stream_request.delivery = BodyDelivery::Stream;
        let outcome = handle
            .dial_request(&stream_request, &initiator, owner)
            .await
            .expect("stream head");
        let ResponseBody::Streamed(stream) = outcome.body else {
            panic!("the outcome is a stream");
        };
        assert_eq!(
            handle.permits.global.available_permits(),
            0,
            "the body task holds the permits"
        );
        handle.cancel_stream(owner, stream);
        tokio::time::timeout(Duration::from_secs(1), async {
            while handle.permits.global.available_permits() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cancelling the stream releases the permit");

        let mut next_request = js_fetch(renderer::CorsMode::SameOrigin, &format!("{origin}/done"));
        next_request.initiator = initiator.origin().ascii_serialization();
        next_request.referrer_url = initiator.to_string();
        let next = tokio::time::timeout(
            Duration::from_secs(1),
            handle.dial_request(&next_request, &initiator, owner),
        )
        .await
        .expect("the next dial gets the released permit")
        .expect("next dial");
        assert!(
            matches!(&next.body, ResponseBody::Bytes(body) if body == b"ok"),
            "the next dial reads its body"
        );
        server.abort();
    }

    /// A `no-cors` subresource reads the unsafe response — body, status, and
    /// headers — because the engine consumes it, while the same request made
    /// by page script is an opaque filtered response
    /// (<https://fetch.spec.whatwg.org/#concept-internal-response>).
    #[tokio::test]
    async fn an_opaque_subresource_is_readable_by_the_engine_and_not_by_script() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut socket, _) = listener.accept().await.expect("accept");
                tokio::spawn(async move {
                    let mut buffer = [0u8; 1024];
                    let _ = socket.read(&mut buffer).await;
                    socket
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nContent-Type: image/png\r\nSet-Cookie: a=1\r\n\r\npng",
                        )
                        .await
                        .expect("response");
                });
            }
        });
        let context = NetworkContext::new(AgentOptions::default());
        let handle = context.tab_handle();
        let initiator = Url::parse("http://a.test/page").expect("page URL");
        let owner = RendererAssignmentId::new(1);
        let url = format!("http://{address}/img");

        let mut image = js_fetch(renderer::CorsMode::NoCors, &url);
        image.policy = renderer::RequestPolicy::image(None);
        let outcome = handle
            .dial_request(&image, &initiator, owner)
            .await
            .expect("image dial");
        assert_eq!(
            outcome.response_type,
            net::cors::ResponseType::Opaque,
            "a cross-origin no-cors image is opaque"
        );
        assert_eq!(outcome.status, 200, "the engine reads the unsafe status");
        assert_eq!(
            outcome.content_type.as_deref(),
            Some("image/png"),
            "the image decoder needs the served type"
        );
        assert!(
            matches!(&outcome.body, ResponseBody::Bytes(body) if body == b"png"),
            "the engine reads the unsafe body"
        );
        assert!(
            outcome
                .headers
                .iter()
                .all(|(name, _)| !name.eq_ignore_ascii_case("set-cookie")),
            "an HttpOnly cookie value must not reach the renderer"
        );

        let page = js_fetch(renderer::CorsMode::NoCors, &url);
        let outcome = handle
            .dial_request(&page, &initiator, owner)
            .await
            .expect("page dial");
        assert_eq!(
            outcome.status, 0,
            "an opaque filtered response has no status"
        );
        assert!(
            matches!(&outcome.body, ResponseBody::Bytes(body) if body.is_empty()),
            "an opaque filtered response has a null body"
        );
        assert!(
            outcome.headers.is_empty(),
            "an opaque filtered response has no headers"
        );
        server.abort();
    }

    #[test]
    fn an_undecodable_body_is_not_stored() {
        let request = js_fetch(renderer::CorsMode::SameOrigin, "http://a.test/data");
        let mut outcome = cached(net::cors::ResponseType::Basic).outcome;
        assert!(should_store(&request, &outcome, RequestCache::Default));
        outcome.body = ResponseBody::Undecodable;
        assert!(
            !should_store(&request, &outcome, RequestCache::Default),
            "a decode failure has nothing to reuse, so a hit would replay it"
        );
    }

    #[test]
    fn vary_star_never_matches_and_a_named_field_must_agree() {
        let mut request = js_fetch(renderer::CorsMode::Cors, "http://a.test/data");
        request.headers = vec![("accept".to_owned(), "text/html".to_owned())];
        let mut entry = cached(net::cors::ResponseType::Basic);
        entry.request_headers = request.headers.clone();
        assert!(
            vary_matches(&entry, &request),
            "a stored response without Vary matches"
        );

        entry.outcome.headers = vec![("vary".to_owned(), "*".to_owned())];
        assert!(!vary_matches(&entry, &request));

        entry.outcome.headers = vec![("Vary".to_owned(), "Accept".to_owned())];
        assert!(vary_matches(&entry, &request));
        request.headers = vec![("accept".to_owned(), "application/json".to_owned())];
        assert!(
            !vary_matches(&entry, &request),
            "a different request header selects a different representation"
        );
    }

    #[test]
    fn fresh_reads_max_age_through_the_header_splitter() {
        let mut entry = cached(net::cors::ResponseType::Basic);
        entry.outcome.headers = vec![(
            "cache-control".to_owned(),
            "private=\"a,b\", max-age=60".to_owned(),
        )];
        assert!(fresh(&entry));

        entry.outcome.headers = vec![("cache-control".to_owned(), "max-age=1".to_owned())];
        entry.stored_at = Instant::now()
            .checked_sub(Duration::from_secs(2))
            .expect("two seconds before now");
        assert!(!fresh(&entry), "an expired max-age is stale");

        // `age` counts whole seconds, so `max-age=0` must not stay fresh for
        // the first second of its life
        // (<https://www.rfc-editor.org/rfc/rfc9111#section-4.2>).
        entry.outcome.headers = vec![("cache-control".to_owned(), "max-age=0".to_owned())];
        entry.stored_at = Instant::now();
        assert!(!fresh(&entry), "a zero lifetime is spent on arrival");

        entry.outcome.headers = vec![(
            "expires".to_owned(),
            "Thu, 01 Jan 2099 00:00:00 GMT".to_owned(),
        )];
        assert!(fresh(&entry), "a future Expires is fresh");
    }

    /// A stored `ETag` holds isomorphic-decoded bytes, so writing it back must
    /// keep one byte per code point. UTF-8 would send `0xC3 0xA9` where the
    /// server sent `0xE9`, and every revalidation would answer 200.
    #[tokio::test]
    async fn revalidation_sends_a_non_ascii_etag_as_one_byte_per_code_point() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let context = NetworkContext::new(AgentOptions::default());
        let handle = context.tab_handle();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        let origin = format!("http://{address}");
        let (replayed_tx, replayed_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let mut replayed_tx = Some(replayed_tx);
            for index in 0..2 {
                let (mut socket, _) = listener.accept().await.expect("accept");
                let mut buffer = [0u8; 2048];
                let read = socket.read(&mut buffer).await.expect("request");
                if index == 0 {
                    socket
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nETag: \"\xE9\"\r\nSet-Cookie: sid=secret; HttpOnly\r\nContent-Length: 2\r\n\r\nhi",
                        )
                        .await
                        .expect("first response");
                } else {
                    let _ = replayed_tx
                        .take()
                        .expect("one replay")
                        .send(buffer[..read].to_vec());
                    socket
                        .write_all(
                            b"HTTP/1.1 304 Not Modified\r\nETag: \"\xE9\"\r\nSet-Cookie: sid=secret; HttpOnly\r\n\r\n",
                        )
                        .await
                        .expect("304");
                }
            }
        });
        let initiator = Url::parse(&format!("{origin}/page")).expect("page URL");
        let owner = RendererAssignmentId::new(1);
        let mut request = js_fetch(renderer::CorsMode::SameOrigin, &format!("{origin}/etag"));
        request.initiator = initiator.origin().ascii_serialization();
        request.referrer_url = initiator.to_string();
        let first = handle
            .dial_request(&request, &initiator, owner)
            .await
            .expect("first dial");
        assert_eq!(first.status, 200);
        assert!(
            !first
                .headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("set-cookie")),
            "an HttpOnly cookie value must not reach page script"
        );

        let revalidated = handle
            .dial_request(&request, &initiator, owner)
            .await
            .expect("revalidation");
        assert_eq!(revalidated.status, 200, "a 304 merges into the stored head");
        assert_eq!(stored_bytes(&revalidated), 2);
        assert!(matches!(&revalidated.body, ResponseBody::Bytes(body) if body == b"hi"));
        assert!(
            !revalidated
                .headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("set-cookie")),
            "a 304 merge must not add the cookie the page path filtered out"
        );

        let replayed = replayed_rx.await.expect("second request");
        assert!(
            replayed
                .windows(b"if-none-match: \"\xE9\"".len())
                .any(|window| window == b"if-none-match: \"\xE9\""),
            "the validator went back as the bytes the server sent: {:?}",
            String::from_utf8_lossy(&replayed)
        );
        server.abort();
    }

    #[test]
    fn cached_body_still_needs_to_satisfy_integrity_metadata() {
        let mut request = js_fetch(renderer::CorsMode::Cors, "http://a.test/data");
        request.integrity = Some("sha256-47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=".to_owned());
        assert!(TabNetworkHandle::cached_integrity_ok(
            &request,
            &cached(net::cors::ResponseType::Basic)
        ));
        request.integrity = Some("sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_owned());
        assert!(!TabNetworkHandle::cached_integrity_ok(
            &request,
            &cached(net::cors::ResponseType::Basic)
        ));
    }

    #[test]
    fn cache_evicts_the_oldest_entry_at_the_count_limit() {
        let mut cache = HashMap::new();
        let initiator = Url::parse("http://a.test/page").expect("page URL");
        let keys: Vec<CacheKey> = (0..=CACHE_MAX_ENTRIES)
            .map(|index| {
                let mut request = js_fetch(renderer::CorsMode::Cors, "http://a.test/data");
                request.url = format!("http://a.test/{index}");
                cache_key(&request, &initiator, RendererAssignmentId::new(1))
            })
            .collect();
        for (index, key) in keys.iter().enumerate() {
            let mut entry = cached(net::cors::ResponseType::Basic);
            entry.stored_at =
                Instant::now() + Duration::from_secs(u64::try_from(index).expect("entry index"));
            insert_cached(&mut cache, key.clone(), entry);
        }
        assert_eq!(cache.len(), CACHE_MAX_ENTRIES);
        assert!(
            !cache.contains_key(&keys[0]),
            "the oldest entry is the one evicted"
        );
        assert!(
            cache.contains_key(&keys[CACHE_MAX_ENTRIES]),
            "the entry just inserted survives"
        );
    }

    #[test]
    fn cache_evicts_by_body_bytes_from_the_oldest_entry() {
        let bodies = CACHE_MAX_BYTES / renderer::MAX_RESPONSE_BODY_BYTES + 1;
        let mut cache = HashMap::new();
        let initiator = Url::parse("http://a.test/page").expect("page URL");
        let keys: Vec<CacheKey> = (0..bodies)
            .map(|index| {
                let mut request = js_fetch(renderer::CorsMode::Cors, "http://a.test/data");
                request.url = format!("http://a.test/{index}");
                cache_key(&request, &initiator, RendererAssignmentId::new(1))
            })
            .collect();
        for (index, key) in keys.iter().enumerate() {
            let mut entry = cached(net::cors::ResponseType::Basic);
            entry.outcome.body = ResponseBody::Bytes(vec![0; renderer::MAX_RESPONSE_BODY_BYTES]);
            entry.stored_at =
                Instant::now() + Duration::from_secs(u64::try_from(index).expect("entry index"));
            insert_cached(&mut cache, key.clone(), entry);
        }
        assert_eq!(cache.len(), bodies - 1);
        assert!(
            !cache.contains_key(&keys[0]),
            "the oldest body is the one evicted"
        );
        assert!(
            cache.contains_key(&keys[bodies - 1]),
            "the body just inserted stays"
        );
    }

    #[tokio::test]
    async fn exhausted_permits_fail_with_queue_full_before_the_deadline() {
        let permits = Arc::new(Semaphore::new(1));
        let _held = Arc::clone(&permits).acquire_owned().await.expect("permit");

        let error = TabNetworkHandle::acquire(&permits, Instant::now() + Duration::from_millis(50))
            .await
            .expect_err("second permit must time out");
        assert_eq!(error, DialFailure::QueueFull);
    }

    #[tokio::test]
    async fn released_permits_are_reusable() {
        let permits = Arc::new(Semaphore::new(1));
        {
            let _held = Arc::clone(&permits).acquire_owned().await.expect("permit");
            let error =
                TabNetworkHandle::acquire(&permits, Instant::now() + Duration::from_millis(20))
                    .await
                    .expect_err("held permit");
            assert_eq!(error, DialFailure::QueueFull);
        }
        let _again =
            TabNetworkHandle::acquire(&permits, Instant::now() + Duration::from_millis(50))
                .await
                .expect("permit after release");
    }

    #[test]
    fn chrome_navigation_client_hints_are_limited_to_trustworthy_urls() {
        let hints = [
            "sec-ch-ua",
            "sec-ch-ua-mobile",
            "sec-ch-ua-platform",
            "sec-ch-prefers-color-scheme",
        ];
        let secure = Url::parse("https://example.com/").expect("https url");
        let mut secure_request = net::Request::new(net::Method::GET, secure.clone());
        chrome_navigation_request(&mut secure_request, &secure);
        for hint in hints {
            assert!(
                secure_request.headers.get(hint).is_some(),
                "{hint} must reach a trustworthy URL"
            );
        }

        let insecure = Url::parse("http://example.com/").expect("http url");
        let mut insecure_request = net::Request::new(net::Method::GET, insecure.clone());
        chrome_navigation_request(&mut insecure_request, &insecure);
        for hint in hints {
            assert!(
                insecure_request.headers.get(hint).is_none(),
                "{hint} must stay off plain http"
            );
        }
        assert!(insecure_request.headers.get("sec-fetch-mode").is_some());
        assert!(insecure_request.headers.get("accept").is_some());
    }
}
