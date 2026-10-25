//! Browser-owned live networking: one [`net::Agent`] behind a value-only handle.
//!
//! Tab coordinators receive [`TabNetworkHandle`]. They do not expose or own
//! [`net::Agent`].

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use net::{Agent, AgentOptions, InitiatorKind, Method, Request};
use renderer::{DialFailure, DialKind, DialOutcome, DialRequest};
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
    method: String,
    url: String,
    credentials: String,
    initiator: String,
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

/// The cache decision shared by the buffered and streaming dial paths.
/// `hit` is a complete response; `etag` is a stored validator the network
/// path may use for a conditional request.
struct CacheRead {
    mode: String,
    explicit_validator: bool,
    hit: Option<DialOutcome>,
    stored: Option<CachedResponse>,
    etag: Option<String>,
}

const CACHE_MAX_ENTRIES: usize = 64;
const CACHE_MAX_BYTES: usize = 64 * 1024 * 1024;

fn insert_cached(
    cache: &mut HashMap<CacheKey, CachedResponse>,
    key: CacheKey,
    entry: CachedResponse,
) {
    let body_len = entry.outcome.body.as_ref().map_or(0, Vec::len);
    if body_len > renderer::MAX_RESPONSE_BODY_BYTES {
        return;
    }
    let mut total = cache
        .values()
        .map(|entry| entry.outcome.body.as_ref().map_or(0, Vec::len))
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
            total = total.saturating_sub(removed.outcome.body.as_ref().map_or(0, Vec::len));
        }
    }
    cache.insert(key, entry);
}

enum StreamMessage {
    Chunk(Vec<u8>),
    End,
    Error(DialFailure),
}

struct StreamState {
    rx: tokio::sync::mpsc::UnboundedReceiver<StreamMessage>,
}

#[derive(Clone)]
struct StreamEntry {
    owner: RendererAssignmentId,
    state: Arc<tokio::sync::Mutex<StreamState>>,
}

type StreamMap = Arc<Mutex<HashMap<u64, StreamEntry>>>;

fn cache_key(request: &DialRequest, initiator: &Url, partition: RendererAssignmentId) -> CacheKey {
    CacheKey {
        method: request.method.clone(),
        url: request.url.clone(),
        credentials: request.cors.map_or_else(
            || "none".to_owned(),
            |cors| format!("{:?}", cors.credentials),
        ),
        initiator: initiator.origin().ascii_serialization(),
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

fn cache_mode(request: &DialRequest) -> &str {
    request.cache.as_deref().unwrap_or("default")
}

fn response_forbids_storage(outcome: &DialOutcome) -> bool {
    outcome.headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("cache-control")
            && value.to_ascii_lowercase().contains("no-store")
    })
}

fn vary_matches(entry: &CachedResponse, request: &DialRequest) -> bool {
    let request_header = |name: &str| {
        request
            .headers
            .iter()
            .filter(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    entry
        .outcome
        .headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("vary"))
        .flat_map(|(_, value)| value.split(','))
        .map(str::trim)
        .filter(|field| !field.is_empty())
        .all(|field| {
            if field == "*" {
                return false;
            }
            let stored = entry
                .request_headers
                .iter()
                .filter(|(name, _)| name.eq_ignore_ascii_case(field))
                .map(|(_, value)| value.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            stored == request_header(field)
        })
}

fn fresh(entry: &CachedResponse) -> bool {
    let age = entry.stored_at.elapsed().as_secs();
    let mut saw_max_age = false;
    for (name, value) in &entry.outcome.headers {
        if !name.eq_ignore_ascii_case("cache-control") {
            continue;
        }
        for directive in value.split(',') {
            let Some((name, value)) = directive.trim().split_once('=') else {
                continue;
            };
            if name.eq_ignore_ascii_case("max-age") {
                saw_max_age = true;
                if value
                    .trim()
                    .parse::<u64>()
                    .is_ok_and(|max_age| age <= max_age)
                {
                    return true;
                }
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
fn merge_not_modified(cached: &DialOutcome, response: &net::Response) -> DialOutcome {
    let mut merged = cached.clone();
    merged.status = 200;
    merged.final_url = response.final_url().to_string();
    merged.referrer = response.referrer().map(ToString::to_string);
    for (name, value) in response.headers().iter() {
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

fn should_store(request: &DialRequest, outcome: &DialOutcome, mode: &str) -> bool {
    request.kind == DialKind::JsFetch
        && !has_page_conditional(request)
        && request.method.eq_ignore_ascii_case("GET")
        && (200..300).contains(&outcome.status)
        && outcome.body.is_ok()
        // An opaque response has no readable body and cannot satisfy a later
        // CORS request, so keeping it only creates a response-type mismatch.
        && !outcome.response_type.is_opaque()
        && mode != "no-store"
        && !response_forbids_storage(outcome)
}

/// Rechecks the request-mode rules that produced a cached filtered response.
/// The cache partition keeps other origins away, but one document can still
/// switch modes between requests.
fn cache_usable(request: &DialRequest, initiator: &Url, entry: &CachedResponse) -> bool {
    if entry.outcome.response_type.is_opaque() {
        return false;
    }
    let Some(cors) = request.cors else {
        return true;
    };
    let Ok(url) = Url::parse(&request.url) else {
        return false;
    };
    match cors.mode {
        net::cors::RequestMode::SameOrigin => url.origin() == initiator.origin(),
        net::cors::RequestMode::NoCors => {
            !matches!(entry.outcome.response_type, net::cors::ResponseType::Cors)
        }
        net::cors::RequestMode::Cors | net::cors::RequestMode::Navigate => true,
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
    ///
    /// # Errors
    ///
    /// [`Agent::new`] rejects an invalid user agent, resolve spec, or TLS CA
    /// value.
    pub(crate) fn new(mut options: AgentOptions) -> Result<Self, net::NetError> {
        if options.user_agent.is_none() {
            options.user_agent = Some(crate::USER_AGENT.to_string());
        }
        if options.timeout_per_call.is_none() {
            options.timeout_per_call = Some(PAGE_FETCH_TIMEOUT);
        }
        let agent = Agent::new(options)?;
        Ok(Self {
            agent,
            permits: NetworkPermits::new(),
            cache: Arc::new(Mutex::new(HashMap::new())),
            streams: Arc::new(Mutex::new(HashMap::new())),
            next_stream: Arc::new(AtomicU64::new(1)),
        })
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
        let headers = response.headers();
        // The renderer owns the response-head derivation (`ResponseHead::http`),
        // so the top-level path and the child-frame path cannot disagree about
        // how a header becomes policy, endpoints, or the nosniff flag.
        let pairs: Vec<(String, String)> = headers
            .iter()
            .map(|(name, value)| (name.to_owned(), isomorphic_decode(value)))
            .collect();
        let head = renderer::ResponseHead::http(
            status,
            final_url.to_string(),
            content_type_header(headers),
            &pairs,
        );
        let body = response.into_body().await;
        Ok(NavOutcome {
            head,
            body,
            _permit: permits,
        })
    }

    /// Async HTTP request for one renderer service call, cancelled when the renderer
    /// dies or the tab closes.
    pub(crate) async fn dial_request(
        &self,
        request: &DialRequest,
        initiator: &Url,
        owner: RendererAssignmentId,
        mut cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<DialOutcome, DialFailure> {
        let started = Instant::now();
        let _keepalive = KeepaliveReservation::acquire(request)?;
        if request.stream_body {
            return self.dial_stream(request, initiator, owner).await;
        }
        let cache = self.cache_read(request, initiator, owner)?;
        if let Some(hit) = cache.hit {
            return Ok(hit);
        }
        let mode = cache.mode;
        let explicit_validator = cache.explicit_validator;
        let conditional = cache.etag;
        let stored = cache.stored;
        logging::trace!(
            target: "browser::net",
            "request start {:?} {}",
            request.kind,
            request.url
        );
        let call = async {
            let budget = Self::dial_budget(request);
            let deadline = Instant::now() + budget;
            let _global = Self::acquire(&self.permits.global, deadline).await?;
            let _tab = Self::acquire(&self.tab, deadline).await?;
            let mut outbound = self.build_outbound(request, initiator, deadline)?;
            if let Some(etag) = conditional.as_deref() {
                let _ = outbound
                    .headers
                    .insert("If-None-Match", etag.as_bytes().to_vec());
            }
            let blocked_redirect = watch_redirects(request, initiator, &mut outbound);
            let response = match self.agent.send(outbound).await {
                Ok(response) => response,
                Err(error) => {
                    return refused_redirect(&blocked_redirect).ok_or_else(|| dial_failure(&error));
                }
            };
            let response_type = response.response_type();
            // An opaque response has no readable body, so it can never satisfy
            // integrity metadata
            // (<https://www.w3.org/TR/SRI/#verify-integrity>).
            if request.integrity.is_some() && response_type.is_opaque() {
                return Err(DialFailure::Connect);
            }
            let status = response.status();
            if status == 304
                && !explicit_validator
                && let Some(cached) = stored.as_ref()
            {
                return Ok(merge_not_modified(&cached.outcome, &response));
            }
            let redirect_count = response.redirect_count();
            let final_url = response.final_url().to_string();
            let content_type = content_type_header(response.headers());
            let headers = response
                .headers()
                .iter()
                .filter(|(name, _)| {
                    request.kind != DialKind::JsFetch
                        || (!name.eq_ignore_ascii_case("set-cookie")
                            && !name.eq_ignore_ascii_case("set-cookie2"))
                })
                .map(|(name, value)| (name.to_owned(), isomorphic_decode(value)))
                .collect();
            // An opaque response hides its body as well as its status and
            // headers (<https://fetch.spec.whatwg.org/#concept-filtered-response-opaque>).
            let status_text = response.status_text().to_owned();
            let referrer = response.referrer().map(ToString::to_string);
            let body = read_response_body(request, response, response_type).await?;
            Ok(DialOutcome {
                status: if response_type.is_opaque() { 0 } else { status },
                status_text,
                // An opaque filtered response has an empty URL list, so
                // `Response.url` must not reveal where the request landed
                // (<https://fetch.spec.whatwg.org/#concept-filtered-response-opaque>).
                final_url: if response_type == net::cors::ResponseType::Opaque {
                    String::new()
                } else {
                    final_url
                },
                content_type,
                headers,
                body,
                response_type,
                redirect_count,
                referrer,
                stream: None,
            })
        };
        let result = tokio::select! {
            biased;
            _ = cancel.changed() => Err(DialFailure::Cancelled),
            result = call => result,
        };
        self.remember_response(request, initiator, owner, &mode, &result);
        if let Ok(outcome) = &result {
            self.remember_sheet(request, outcome.status, &outcome.final_url);
        }
        trace_dial_result(request, started, &result);
        result
    }

    async fn dial_stream(
        &self,
        request: &DialRequest,
        initiator: &Url,
        owner: RendererAssignmentId,
    ) -> Result<DialOutcome, DialFailure> {
        let cache = self.cache_read(request, initiator, owner)?;
        if let Some(hit) = cache.hit {
            return Ok(hit);
        }
        let mode = cache.mode;
        let explicit_validator = cache.explicit_validator;
        let conditional = cache.etag;
        let stored = cache.stored;
        let cache_key = cache_key(request, initiator, owner);
        let budget = Self::dial_budget(request);
        let deadline = Instant::now() + budget;
        let _global = Self::acquire(&self.permits.global, deadline).await?;
        let _tab = Self::acquire(&self.tab, deadline).await?;
        let mut outbound = self.build_outbound(request, initiator, deadline)?;
        if let Some(etag) = conditional.as_deref() {
            let _ = outbound
                .headers
                .insert("If-None-Match", etag.as_bytes().to_vec());
        }
        let blocked_redirect = watch_redirects(request, initiator, &mut outbound);
        let response = match self.agent.send(outbound).await {
            Ok(response) => response,
            Err(error) => {
                return refused_redirect(&blocked_redirect).ok_or_else(|| dial_failure(&error));
            }
        };
        let response_type = response.response_type();
        let status = response.status();
        if request.integrity.is_some() && response_type.is_opaque() {
            return Err(DialFailure::Connect);
        }
        if status == 304
            && !explicit_validator
            && let Some(cached) = stored.as_ref()
        {
            return Ok(merge_not_modified(&cached.outcome, &response));
        }
        let status_text = response.status_text().to_owned();
        let final_url = response.final_url().to_string();
        let headers = response
            .headers()
            .iter()
            .filter(|(name, _)| {
                !name.eq_ignore_ascii_case("set-cookie")
                    && !name.eq_ignore_ascii_case("set-cookie2")
            })
            .map(|(name, value)| (name.to_owned(), isomorphic_decode(value)))
            .collect();
        let content_type = content_type_header(response.headers());
        let mut outcome = DialOutcome {
            status: if response_type.is_opaque() { 0 } else { status },
            status_text,
            final_url: if response_type.is_opaque() {
                String::new()
            } else {
                final_url
            },
            content_type,
            headers,
            body: Ok(Vec::new()),
            response_type,
            redirect_count: response.redirect_count(),
            referrer: response.referrer().map(ToString::to_string),
            stream: None,
        };
        if response_type.is_opaque() {
            return Ok(outcome);
        }
        self.remember_sheet(request, outcome.status, &outcome.final_url);
        let id = self.next_stream.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        self.streams
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                id,
                StreamEntry {
                    owner,
                    state: Arc::new(tokio::sync::Mutex::new(StreamState { rx })),
                },
            );
        outcome.stream = Some(id);
        spawn_stream_body(
            response,
            request.integrity.clone(),
            StreamCache {
                store: should_store(request, &outcome, &mode),
                stored: outcome.clone(),
                cache: Arc::clone(&self.cache),
                cache_key,
                request_headers: request.headers.clone(),
            },
            tx,
        );
        Ok(outcome)
    }

    pub(crate) async fn read_stream(
        &self,
        owner: RendererAssignmentId,
        id: u64,
    ) -> Result<Option<Vec<u8>>, DialFailure> {
        let Some(entry) = self
            .streams
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&id)
            .cloned()
        else {
            return Ok(None);
        };
        if entry.owner != owner {
            return Ok(None);
        }
        let mut state = entry.state.lock().await;
        match state.rx.recv().await {
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

    pub(crate) fn cancel_stream(&self, owner: RendererAssignmentId, id: u64) {
        let matches_owner = self
            .streams
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&id)
            .is_some_and(|entry| entry.owner == owner);
        if matches_owner {
            self.remove_stream(id);
        }
    }

    fn remove_stream(&self, id: u64) {
        self.streams
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&id);
    }

    fn cache_read(
        &self,
        request: &DialRequest,
        initiator: &Url,
        owner: RendererAssignmentId,
    ) -> Result<CacheRead, DialFailure> {
        let mode = cache_mode(request).to_owned();
        let explicit_validator = has_page_conditional(request);
        let cached = if explicit_validator {
            None
        } else {
            self.cached_hit(request, initiator, owner)
        };
        if let Some(hit) = Self::cached_response(request, &mode, cached.as_ref())? {
            return Ok(CacheRead {
                mode,
                explicit_validator,
                hit: Some(hit),
                stored: cached,
                etag: None,
            });
        }
        // A default or no-cache request revalidates a stored response when it
        // has a validator; the 304 path reuses the stored body. A page-supplied
        // validator is the caller's own conditional request and must surface
        // its 304.
        let etag = (!explicit_validator && matches!(mode.as_str(), "default" | "no-cache"))
            .then(|| cached.as_ref().and_then(|entry| etag(&entry.outcome)))
            .flatten();
        Ok(CacheRead {
            mode,
            explicit_validator,
            hit: None,
            stored: cached,
            etag,
        })
    }

    fn cached_integrity_ok(request: &DialRequest, entry: &CachedResponse) -> bool {
        let Some(integrity) = request.integrity.as_deref() else {
            return true;
        };
        entry
            .outcome
            .body
            .as_ref()
            .is_ok_and(|body| renderer::integrity_verifies(integrity, body))
    }

    fn cached_hit(
        &self,
        request: &DialRequest,
        initiator: &Url,
        partition: RendererAssignmentId,
    ) -> Option<CachedResponse> {
        if request.kind != DialKind::JsFetch {
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
        mode: &str,
        cached: Option<&CachedResponse>,
    ) -> Result<Option<DialOutcome>, DialFailure> {
        if request.kind != DialKind::JsFetch {
            return Ok(None);
        }
        if mode == "only-if-cached" {
            let Some(cached) = cached else {
                return Err(DialFailure::Connect);
            };
            if let (Ok(request_url), Ok(final_url)) = (
                Url::parse(&request.url),
                Url::parse(&cached.outcome.final_url),
            ) && request_url.origin() != final_url.origin()
            {
                return Err(DialFailure::Connect);
            }
            if !Self::cached_integrity_ok(request, cached) {
                return Err(DialFailure::Connect);
            }
            return Ok(Some(cached.outcome.clone()));
        }
        if mode == "force-cache" {
            let Some(cached) = cached else {
                return Ok(None);
            };
            if !Self::cached_integrity_ok(request, cached) {
                return Err(DialFailure::Connect);
            }
            return Ok(Some(cached.outcome.clone()));
        }
        if mode == "default" && cached.is_some_and(fresh) {
            let Some(cached) = cached else {
                return Ok(None);
            };
            if !Self::cached_integrity_ok(request, cached) {
                return Err(DialFailure::Connect);
            }
            return Ok(Some(cached.outcome.clone()));
        }
        Ok(None)
    }

    fn remember_response(
        &self,
        request: &DialRequest,
        initiator: &Url,
        partition: RendererAssignmentId,
        mode: &str,
        result: &Result<DialOutcome, DialFailure>,
    ) {
        let Ok(outcome) = result else {
            return;
        };
        if !should_store(request, outcome, mode) {
            return;
        }
        let key = cache_key(request, initiator, partition);
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        insert_cached(
            &mut cache,
            key,
            CachedResponse {
                outcome: outcome.clone(),
                stored_at: Instant::now(),
                request_headers: request.headers.clone(),
            },
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
        if request.kind != DialKind::Stylesheet || !(200..300).contains(&status) {
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

/// Browser-generated request headers cannot be overridden by page scripts.
/// <https://fetch.spec.whatwg.org/#forbidden-request-header>
fn forbidden_page_header(name: &str, value: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("proxy-") || lower.starts_with("sec-") {
        return true;
    }
    if matches!(
        lower.as_str(),
        "accept-charset"
            | "accept-encoding"
            | "access-control-request-headers"
            | "access-control-request-method"
            | "connection"
            | "content-length"
            | "cookie"
            | "cookie2"
            | "date"
            | "dnt"
            | "expect"
            | "host"
            | "keep-alive"
            | "origin"
            | "referer"
            | "set-cookie"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "via"
    ) {
        return true;
    }
    matches!(
        lower.as_str(),
        "x-http-method" | "x-http-method-override" | "x-method-override"
    ) && value.split(',').any(|method| {
        matches!(
            method.trim().to_ascii_lowercase().as_str(),
            "connect" | "trace" | "track"
        )
    })
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
fn isomorphic_encode(value: &str) -> Vec<u8> {
    value
        .chars()
        .map(|character| u8::try_from(character).unwrap_or(0))
        .collect()
}

fn outbound_request(
    request: &DialRequest,
    initiator: &Url,
    deadline: Instant,
    sheets: &HashSet<Url>,
) -> Result<Request, DialFailure> {
    let url = Url::parse(&request.url).map_err(|_| DialFailure::Connect)?;
    let method = net::Method::parse(&request.method).map_err(|_| DialFailure::Connect)?;
    // https://fetch.spec.whatwg.org/#forbidden-method
    if request.kind == DialKind::JsFetch
        && matches!(
            method.as_str().to_ascii_uppercase().as_str(),
            "CONNECT" | "TRACE" | "TRACK"
        )
    {
        return Err(DialFailure::Connect);
    }
    // A `no-cors` request may only use CORS-safelisted methods and headers:
    // the page-side `request-no-cors` guard is advisory, so the network
    // boundary enforces it here. Otherwise a page could send an unpreflighted,
    // state-changing cross-origin request with an arbitrary method and headers
    // (<https://fetch.spec.whatwg.org/#concept-headers-guard>).
    let no_cors = request.kind == DialKind::JsFetch
        && request
            .cors
            .as_ref()
            .is_some_and(|cors| cors.mode == net::cors::RequestMode::NoCors);
    if no_cors && !net::cors::is_cors_safelisted_method(&method) {
        return Err(DialFailure::Connect);
    }
    let mut outbound = Request::new(method, url);
    apply_cors(request, initiator, &mut outbound);
    if let Some(content_type) = &request.content_type {
        let content_type = isomorphic_encode(content_type);
        let _ = outbound.headers.insert("Content-Type", content_type);
    }
    for (name, value) in &request.headers {
        if request.kind == DialKind::JsFetch && forbidden_page_header(name, value) {
            continue;
        }
        let value = isomorphic_encode(value);
        if no_cors && !net::cors::is_no_cors_safelisted_request_header(name, &value) {
            continue;
        }
        outbound
            .headers
            .insert(name, value)
            .map_err(|_| DialFailure::Connect)?;
    }
    let destination = match request.kind {
        DialKind::JsFetch => "empty",
        DialKind::ClassicScript | DialKind::ModuleScript => "script",
        DialKind::Stylesheet => "style",
        DialKind::Image => "image",
        DialKind::Font => "font",
        DialKind::FrameLoad => "iframe",
        DialKind::CspReport => "report",
    };
    let _ = outbound
        .headers
        .insert("Sec-Fetch-Dest", destination.as_bytes().to_vec());
    if request.kind == DialKind::JsFetch {
        let mode = request
            .cors
            .as_ref()
            .map_or("cors", |cors| match cors.mode {
                net::cors::RequestMode::NoCors => "no-cors",
                net::cors::RequestMode::SameOrigin => "same-origin",
                _ => "cors",
            });
        let _ = outbound
            .headers
            .insert("Sec-Fetch-Mode", mode.as_bytes().to_vec());
    }
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

/// Copies a page dial's fetch parameters onto the `net` request. The origin
/// comes from the site-authorized initiator, so a compromised renderer cannot
/// claim an origin outside its renderer's site lock
/// (<https://fetch.spec.whatwg.org/#concept-request-origin>). It can still
/// name a different origin *within* that site, which shares the renderer
/// process; deriving the origin from a per-frame record the browser owns is a
/// known gap.
fn apply_cors(request: &DialRequest, initiator: &Url, outbound: &mut Request) {
    let origin = initiator.origin();
    outbound.cors = match request.cors {
        Some(cors) => net::cors::CorsSettings {
            mode: cors.mode,
            credentials: cors.credentials,
            redirect: cors.redirect,
            origin,
        },
        None => net::cors::CorsSettings {
            mode: net::cors::RequestMode::Navigate,
            credentials: net::cors::CredentialsMode::Include,
            redirect: net::cors::RedirectMode::Follow,
            origin,
        },
    };
}

struct StreamCache {
    store: bool,
    stored: DialOutcome,
    cache: ResponseCache,
    cache_key: CacheKey,
    request_headers: Vec<(String, String)>,
}

fn spawn_stream_body(
    response: net::Response,
    integrity: Option<String>,
    write: StreamCache,
    tx: tokio::sync::mpsc::UnboundedSender<StreamMessage>,
) {
    let StreamCache {
        store,
        stored,
        cache,
        cache_key,
        request_headers,
    } = write;
    tokio::spawn(async move {
        let mut body = response.into_body().await;
        let mut total = 0usize;
        let mut buffer = Vec::new();
        loop {
            match body.read_chunk().await {
                Ok(Some(chunk)) => {
                    total = total.saturating_add(chunk.len());
                    if total > renderer::MAX_RESPONSE_BODY_BYTES {
                        let _ = tx.send(StreamMessage::Error(DialFailure::Connect));
                        break;
                    }
                    if integrity.is_some() || store {
                        buffer.extend_from_slice(&chunk);
                    }
                    if tx.send(StreamMessage::Chunk(chunk)).is_err() {
                        break;
                    }
                }
                Ok(None) => {
                    if let Some(integrity) = integrity.as_deref()
                        && !renderer::integrity_verifies(integrity, &buffer)
                    {
                        let _ = tx.send(StreamMessage::Error(DialFailure::Connect));
                    } else {
                        if store {
                            let mut stored = stored;
                            stored.body = Ok(buffer);
                            stored.stream = None;
                            let mut cache = cache
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            insert_cached(
                                &mut cache,
                                cache_key,
                                CachedResponse {
                                    outcome: stored,
                                    stored_at: Instant::now(),
                                    request_headers,
                                },
                            );
                        }
                        let _ = tx.send(StreamMessage::End);
                    }
                    break;
                }
                Err(_) => {
                    let _ = tx.send(StreamMessage::Error(DialFailure::Connect));
                    break;
                }
            }
        }
    });
}

async fn read_response_body(
    request: &DialRequest,
    response: net::Response,
    response_type: net::cors::ResponseType,
) -> Result<Result<Vec<u8>, renderer::BodyError>, DialFailure> {
    if !request.read_body || response_type.is_opaque() {
        return Ok(Ok(Vec::new()));
    }
    match response
        .into_body()
        .await
        .bytes(renderer::MAX_RESPONSE_BODY_BYTES)
        .await
    {
        Ok(body) => {
            if let Some(integrity) = request.integrity.as_deref()
                && !renderer::integrity_verifies(integrity, &body)
            {
                return Err(DialFailure::Connect);
            }
            Ok(Ok(body))
        }
        Err(net::NetError::Decode(_)) => Ok(Err(renderer::BodyError)),
        Err(error) => Err(dial_failure(&error)),
    }
}

fn trace_dial_result(
    request: &DialRequest,
    started: Instant,
    result: &Result<DialOutcome, DialFailure>,
) {
    match result {
        Ok(outcome) => logging::trace!(
            target: "browser::net",
            "request done {:?} {} -> {} in {:?}",
            request.kind,
            request.url,
            outcome.status,
            started.elapsed()
        ),
        Err(failure) => logging::trace!(
            target: "browser::net",
            "request failed {:?} {} in {:?}: {failure:?}",
            request.kind,
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
        body: Ok(Vec::new()),
        response_type: net::cors::ResponseType::Basic,
        redirect_count: 0,
        referrer: None,
        stream: None,
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
    identity_header(request, "Upgrade-Insecure-Requests", "1");
    identity_header(
        request,
        "Accept",
        "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8",
    );
    identity_header(request, "Sec-Fetch-Site", "none");
    identity_header(request, "Sec-Fetch-Mode", "navigate");
    identity_header(request, "Sec-Fetch-User", "?1");
    identity_header(request, "Sec-Fetch-Dest", "document");
    if sends_default_ua_client_hints(url) {
        for (name, value) in [
            ("Sec-CH-UA", crate::SEC_CH_UA),
            ("Sec-CH-UA-Mobile", crate::SEC_CH_UA_MOBILE),
            ("Sec-CH-UA-Platform", crate::SEC_CH_UA_PLATFORM),
            (
                "Sec-CH-Prefers-Color-Scheme",
                crate::SEC_CH_PREFERS_COLOR_SCHEME,
            ),
        ] {
            identity_header(request, name, value);
        }
    }
}

fn identity_header(request: &mut Request, name: &str, value: &str) {
    request
        .headers
        .insert(name, value)
        .expect("Chrome navigation identity headers are static HTTP tokens with no CTL bytes");
}

fn sends_default_ua_client_hints(url: &Url) -> bool {
    if url.scheme() == "https" {
        return true;
    }
    match url.host() {
        Some(url::Host::Ipv4(addr)) => addr.is_loopback(),
        Some(url::Host::Ipv6(addr)) => addr.is_loopback(),
        Some(url::Host::Domain(host)) => {
            let host = host.to_ascii_lowercase();
            host == "localhost" || host.ends_with(".localhost")
        }
        None => false,
    }
}

/// Header values are byte sequences; each byte is exposed as the same code
/// point so the page never sees a replacement character
/// (<https://infra.spec.whatwg.org/#isomorphic-decode>).
fn isomorphic_decode(bytes: &[u8]) -> String {
    bytes.iter().map(|&byte| char::from(byte)).collect()
}

/// Every `Content-Type` value from the response as one string: the values
/// joined with `", "`, each byte the same code point
/// (<https://fetch.spec.whatwg.org/#concept-header-list-get-decode-split>,
/// <https://infra.spec.whatwg.org/#isomorphic-decode>). The renderer extracts
/// the MIME type from it
/// (<https://fetch.spec.whatwg.org/#concept-header-extract-mime-type>).
fn content_type_header(headers: &net::HeaderMap) -> Option<String> {
    let values: Vec<String> = headers
        .get_all("content-type")
        .map(isomorphic_decode)
        .collect();
    (!values.is_empty()).then(|| values.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn js_fetch(mode: net::cors::RequestMode, url: &str) -> DialRequest {
        DialRequest {
            kind: DialKind::JsFetch,
            url: url.to_owned(),
            initiator: "http://a.test/page".to_owned(),
            read_body: true,
            method: "GET".to_owned(),
            body: Vec::new(),
            content_type: None,
            headers: Vec::new(),
            referrer: net::Referrer::NoReferrer,
            referrer_url: "http://a.test/page".to_owned(),
            timeout_ms: 0,
            csp: None,
            cors: Some(renderer::CorsRequest {
                mode,
                credentials: net::cors::CredentialsMode::SameOrigin,
                redirect: net::cors::RedirectMode::Follow,
            }),
            keepalive: false,
            body_len: 0,
            integrity: None,
            cache: None,
            stream_body: false,
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
                body: Ok(Vec::new()),
                response_type,
                redirect_count: 0,
                referrer: None,
                stream: None,
            },
            stored_at: Instant::now(),
            request_headers: Vec::new(),
        }
    }

    #[test]
    fn cache_partitions_by_document_and_initiator_origin() {
        let request = js_fetch(net::cors::RequestMode::Cors, "http://a.test/data");
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
        let request = js_fetch(net::cors::RequestMode::NoCors, "http://b.test/data");
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
        let same_origin = js_fetch(net::cors::RequestMode::NoCors, "http://a.test/data");
        assert!(cache_usable(
            &same_origin,
            &initiator,
            &cached(net::cors::ResponseType::Basic)
        ));
    }

    #[test]
    fn cached_body_still_needs_to_satisfy_integrity_metadata() {
        let mut request = js_fetch(net::cors::RequestMode::Cors, "http://a.test/data");
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
        for index in 0..=CACHE_MAX_ENTRIES {
            let mut request = js_fetch(net::cors::RequestMode::Cors, "http://a.test/data");
            request.url = format!("http://a.test/{index}");
            let key = cache_key(
                &request,
                &Url::parse("http://a.test/page").expect("page URL"),
                RendererAssignmentId::new(1),
            );
            let mut entry = cached(net::cors::ResponseType::Basic);
            entry.stored_at = Instant::now() + Duration::from_secs(index as u64);
            insert_cached(&mut cache, key, entry);
        }
        assert_eq!(cache.len(), CACHE_MAX_ENTRIES);
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
    fn default_ua_client_hints_are_limited_to_trustworthy_urls() {
        assert!(sends_default_ua_client_hints(
            &Url::parse("https://example.com/").expect("https")
        ));
        assert!(!sends_default_ua_client_hints(
            &Url::parse("http://example.com/").expect("http")
        ));
        assert!(sends_default_ua_client_hints(
            &Url::parse("http://127.0.0.1/").expect("loopback")
        ));
        assert!(sends_default_ua_client_hints(
            &Url::parse("http://localhost/").expect("localhost")
        ));
    }
}
