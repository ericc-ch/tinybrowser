//! Browser-owned live networking: one [`net::Agent`] behind a value-only handle.
//!
//! Tab coordinators receive [`TabNetworkHandle`]. They do not expose or own
//! [`net::Agent`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use net::{Agent, AgentOptions, InitiatorKind, Method, Request};
use renderer::{DialFailure, DialKind, DialOutcome, DialRequest};
use tokio::sync::Semaphore;
use tokio::sync::mpsc::UnboundedSender;
use url::Url;

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
}

/// A whole buffered response. The fetch body is already bounded by
/// `MAX_RESPONSE_BODY_BYTES`, so the cache holds at most that much per entry.
type ResponseCache = Arc<Mutex<HashMap<CacheKey, DialOutcome>>>;

fn cache_key(request: &DialRequest) -> CacheKey {
    CacheKey {
        method: request.method.clone(),
        url: request.url.clone(),
        credentials: request.cors.map_or_else(
            || "none".to_owned(),
            |cors| format!("{:?}", cors.credentials),
        ),
    }
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

fn should_store(request: &DialRequest, outcome: &DialOutcome, mode: &str) -> bool {
    request.kind == DialKind::JsFetch
        && request.method.eq_ignore_ascii_case("GET")
        && (200..300).contains(&outcome.status)
        && outcome.body.is_ok()
        && mode != "no-store"
        && !response_forbids_storage(outcome)
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
        mut cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<DialOutcome, DialFailure> {
        let started = Instant::now();
        let _keepalive = KeepaliveReservation::acquire(request)?;
        let mode = cache_mode(request).to_owned();
        if let Some(hit) = self.cached_response(request, &mode)? {
            return Ok(hit);
        }
        logging::trace!(
            target: "browser::net",
            "request start {:?} {}",
            request.kind,
            request.url
        );
        let call = async {
            let budget = if request.timeout_ms == 0 {
                PAGE_FETCH_TIMEOUT
            } else {
                PAGE_FETCH_TIMEOUT.min(Duration::from_millis(request.timeout_ms))
            };
            let deadline = Instant::now() + budget;
            let _global = Self::acquire(&self.permits.global, deadline).await?;
            let _tab = Self::acquire(&self.tab, deadline).await?;
            let mut outbound = outbound_request(request, initiator, deadline)?;
            let blocked_redirect = watch_redirects(request, initiator, &mut outbound);
            let response = match self.agent.send(outbound).await {
                Ok(response) => response,
                Err(error) => {
                    return refused_redirect(&blocked_redirect).ok_or_else(|| dial_failure(&error));
                }
            };
            let response_type = response.response_type();
            let status = response.status();
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
            let body = if request.read_body && !response_type.is_opaque() {
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
                        Ok(body)
                    }
                    Err(net::NetError::Decode(_)) => Err(renderer::BodyError),
                    Err(error) => return Err(dial_failure(&error)),
                }
            } else {
                Ok(Vec::new())
            };
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
            })
        };
        let result = tokio::select! {
            biased;
            _ = cancel.changed() => Err(DialFailure::Cancelled),
            result = call => result,
        };
        self.remember_response(request, &mode, &result);
        trace_dial_result(request, started, &result);
        result
    }

    fn cached_response(
        &self,
        request: &DialRequest,
        mode: &str,
    ) -> Result<Option<DialOutcome>, DialFailure> {
        if request.kind != DialKind::JsFetch || !matches!(mode, "force-cache" | "only-if-cached") {
            return Ok(None);
        }
        let hit = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&cache_key(request))
            .cloned();
        if hit.is_none() && mode == "only-if-cached" {
            return Err(DialFailure::Connect);
        }
        Ok(hit)
    }

    fn remember_response(
        &self,
        request: &DialRequest,
        mode: &str,
        result: &Result<DialOutcome, DialFailure>,
    ) {
        let Ok(outcome) = result else {
            return;
        };
        if !should_store(request, outcome, mode) {
            return;
        }
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(cache_key(request), outcome.clone());
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
    if let Some(referrer) = &request.referrer
        // Defense in depth: the renderer computes this per `referrer_for_policy`,
        // but the network boundary re-validates so a compromised renderer cannot
        // leak a cross-origin `Referer`.
        && is_safe_referrer(referrer, &request.url)
    {
        outbound
            .headers
            .insert("Referer", referrer.as_bytes())
            .map_err(|_| DialFailure::Connect)?;
    }
    if !request.body.is_empty() {
        outbound.body = Some(request.body.clone());
    }
    outbound.initiator_kind = InitiatorKind::Fetch;
    outbound.initiator = Some(initiator.clone());
    outbound.deadline = Some(deadline);
    Ok(outbound)
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

/// Network-boundary `Referer` check: rejects only a true downgrade (https
/// referrer to an http/ws target) or a referrer that is not a stripped
/// http(s) URL. The cross-origin send decision belongs to the renderer's
/// `referrer_for_policy`; this layer must not second-guess it (an http to
/// http cross-origin `Referer` is legitimate under several policies, and
/// WPT servers are plain http).
fn is_safe_referrer(referrer: &str, request_url: &str) -> bool {
    let (Ok(from), Ok(to)) = (Url::parse(referrer), Url::parse(request_url)) else {
        return false;
    };
    if !matches!(from.scheme(), "http" | "https") {
        return false;
    }
    if from.scheme() == "https" && matches!(to.scheme(), "http" | "ws") {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

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
    fn unsafe_referrer_survives_plain_http_cross_origin() {
        // WPT servers are plain http: an http referrer to another http
        // origin must keep its header; only a true downgrade is dropped.
        assert!(is_safe_referrer(
            "http://a.test/page",
            "http://b.test/other"
        ));
        assert!(is_safe_referrer(
            "https://a.test/page",
            "https://b.test/other"
        ));
        assert!(!is_safe_referrer(
            "https://a.test/page",
            "http://b.test/other"
        ));
        assert!(!is_safe_referrer(
            "data:text/plain,x",
            "http://b.test/other"
        ));
        assert!(!is_safe_referrer("not a url", "http://b.test/other"));
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
