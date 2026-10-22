use std::borrow::Cow;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use crate::InitiatorKind;
use crate::error::{LimitExceeded, NetError, ProtocolError, TimeoutKind, TransportError};
use crate::protocol::{HeaderMap, Method};
use crate::resolve::HostMap;
use crate::transport::{CallBudget, HttpEngine, Proxy, basic_authorization, within};
use cookies::{CookieJar, CookieOp, RetrievalKind};
use http_body_util::BodyExt as _;
use hyper::header::{self, HeaderValue};
use percent_encoding::percent_decode_str;
use url::Url;

const DEFAULT_MAX_REDIRECTS: u32 = 20;

/// Wall and monotonic clock reads used by one [`Agent`].
///
/// The default reads the system clocks. Embedders and tests pass their own
/// functions so cookie expiry and every timeout path stay deterministic.
#[derive(Clone, Copy)]
pub struct Clock {
    wall: fn() -> SystemTime,
    monotonic: fn() -> Instant,
}

impl Clock {
    /// Builds a clock from a wall-clock read and a monotonic read.
    #[must_use]
    pub fn new(wall: fn() -> SystemTime, monotonic: fn() -> Instant) -> Self {
        Self { wall, monotonic }
    }

    /// Current wall-clock time.
    #[must_use]
    pub fn now(self) -> SystemTime {
        (self.wall)()
    }

    /// Current monotonic instant.
    #[must_use]
    pub fn monotonic(self) -> Instant {
        (self.monotonic)()
    }
}

impl Default for Clock {
    fn default() -> Self {
        Self {
            wall: SystemTime::now,
            monotonic: Instant::now,
        }
    }
}

impl std::fmt::Debug for Clock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Clock")
    }
}

/// Constructor inputs for [`Agent::new`]. `user_agent` must be a valid field
/// value, each `resolve` entry must be `PATTERN=ADDR`, and each `tls_cas`
/// entry must be PEM. `proxy` is already parsed, so credentials never exist as
/// a plain configuration string here.
///
/// Debug output records whether a proxy is configured, not its URI or credentials.
#[derive(Clone)]
pub struct AgentOptions {
    /// Default `User-Agent` for requests that do not set that header themselves.
    /// An invalid field value makes [`Agent::new`] fail rather than dropping the
    /// header from every request.
    pub user_agent: Option<String>,
    /// Cap on the whole call, including redirects.
    pub timeout_global: Option<Duration>,
    /// Cap on a single hop.
    pub timeout_per_call: Option<Duration>,
    /// Maximum redirect hops. `0` returns the redirect response without following.
    pub max_redirects: u32,
    /// HTTP CONNECT proxy, parsed at the CLI or embedding boundary.
    pub proxy: Option<Proxy>,
    /// Wall and monotonic clock reads.
    pub clock: Clock,
    /// `--resolve=PATTERN=ADDR` rewrites, first match wins. `PATTERN` is an
    /// exact host or a `*` glob; `ADDR` is an IPv4 literal or `fail`.
    pub resolve: Vec<String>,
    /// PEM-encoded certificate authorities to trust in addition to the
    /// platform's, e.g. a private test CA. Repeatable. A bad entry is
    /// reported as `CA #n`, counting from 1.
    pub tls_cas: Vec<Vec<u8>>,
}

impl std::fmt::Debug for AgentOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentOptions")
            .field("has_proxy", &self.proxy.is_some())
            .field("timeout_global", &self.timeout_global)
            .field("timeout_per_call", &self.timeout_per_call)
            .field("max_redirects", &self.max_redirects)
            .field("has_resolve", &!self.resolve.is_empty())
            .field("extra_tls_cas", &self.tls_cas.len())
            .finish_non_exhaustive()
    }
}

impl Default for AgentOptions {
    fn default() -> Self {
        Self {
            user_agent: None,
            timeout_global: None,
            timeout_per_call: None,
            max_redirects: DEFAULT_MAX_REDIRECTS,
            proxy: None,
            clock: Clock::default(),
            resolve: Vec::new(),
            tls_cas: Vec::new(),
        }
    }
}

/// Shared HTTP client: connection pool, cookie jar, and proxy/timeout settings.
///
/// Cloning is cheap. Debug output does not include proxy credentials.
#[derive(Clone)]
pub struct Agent {
    pub(crate) engine: HttpEngine,
    ua: Option<HeaderValue>,
    max_redirects: u32,
    jar: Arc<Mutex<CookieJar>>,
    clock: Clock,
}

impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agent")
            .field("engine", &self.engine)
            .field("max_redirects", &self.max_redirects)
            .field("jar", &self.jar)
            .finish_non_exhaustive()
    }
}

impl Agent {
    /// Builds an agent from `options`, the only constructor. Every raw value is
    /// parsed here, before any request can see it.
    ///
    /// # Errors
    ///
    /// [`ProtocolError::InvalidUserAgent`] when `user_agent` is not a valid
    /// field value, [`ProtocolError::InvalidResolve`] when a `resolve` entry is
    /// not `PATTERN=ADDR`, or [`NetError::Transport`] when a `tls_cas` entry is
    /// not a PEM certificate.
    pub fn new(options: AgentOptions) -> Result<Agent, NetError> {
        let AgentOptions {
            user_agent,
            timeout_global,
            timeout_per_call,
            max_redirects,
            proxy,
            clock,
            resolve,
            tls_cas,
        } = options;
        let ua = user_agent
            .as_deref()
            .map(|raw| {
                HeaderValue::from_str(raw)
                    .map_err(|_| NetError::Protocol(ProtocolError::InvalidUserAgent))
            })
            .transpose()?;
        let mut host_map = HostMap::default();
        for spec in &resolve {
            host_map = host_map.with_spec(spec)?;
        }
        let mut parsed_cas = Vec::with_capacity(tls_cas.len());
        for (index, pem) in tls_cas.iter().enumerate() {
            parsed_cas.push(native_tls::Certificate::from_pem(pem).map_err(|error| {
                // 1-based: it matches the Nth `tls_cas` (or `--tls-ca`) entry,
                // so the caller can point at the bad file.
                NetError::Transport(TransportError::Tls(
                    format!("CA #{}: {error}", index + 1).into(),
                ))
            })?);
        }
        Ok(Agent {
            engine: HttpEngine::new(
                timeout_global,
                timeout_per_call,
                proxy,
                clock,
                host_map,
                &parsed_cas,
            ),
            ua,
            max_redirects,
            jar: Arc::new(Mutex::new(CookieJar::default())),
            clock,
        })
    }

    /// Borrows the live jar, recovering from a poisoned lock.
    fn jar(&self) -> MutexGuard<'_, CookieJar> {
        self.jar.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Operation for a `document.cookie` or `WebDriver` view of `url`.
    fn fetch_op<'a>(&self, url: &'a Url, kind: RetrievalKind) -> CookieOp<'a> {
        CookieOp {
            url,
            now: self.clock.now(),
            kind,
            initiator_kind: InitiatorKind::Fetch,
            method_is_safe: true,
            initiator: Some(url),
            cross_site_redirect: false,
        }
    }

    /// `document.cookie` getter for `uri`: non-HTTP cookies, semicolon-separated.
    #[must_use]
    pub fn cookies_for(&self, uri: &Url) -> String {
        self.jar()
            .cookie_string(self.fetch_op(uri, RetrievalKind::NonHttp))
    }

    /// `document.cookie` setter for `uri`. Invalid `Set-Cookie` lines are ignored.
    pub fn set_cookie(&self, value: &str, uri: &Url) {
        self.jar()
            .store(value, self.fetch_op(uri, RetrievalKind::NonHttp));
    }

    /// Stores one `Set-Cookie` line with HTTP-level rules, so an `HttpOnly`
    /// cookie is allowed where `document.cookie` would refuse it. Returns
    /// whether the jar stored the cookie
    /// (<https://w3c.github.io/webdriver/#add-cookie>).
    pub fn store_cookie_http(&self, value: &str, uri: &Url) -> bool {
        self.jar()
            .store(value, self.fetch_op(uri, RetrievalKind::Http))
    }

    /// Cookies visible to `uri`, including session and `HttpOnly` cookies.
    /// This is the `WebDriver` cookie view
    /// (<https://w3c.github.io/webdriver/#get-all-cookies>).
    #[must_use]
    pub fn cookie_records(&self, uri: &Url) -> Vec<crate::CookieRecord> {
        self.jar()
            .records_for(self.fetch_op(uri, RetrievalKind::Http))
    }

    /// Drops every cookie from the live jar
    /// (<https://w3c.github.io/webdriver/#delete-all-cookies>).
    pub fn clear_cookies(&self) {
        self.jar().clear();
    }

    /// Persistent cookies from the live jar. Session cookies are omitted.
    #[must_use]
    pub fn export_cookies(&self) -> Vec<crate::CookieRecord> {
        self.jar().snapshot()
    }

    /// Loads `records` into the live jar, replacing matching identities.
    pub fn import_cookies(&self, records: Vec<crate::CookieRecord>) {
        self.jar().restore(records, self.clock.now());
    }

    /// Adds the `Cookie` header and the default `User-Agent` for one hop.
    fn prepare_outbound(
        &self,
        headers: &mut HeaderMap,
        operation: CookieOp<'_>,
    ) -> Result<(), NetError> {
        let cookie = self.jar().cookie_string(operation);
        if !cookie.is_empty() {
            let value = match headers.value("cookie") {
                Some(existing) => {
                    format!("{}; {cookie}", String::from_utf8_lossy(existing.as_bytes()))
                }
                None => cookie,
            };
            let value = HeaderValue::from_str(&value)
                .map_err(|_| NetError::Protocol(ProtocolError::RejectedRequest))?;
            headers.set(header::COOKIE, value);
        }
        if headers.value("user-agent").is_none()
            && let Some(ua) = &self.ua
        {
            headers.append(header::USER_AGENT, ua.clone());
        }
        Ok(())
    }

    /// Runs the preflight for a non-safelisted cross-origin `cors` request
    /// (<https://fetch.spec.whatwg.org/#cors-preflight-fetch>). The preflight
    /// carries the same `Referer` and `User-Agent` as the request it gates.
    /// `origin_header` is the request's serialized origin: the document origin,
    /// or `null` once a redirect has tainted it.
    async fn cors_preflight(
        &self,
        settings: &crate::cors::CorsSettings,
        origin_header: &HeaderValue,
        hop: &Hop,
        headers: &HeaderMap,
        wire: &Url,
        budget: CallBudget,
    ) -> Result<(), NetError> {
        let method = &hop.method;
        let unsafe_names = crate::cors::unsafe_request_header_names(headers);
        if !crate::cors::needs_preflight(method, &unsafe_names) {
            return Ok(());
        }
        let mut preflight = crate::cors::preflight_headers(method, &unsafe_names, origin_header)
            .map_err(NetError::Protocol)?;
        let origin = origin_header
            .to_str()
            .map_err(|_| NetError::Protocol(ProtocolError::RejectedRequest))?;
        if let Some(referrer) = headers.value("referer") {
            preflight.append(header::REFERER, referrer.clone());
        }
        if let Some(ua) = &self.ua {
            preflight.append(header::USER_AGENT, ua.clone());
        }
        let (status, preflight_headers, body) = self
            .engine
            .send(&Method::OPTIONS, wire, &preflight, None, budget)
            .await?;
        drop(body);
        if !crate::cors::preflight_ok(
            status,
            &preflight_headers,
            origin,
            settings.credentials.includes(),
            method,
            &unsafe_names,
        ) {
            return Err(NetError::Protocol(ProtocolError::RejectedRequest));
        }
        Ok(())
    }

    /// Applies the request-side CORS rules for one hop: the preflight a
    /// `cors`-tainted hop needs, and the `Origin` header
    /// (<https://fetch.spec.whatwg.org/#http-fetch>,
    /// <https://fetch.spec.whatwg.org/#append-a-request-origin-header>).
    async fn apply_cors_request(
        &self,
        settings: &crate::cors::CorsSettings,
        hop: &Hop,
        wire: &Url,
        headers: &mut HeaderMap,
        budget: CallBudget,
    ) -> Result<(), NetError> {
        let serialized_origin = hop.serialized_origin(settings);
        let origin_header = HeaderValue::from_str(&serialized_origin)
            .map_err(|_| NetError::Protocol(ProtocolError::RejectedRequest))?;
        // Case 2 of main fetch sends same-origin, still-`basic` requests
        // straight to scheme fetch, so only a `cors`-tainted hop can need a
        // preflight (<https://fetch.spec.whatwg.org/#main-fetch>).
        if hop.response_type == crate::cors::ResponseType::Cors {
            self.cors_preflight(settings, &origin_header, hop, headers, wire, budget)
                .await?;
        }
        // `Origin` rides on every hop whose response tainting is `cors`, and
        // on every other request whose method is neither GET nor HEAD
        // (<https://fetch.spec.whatwg.org/#append-a-request-origin-header>).
        // A same-origin `cors` POST that survives a 307/308 redirect sends
        // `Origin` here, which `fetch/api/redirect/redirect-origin.any.js`
        // expects to be absent; step 3 of the algorithm does not exempt that
        // case, and Chrome and Firefox fail the same two subtests.
        if hop.response_type == crate::cors::ResponseType::Cors
            || !matches!(hop.method.as_str(), "GET" | "HEAD")
        {
            headers.set(header::ORIGIN, origin_header);
        }
        Ok(())
    }

    fn store_set_cookie_lines(
        &self,
        operation: CookieOp<'_>,
        lines: impl IntoIterator<Item = impl AsRef<str>>,
    ) {
        let mut jar = self.jar();
        let now = self.clock.now();
        for line in lines {
            jar.store(line.as_ref(), CookieOp { now, ..operation });
        }
    }

    /// Sends `request` and follows HTTP redirects per
    /// [HTTP redirect fetch](https://fetch.spec.whatwg.org/#http-redirect-fetch).
    ///
    /// Status codes are response data, not errors. The URL must be `http` or `https`.
    ///
    /// # Errors
    ///
    /// [`NetError::Protocol`] for a non-`http`/`https` URL, a rejected request
    /// (a failed CORS check or preflight, a redirect the CORS rules refuse, or
    /// a merged `Cookie` header that is not a valid field value), or an
    /// unusable `Location`. [`NetError::Limit`] when the redirect cap is
    /// exceeded. [`NetError::Transport`] when a hop fails.
    pub async fn send(&self, request: Request) -> Result<Response, NetError> {
        if !matches!(request.url.scheme(), "http" | "https") {
            return Err(NetError::Protocol(ProtocolError::RejectedRequest));
        }
        let dial = Dial {
            cors: &request.cors,
            initiator_kind: request.initiator_kind,
            initiator: request.initiator.as_ref(),
            on_redirect: request.on_redirect.as_ref(),
        };
        let mut hop = Hop {
            method: request.method,
            headers: request.headers,
            body: request.body,
            // Once any hop is cross-origin under `cors` (or `no-cors`), every
            // later hop keeps that tainting
            // (<https://fetch.spec.whatwg.org/#main-fetch>).
            response_type: crate::cors::ResponseType::Basic,
            previous_url: None,
            origin_tainted: false,
        };
        let mut url = request.url;
        let mut followed = 0u32;
        let mut cross_site_redirect = false;
        let mut budget = self.engine.budget_at();
        if let Some(deadline) = request.deadline {
            budget.global = Some(deadline);
        }

        loop {
            let response = self
                .exchange(&dial, &url, &mut hop, cross_site_redirect, budget)
                .await?;
            let hop_status = response.status();
            if self.max_redirects == 0 {
                cors_gate(&response, &dial, &hop)?;
                return Ok(finalize(
                    response,
                    hop.response_type,
                    dial.credentials_include(),
                ));
            }
            let next = match classify_response(response, &url, &dial, &hop)? {
                HopOutcome::Final(response) => return Ok(response),
                HopOutcome::Redirect { next } => next,
            };
            if followed == self.max_redirects {
                return Err(NetError::Limit(LimitExceeded::Redirect));
            }
            // `followed + 1` is the redirect count the next hop will carry.
            // A false gate refuses the hop before any bytes are sent.
            if let Some(gate) = dial.on_redirect
                && !gate(&next, followed + 1)
            {
                return Err(NetError::Protocol(ProtocolError::RejectedRequest));
            }
            cross_site_redirect |= !cookies::schemeful_same_site(&url, &next);
            apply_redirect_policy(hop_status, &url, &next, &mut hop);
            url = next;
            followed += 1;
        }
    }

    /// One HTTP hop: the response-tainting gate, the CORS request rules, the
    /// send, and the cookie store. Mutates `hop` only to acquire its CORS
    /// tainting (<https://fetch.spec.whatwg.org/#http-fetch>).
    async fn exchange(
        &self,
        dial: &Dial<'_>,
        url: &Url,
        hop: &mut Hop,
        cross_site_redirect: bool,
        budget: CallBudget,
    ) -> Result<Response, NetError> {
        let taint = hop_taint(dial.cors, url)?;
        if taint != crate::cors::ResponseType::Basic {
            hop.response_type = taint;
        }
        // The redirect-taint: once a hop crosses an origin that is neither
        // the previous URL's nor the request's own, the serialized origin
        // becomes `null` for this and every later hop
        // (<https://fetch.spec.whatwg.org/#concept-request-tainted-origin>).
        if let Some(previous) = hop.previous_url.as_ref()
            && previous.origin() != url.origin()
            && dial.cors.origin != previous.origin()
        {
            hop.origin_tainted = true;
        }
        hop.previous_url = Some(url.clone());
        if budget.is_expired() {
            return Err(NetError::Transport(TransportError::Timeout(
                budget.timeout_kind(TimeoutKind::Global),
            )));
        }
        let budget = budget.with_hop_start(self.engine.timeout_per_call);
        let mut wire = url.clone();
        wire.set_fragment(None);
        let cross_origin = dial.cors.origin != wire.origin();
        let operation = CookieOp {
            url,
            now: self.clock.now(),
            kind: RetrievalKind::Http,
            initiator_kind: dial.initiator_kind,
            method_is_safe: hop.method.is_safe(),
            initiator: dial.initiator,
            cross_site_redirect,
        };
        let mut hop_headers = hop.headers.clone();
        self.apply_cors_request(dial.cors, hop, &wire, &mut hop_headers, budget)
            .await?;
        self.prepare_outbound(&mut hop_headers, operation)?;
        // Credentials mode gates both the `Cookie` header and the
        // URL-credentials `Authorization` header
        // (<https://fetch.spec.whatwg.org/#concept-request-credentials-mode>,
        // <https://fetch.spec.whatwg.org/#http-network-or-cache-fetch>).
        let send_credentials = credentials_admitted(dial.cors.credentials, cross_origin);
        if send_credentials {
            apply_url_credentials(&mut hop_headers, url)?;
        } else {
            hop_headers.remove("cookie");
        }
        let (status, response_headers, reader) = self
            .engine
            .send(
                &hop.method,
                &wire,
                &hop_headers,
                hop.body.as_deref(),
                budget,
            )
            .await?;
        let response = Response {
            status,
            headers: response_headers,
            final_url: url.clone(),
            body: Body {
                inner: reader,
                budget,
            },
            filtering: hop.response_type,
        };
        // Set-Cookie must be read before CORS filtering removes it, and only
        // when credentials mode admits them for this origin.
        if send_credentials {
            self.store_set_cookie_lines(
                operation,
                response
                    .headers()
                    .get_all("set-cookie")
                    .filter_map(|v| std::str::from_utf8(v).ok()),
            );
        }
        Ok(response)
    }
}

/// Decides whether a redirect hop may be followed.
pub type RedirectGate = Arc<dyn Fn(&Url, u32) -> bool + Send + Sync>;

/// One outbound request as plain data. Set fields directly, then hand it to
/// [`Agent::send`].
///
/// Default [`InitiatorKind`] is [`InitiatorKind::Navigation`].
pub struct Request {
    /// Request method.
    pub method: Method,
    /// Absolute URL.
    pub url: Url,
    /// Outbound headers. [`HeaderMap::insert`] appends values with the same
    /// name (RFC 9110 §5.2); call [`HeaderMap::remove`] first to replace.
    pub headers: HeaderMap,
    /// Initiator class used for `SameSite` and (later) `Sec-Fetch-*`.
    pub initiator_kind: InitiatorKind,
    /// Document URL used as the `SameSite` initiator.
    pub initiator: Option<Url>,
    /// Request body bytes. Redirects that convert to GET drop this.
    pub body: Option<Vec<u8>>,
    /// Absolute deadline for this call, covering every redirect hop.
    /// Overrides [`AgentOptions::timeout_global`]; the per-call timeout still
    /// applies hop by hop.
    pub deadline: Option<Instant>,
    /// Fetch parameters: mode, credentials, redirect handling, and origin. The
    /// caller fills `origin` from the document it is fetching for, so a
    /// compromised renderer cannot claim another origin.
    pub cors: crate::cors::CorsSettings,
    /// Called with the next URL and the redirect count that hop will carry
    /// (`1` for the first redirect) before the hop is followed. `false`
    /// rejects the dial. Unset follows every redirect the cap allows.
    pub on_redirect: Option<RedirectGate>,
}

impl std::fmt::Debug for Request {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Request")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("headers", &self.headers)
            .field("initiator_kind", &self.initiator_kind)
            .field("initiator", &self.initiator)
            .field("body", &self.body)
            .field("deadline", &self.deadline)
            .field("cors", &self.cors)
            .field("on_redirect", &self.on_redirect.is_some())
            .finish()
    }
}

impl Request {
    /// A navigation request for `method` and absolute `url` with no headers,
    /// body, initiator, or deadline. Its origin starts as the target URL's
    /// origin; callers that know the initiator replace it.
    #[must_use]
    pub fn new(method: Method, url: Url) -> Self {
        let cors = crate::cors::CorsSettings {
            mode: crate::cors::RequestMode::Navigate,
            credentials: crate::cors::CredentialsMode::Include,
            redirect: crate::cors::RedirectMode::Follow,
            origin: url.origin(),
        };
        Self {
            method,
            url,
            headers: HeaderMap::new(),
            initiator_kind: InitiatorKind::default(),
            initiator: None,
            body: None,
            deadline: None,
            cors,
            on_redirect: None,
        }
    }
}

fn followable_location(status: u16, headers: &HeaderMap) -> Result<Option<&str>, NetError> {
    if !matches!(status, 301 | 302 | 303 | 307 | 308) {
        return Ok(None);
    }
    let Some(raw) = headers.get("location") else {
        return Ok(None);
    };
    let location =
        std::str::from_utf8(raw).map_err(|_| NetError::Protocol(ProtocolError::RejectedRequest))?;
    Ok(Some(location))
}

// https://fetch.spec.whatwg.org/#http-redirect-fetch
fn resolve_location(current: &Url, location: &str) -> Result<Url, NetError> {
    let mut next = current
        .join(location)
        .map_err(|_| NetError::Protocol(ProtocolError::RejectedRequest))?;
    if !matches!(next.scheme(), "http" | "https") {
        return Err(NetError::Protocol(ProtocolError::RejectedRequest));
    }
    if next.fragment().is_none()
        && let Some(fragment) = current.fragment()
    {
        next.set_fragment(Some(fragment));
    }
    Ok(next)
}

// https://fetch.spec.whatwg.org/#http-redirect-fetch
fn apply_redirect_policy(status: u16, current: &Url, next: &Url, hop: &mut Hop) {
    let post_to_get = matches!(status, 301 | 302) && hop.method == Method::POST;
    let see_other = status == 303 && hop.method != Method::GET && hop.method != Method::HEAD;
    if post_to_get || see_other {
        hop.method = Method::GET;
        hop.body = None;
        hop.headers.remove("content-encoding");
        hop.headers.remove("content-language");
        hop.headers.remove("content-location");
        hop.headers.remove("content-type");
        hop.headers.remove("content-length");
        hop.headers.remove("transfer-encoding");
    }
    if current.origin() != next.origin() {
        hop.headers.remove("authorization");
        hop.headers.remove("cookie");
        hop.headers.remove("host");
    }
    if next.scheme() != "https" && !loopback_http(next) {
        hop.headers.remove("sec-ch-ua");
        hop.headers.remove("sec-ch-ua-mobile");
        hop.headers.remove("sec-ch-ua-platform");
        hop.headers.remove("sec-ch-prefers-color-scheme");
    }
}

fn loopback_http(url: &Url) -> bool {
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

/// What the caller does after one hop.
enum HopOutcome {
    /// The response is the final result.
    Final(Response),
    /// The response redirects; fetch `next`.
    Redirect { next: Url },
}

/// Request-scoped values shared by every hop of one `send` call.
struct Dial<'a> {
    cors: &'a crate::cors::CorsSettings,
    initiator_kind: InitiatorKind,
    initiator: Option<&'a Url>,
    on_redirect: Option<&'a RedirectGate>,
}

impl Dial<'_> {
    /// Whether the credentials mode includes credentials, for the CORS check
    /// and response filtering
    /// (<https://fetch.spec.whatwg.org/#concept-request-credentials-mode>).
    fn credentials_include(&self) -> bool {
        self.cors.credentials.includes()
    }
}

/// The request state a redirect may rewrite, plus the sticky CORS tainting.
struct Hop {
    method: Method,
    headers: HeaderMap,
    body: Option<Vec<u8>>,
    response_type: crate::cors::ResponseType,
    /// The previous hop's URL, for the redirect-taint computation
    /// (<https://fetch.spec.whatwg.org/#concept-request-tainted-origin>).
    previous_url: Option<Url>,
    /// Whether the serialized request origin has collapsed to `null` because a
    /// hop crossed an origin that is neither the previous URL's nor the
    /// request's own
    /// (<https://fetch.spec.whatwg.org/#serializing-a-request-origin>).
    origin_tainted: bool,
}

impl Hop {
    /// The byte-serialized request origin CORS compares, `null` once a redirect
    /// has tainted the request
    /// (<https://fetch.spec.whatwg.org/#serializing-a-request-origin>). The
    /// request `Origin` header and the response CORS check both read it here.
    fn serialized_origin<'a>(&self, settings: &'a crate::cors::CorsSettings) -> Cow<'a, str> {
        if self.origin_tainted {
            Cow::Borrowed("null")
        } else {
            Cow::Owned(settings.origin.ascii_serialization())
        }
    }
}

/// The response tainting a hop acquires
/// (<https://fetch.spec.whatwg.org/#concept-request-response-tainting>). A
/// `same-origin` request across an origin boundary is a network error.
fn hop_taint(
    settings: &crate::cors::CorsSettings,
    url: &Url,
) -> Result<crate::cors::ResponseType, NetError> {
    if settings.origin == url.origin() {
        return Ok(crate::cors::ResponseType::Basic);
    }
    match settings.mode {
        crate::cors::RequestMode::Navigate => Ok(crate::cors::ResponseType::Basic),
        crate::cors::RequestMode::SameOrigin => {
            Err(NetError::Protocol(ProtocolError::RejectedRequest))
        }
        crate::cors::RequestMode::Cors => Ok(crate::cors::ResponseType::Cors),
        // A cross-origin `no-cors` request must follow redirects; any other
        // redirect mode is a network error
        // (<https://fetch.spec.whatwg.org/#main-fetch>).
        crate::cors::RequestMode::NoCors => {
            if settings.redirect == crate::cors::RedirectMode::Follow {
                Ok(crate::cors::ResponseType::Opaque)
            } else {
                Err(NetError::Protocol(ProtocolError::RejectedRequest))
            }
        }
    }
}

/// Whether a credentials mode admits credentials for a hop that is (or is
/// not) cross-origin
/// (<https://fetch.spec.whatwg.org/#concept-request-credentials-mode>).
fn credentials_admitted(mode: crate::cors::CredentialsMode, cross_origin: bool) -> bool {
    match mode {
        crate::cors::CredentialsMode::Omit => false,
        crate::cors::CredentialsMode::SameOrigin => !cross_origin,
        crate::cors::CredentialsMode::Include => true,
    }
}

/// The CORS check for a `cors`-tainted response
/// (<https://fetch.spec.whatwg.org/#cors-check>). Every hop, redirect or
/// final, must pass it.
fn cors_gate(response: &Response, dial: &Dial<'_>, hop: &Hop) -> Result<(), NetError> {
    if hop.response_type != crate::cors::ResponseType::Cors {
        return Ok(());
    }
    let origin = hop.serialized_origin(dial.cors);
    if !crate::cors::cors_check(response.headers(), &origin, dial.credentials_include()) {
        return Err(NetError::Protocol(ProtocolError::RejectedRequest));
    }
    Ok(())
}

/// Applies the response-side CORS rules for one hop, in spec order: the CORS
/// check, then the redirect handling (the credentials rule, the redirect
/// mode, and the final filtering)
/// (<https://fetch.spec.whatwg.org/#http-fetch>,
/// <https://fetch.spec.whatwg.org/#http-redirect-fetch>).
fn classify_response(
    response: Response,
    current_url: &Url,
    dial: &Dial<'_>,
    hop: &Hop,
) -> Result<HopOutcome, NetError> {
    cors_gate(&response, dial, hop)?;
    let Some(location) =
        followable_location(response.status(), response.headers())?.map(str::to_owned)
    else {
        return Ok(HopOutcome::Final(finalize(
            response,
            hop.response_type,
            dial.credentials_include(),
        )));
    };
    let next = resolve_location(current_url, &location)?;
    let settings = dial.cors;
    // A redirect to a URL that includes credentials is a network error
    // once the request is `cors`-tainted or would cross an origin
    // (<https://fetch.spec.whatwg.org/#http-redirect-fetch>, steps 9-10).
    let redirects_with_credentials = !next.username().is_empty() || next.password().is_some();
    if redirects_with_credentials
        && (hop.response_type == crate::cors::ResponseType::Cors
            || (settings.mode == crate::cors::RequestMode::Cors
                && settings.origin != next.origin()))
    {
        return Err(NetError::Protocol(ProtocolError::RejectedRequest));
    }
    match settings.redirect {
        crate::cors::RedirectMode::Error => {
            return Err(NetError::Protocol(ProtocolError::RejectedRequest));
        }
        crate::cors::RedirectMode::Manual => {
            return Ok(HopOutcome::Final(finalize(
                response,
                crate::cors::ResponseType::OpaqueRedirect,
                dial.credentials_include(),
            )));
        }
        crate::cors::RedirectMode::Follow => {}
    }
    Ok(HopOutcome::Redirect { next })
}

/// Applies a response's CORS filtering: `cors` exposes only safelisted and
/// explicitly exposed headers, and the opaque types expose none
/// (<https://fetch.spec.whatwg.org/#concept-filtered-response>).
fn finalize(
    mut response: Response,
    response_type: crate::cors::ResponseType,
    credentials_include: bool,
) -> Response {
    response.filtering = response_type;
    match response_type {
        crate::cors::ResponseType::Cors => {
            response.headers =
                crate::cors::exposed_response_headers(&response.headers, credentials_include);
        }
        crate::cors::ResponseType::Opaque | crate::cors::ResponseType::OpaqueRedirect => {
            response.headers = crate::cors::opaque_response_headers();
        }
        crate::cors::ResponseType::Basic => {}
    }
    response
}

fn apply_url_credentials(headers: &mut HeaderMap, url: &Url) -> Result<(), NetError> {
    // https://fetch.spec.whatwg.org/#http-network-or-cache-fetch
    if headers.value("authorization").is_some() || url.username().is_empty() {
        return Ok(());
    }
    let username = percent_decode_str(url.username()).decode_utf8_lossy();
    let password = url
        .password()
        .map(|password| percent_decode_str(password).decode_utf8_lossy())
        .unwrap_or_default();
    let value = basic_authorization(&username, &password);
    let value = HeaderValue::from_str(&value)
        .map_err(|_| NetError::Protocol(ProtocolError::RejectedRequest))?;
    headers.set(header::AUTHORIZATION, value);
    Ok(())
}

/// Streaming response body. Dropping it closes the socket.
pub struct Body {
    inner: hyper::body::Incoming,
    budget: CallBudget,
}

impl std::fmt::Debug for Body {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Body").finish_non_exhaustive()
    }
}

impl Body {
    /// Next chunk, or `None` at end of body.
    ///
    /// # Errors
    ///
    /// [`NetError::Transport`] when the socket read fails or the deadline
    /// expires.
    pub async fn read_chunk(&mut self) -> Result<Option<Vec<u8>>, NetError> {
        loop {
            let frame = within(self.budget, TimeoutKind::RecvBody, self.inner.frame()).await?;
            match frame {
                Some(Ok(frame)) => {
                    if let Ok(data) = frame.into_data() {
                        return Ok(Some(data.to_vec()));
                    }
                }
                Some(Err(error)) => {
                    return Err(NetError::Transport(TransportError::Io(
                        std::io::Error::other(error),
                    )));
                }
                None => return Ok(None),
            }
        }
    }

    /// Entire body, failing if it would exceed `limit` bytes.
    ///
    /// # Errors
    ///
    /// [`NetError::Limit`] when the body is larger than `limit`.
    /// [`NetError::Transport`] when a read fails.
    pub async fn bytes(mut self, limit: usize) -> Result<Vec<u8>, NetError> {
        let mut out = Vec::new();
        while let Some(chunk) = self.read_chunk().await? {
            if out.len() + chunk.len() > limit {
                return Err(NetError::Limit(LimitExceeded::Size(limit as u64)));
            }
            out.extend_from_slice(&chunk);
        }
        Ok(out)
    }
}

/// HTTP response after redirects. Status codes are data, including 4xx and 5xx.
#[derive(Debug)]
pub struct Response {
    status: u16,
    headers: HeaderMap,
    final_url: Url,
    body: Body,
    filtering: crate::cors::ResponseType,
}

impl Response {
    /// HTTP status code.
    #[must_use]
    pub fn status(&self) -> u16 {
        self.status
    }

    /// How the response was filtered: basic, CORS, or opaque
    /// (<https://fetch.spec.whatwg.org/#concept-response-type>).
    #[must_use]
    pub fn response_type(&self) -> crate::cors::ResponseType {
        self.filtering
    }

    /// Response fields, including each `Set-Cookie`.
    #[must_use]
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// URL after redirects. Fragments from the original request are preserved.
    #[must_use]
    pub fn final_url(&self) -> &Url {
        &self.final_url
    }

    /// Consumes the response and returns its body stream.
    #[must_use]
    pub fn into_body(self) -> Body {
        self.body
    }
}
