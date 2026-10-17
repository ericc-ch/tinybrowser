use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use crate::InitiatorKind;
use crate::error::{LimitExceeded, NetError, ProtocolError, TimeoutKind, TransportError};
use crate::protocol::{HeaderMap, Method};
use crate::resolve::HostMap;
use crate::transport::{CallBudget, HttpEngine, basic_authorization, within};
use crate::websocket::{self, WebSocket};
use cookies::{CookieJar, CookieOp, RetrievalKind};
use http_body_util::BodyExt as _;
use url::Url;

const DEFAULT_MAX_REDIRECTS: u32 = 20;

/// Constructor inputs for [`Agent::new`]. Every field is raw and validated at
/// construction: `proxy` must be an `http://` URI with a host, each `resolve`
/// entry must be `PATTERN=ADDR`, and each `tls_cas` entry must be PEM.
///
/// Debug output records whether a proxy is configured, not its URI or credentials.
#[derive(Clone)]
pub struct AgentOptions {
    /// Default `User-Agent` for requests that do not set that header themselves.
    pub user_agent: Option<String>,
    /// Cap on the whole call, including redirects.
    pub timeout_global: Option<Duration>,
    /// Cap on a single hop.
    pub timeout_per_call: Option<Duration>,
    /// Maximum redirect hops. `0` returns the redirect response without following.
    pub max_redirects: u32,
    /// HTTP CONNECT proxy authority: an `http://` URI with a host.
    pub proxy: Option<String>,
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
            resolve: Vec::new(),
            tls_cas: Vec::new(),
        }
    }
}

/// Validated constructor inputs: proxy, resolve map, and TLS CAs are already
/// parsed, so assembly cannot fail.
struct AgentParts {
    user_agent: Option<String>,
    timeout_global: Option<Duration>,
    timeout_per_call: Option<Duration>,
    max_redirects: u32,
    proxy: Option<String>,
    host_map: HostMap,
    tls_cas: Vec<native_tls::Certificate>,
}

impl AgentParts {
    fn assemble(self) -> Agent {
        Agent {
            engine: HttpEngine::new(
                self.timeout_global,
                self.timeout_per_call,
                self.proxy,
                self.host_map,
                &self.tls_cas,
            ),
            ua: self.user_agent,
            max_redirects: self.max_redirects,
            jar: Arc::new(Mutex::new(CookieJar::default())),
            now: SystemTime::now,
        }
    }
}

impl AgentOptions {
    /// Parses every raw field, reporting the first invalid value.
    fn into_parts(self) -> Result<AgentParts, NetError> {
        let proxy = self.proxy.as_deref().map(parse_proxy).transpose()?;
        let mut host_map = HostMap::default();
        for spec in &self.resolve {
            host_map = host_map.with_spec(spec)?;
        }
        let mut tls_cas = Vec::with_capacity(self.tls_cas.len());
        for (index, pem) in self.tls_cas.iter().enumerate() {
            tls_cas.push(native_tls::Certificate::from_pem(pem).map_err(|error| {
                // 1-based: it matches the Nth `tls_cas` (or `--tls-ca`) entry,
                // so the caller can point at the bad file.
                NetError::Transport(TransportError::Tls(
                    format!("CA #{}: {error}", index + 1).into(),
                ))
            })?);
        }
        Ok(AgentParts {
            user_agent: self.user_agent,
            timeout_global: self.timeout_global,
            timeout_per_call: self.timeout_per_call,
            max_redirects: self.max_redirects,
            proxy,
            host_map,
            tls_cas,
        })
    }
}

fn parse_proxy(authority: &str) -> Result<String, NetError> {
    let parsed = Url::parse(authority).map_err(|_| invalid_proxy())?;
    if parsed.scheme() != "http" || parsed.host_str().is_none() {
        return Err(invalid_proxy());
    }
    Ok(authority.to_owned())
}

fn invalid_proxy() -> NetError {
    NetError::Protocol(ProtocolError::InvalidProxy)
}

/// Shared HTTP client: connection pool, cookie jar, and proxy/timeout settings.
///
/// Cloning is cheap. Debug output does not include proxy credentials.
#[derive(Clone)]
pub struct Agent {
    pub(crate) engine: HttpEngine,
    ua: Option<String>,
    max_redirects: u32,
    jar: Arc<Mutex<CookieJar>>,
    now: fn() -> SystemTime,
}

impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agent")
            .field("max_redirects", &self.max_redirects)
            .field("has_proxy", &self.engine.proxy.is_some())
            .field("jar", &self.jar)
            .finish_non_exhaustive()
    }
}

impl Agent {
    /// Builds an agent from `options`, the only constructor. The raw `proxy`,
    /// `resolve`, and `tls_cas` values are validated here before assembly.
    ///
    /// # Errors
    ///
    /// [`ProtocolError::InvalidProxy`] when `proxy` is not an `http://` URI with
    /// a host, [`ProtocolError::InvalidResolve`] when a `resolve` entry is not
    /// `PATTERN=ADDR`, or [`NetError::Transport`] when a `tls_cas` entry is not
    /// a PEM certificate.
    pub fn new(options: AgentOptions) -> Result<Agent, NetError> {
        Ok(options.into_parts()?.assemble())
    }

    /// Borrows the live jar, recovering from a poisoned lock.
    fn jar(&self) -> MutexGuard<'_, CookieJar> {
        self.jar.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Operation for a `document.cookie` or `WebDriver` view of `url`.
    fn fetch_op<'a>(&self, url: &'a Url, kind: RetrievalKind) -> CookieOp<'a> {
        CookieOp {
            url,
            now: (self.now)(),
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
        self.jar().restore(records, (self.now)());
    }

    pub(crate) fn prepare_outbound(
        &self,
        headers: &mut HeaderMap,
        url: &Url,
        initiator_kind: InitiatorKind,
        method: &Method,
        initiator: Option<&Url>,
        cross_site_redirect: bool,
    ) -> Result<(), NetError> {
        let cookie = self.jar().cookie_string(CookieOp {
            url,
            now: (self.now)(),
            kind: RetrievalKind::Http,
            initiator_kind,
            method_is_safe: method.is_safe(),
            initiator,
            cross_site_redirect,
        });
        if !cookie.is_empty() {
            let value = match headers.get("cookie") {
                Some(existing) => format!("{}; {cookie}", String::from_utf8_lossy(existing)),
                None => cookie,
            };
            headers.remove("cookie");
            headers
                .insert("Cookie", value)
                .map_err(|_| NetError::Protocol(ProtocolError::RejectedRequest))?;
        }
        if headers.get("user-agent").is_none()
            && let Some(ua) = &self.ua
        {
            let _ = headers.insert("User-Agent", ua.as_bytes());
        }
        if initiator_kind == InitiatorKind::WsHandshake
            && let Some(document) = initiator
        {
            let origin = document.origin().ascii_serialization();
            headers.remove("origin");
            let _ = headers.insert("Origin", origin.as_bytes());
        }
        Ok(())
    }

    /// Runs the preflight for a non-safelisted cross-origin `cors` request
    /// (<https://fetch.spec.whatwg.org/#cors-preflight-fetch>). The preflight
    /// carries the same `Referer` and `User-Agent` as the request it gates.
    /// `origin` is the request's byte-serialized origin: the document origin,
    /// or `null` once a redirect has tainted it.
    async fn cors_preflight(
        &self,
        settings: &crate::cors::CorsSettings,
        origin: &str,
        method: &Method,
        headers: &HeaderMap,
        wire: &Url,
        budget: CallBudget,
    ) -> Result<(), NetError> {
        let unsafe_names = crate::cors::unsafe_request_header_names(headers);
        if !crate::cors::needs_preflight(method, &unsafe_names) {
            return Ok(());
        }
        let mut preflight = crate::cors::preflight_headers(method, &unsafe_names, origin);
        if let Some(referrer) = headers.get("referer") {
            let _ = preflight.insert("Referer", referrer);
        }
        if let Some(ua) = &self.ua {
            let _ = preflight.insert("User-Agent", ua.as_bytes());
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
        settings: Option<&crate::cors::CorsSettings>,
        hop: &Hop,
        wire: &Url,
        headers: &mut HeaderMap,
        budget: CallBudget,
    ) -> Result<(), NetError> {
        let Some(settings) = settings else {
            return Ok(());
        };
        // A redirect-tainted request serializes its origin as `null` for the
        // rest of the chain, and both the request `Origin` header and the
        // response CORS check compare that serialization
        // (<https://fetch.spec.whatwg.org/#serializing-a-request-origin>).
        let serialized_origin = if hop.origin_tainted {
            "null"
        } else {
            settings.origin.as_str()
        };
        // Case 2 of main fetch sends same-origin, still-`basic` requests
        // straight to scheme fetch, so only a `cors`-tainted hop can need a
        // preflight (<https://fetch.spec.whatwg.org/#main-fetch>).
        if hop.response_type == crate::cors::ResponseType::Cors {
            self.cors_preflight(
                settings,
                serialized_origin,
                &hop.method,
                headers,
                wire,
                budget,
            )
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
            append_origin(headers, serialized_origin);
        }
        Ok(())
    }

    pub(crate) fn store_set_cookie_lines(
        &self,
        url: &Url,
        initiator_kind: InitiatorKind,
        method: &Method,
        initiator: Option<&Url>,
        cross_site_redirect: bool,
        lines: impl IntoIterator<Item = impl AsRef<str>>,
    ) {
        let now = (self.now)();
        let mut jar = self.jar();
        for line in lines {
            jar.store(
                line.as_ref(),
                CookieOp {
                    url,
                    now,
                    kind: RetrievalKind::Http,
                    initiator_kind,
                    method_is_safe: method.is_safe(),
                    initiator,
                    cross_site_redirect,
                },
            );
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
            cors: request.cors.as_ref(),
            initiator_kind: request.initiator_kind,
            initiator: request.initiator.as_ref(),
            same_origin: request.same_origin.as_ref(),
            on_redirect: request.on_redirect.as_ref(),
            credentials_include: request
                .cors
                .as_ref()
                .is_some_and(|settings| settings.credentials.includes()),
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
        let mut budget = self.engine.budget_at(Instant::now());
        if let Some(deadline) = request.deadline {
            budget.global = Some(deadline);
        }

        loop {
            let response = self
                .exchange(&dial, &url, &mut hop, cross_site_redirect, budget)
                .await?;
            let hop_status = response.status();
            let response_type = hop.response_type;
            let origin_tainted = hop.origin_tainted;
            if self.max_redirects == 0 {
                return filtered(
                    response,
                    dial.cors,
                    response_type,
                    dial.credentials_include,
                    origin_tainted,
                );
            }
            let next = match classify_response(
                response,
                &url,
                dial.cors,
                response_type,
                dial.credentials_include,
                origin_tainted,
            )? {
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

    /// One HTTP hop: the same-origin gate, the CORS request rules, the send,
    /// and the cookie store. Mutates `hop` only to acquire its CORS tainting
    /// (<https://fetch.spec.whatwg.org/#http-fetch>).
    async fn exchange(
        &self,
        dial: &Dial<'_>,
        url: &Url,
        hop: &mut Hop,
        cross_site_redirect: bool,
        budget: CallBudget,
    ) -> Result<Response, NetError> {
        if dial
            .same_origin
            .is_some_and(|origin| url.origin() != *origin)
        {
            return Err(NetError::Protocol(ProtocolError::RejectedRequest));
        }
        let taint = hop_taint(dial.cors, url)?;
        if taint != crate::cors::ResponseType::Basic {
            hop.response_type = taint;
        }
        // The redirect-taint: once a hop crosses an origin that is neither
        // the previous URL's nor the request's own, the serialized origin
        // becomes `null` for this and every later hop
        // (<https://fetch.spec.whatwg.org/#concept-request-tainted-origin>).
        if let (Some(settings), Some(previous)) = (dial.cors, hop.previous_url.as_ref())
            && previous.origin() != url.origin()
            && settings.origin != previous.origin().ascii_serialization()
        {
            hop.origin_tainted = true;
        }
        hop.previous_url = Some(url.clone());
        if budget.is_expired() {
            return Err(NetError::Transport(TransportError::Timeout(
                budget.timeout_kind(TimeoutKind::Global),
            )));
        }
        let budget = budget.with_hop_start(self.engine.timeout_per_call, Instant::now());
        let mut wire = url.clone();
        wire.set_fragment(None);
        let cross_origin = dial
            .cors
            .is_some_and(|settings| settings.origin != wire.origin().ascii_serialization());
        let mut hop_headers = hop.headers.clone();
        self.apply_cors_request(dial.cors, hop, &wire, &mut hop_headers, budget)
            .await?;
        self.prepare_outbound(
            &mut hop_headers,
            url,
            dial.initiator_kind,
            &hop.method,
            dial.initiator,
            cross_site_redirect,
        )?;
        // Credentials mode gates both the `Cookie` header and the
        // URL-credentials `Authorization` header
        // (<https://fetch.spec.whatwg.org/#concept-request-credentials-mode>,
        // <https://fetch.spec.whatwg.org/#http-network-or-cache-fetch>).
        let send_credentials = dial
            .cors
            .is_none_or(|settings| credentials_admitted(settings.credentials, cross_origin));
        if send_credentials {
            apply_url_credentials(&mut hop_headers, url);
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
        let response = Response::from_parts(
            status,
            response_headers,
            reader,
            url.clone(),
            budget,
            hop.response_type,
        );
        // Set-Cookie must be read before CORS filtering removes it, and only
        // when credentials mode admits them for this origin.
        if send_credentials {
            self.store_set_cookie_lines(
                url,
                dial.initiator_kind,
                &hop.method,
                dial.initiator,
                cross_site_redirect,
                response
                    .headers()
                    .get_all("set-cookie")
                    .filter_map(|v| std::str::from_utf8(v).ok()),
            );
        }
        Ok(response)
    }

    /// WebSocket handshake for `request`. The URL must be `ws` or `wss`.
    ///
    /// # Errors
    ///
    /// [`NetError::Protocol`] for a non-WebSocket URL, a merged `Cookie` header
    /// that is not a valid field value, or a failed handshake.
    /// [`NetError::Transport`] when the dial or TLS handshake fails.
    pub async fn upgrade(&self, request: Request) -> Result<WebSocket, NetError> {
        if !matches!(request.url.scheme(), "ws" | "wss") {
            return Err(NetError::Protocol(ProtocolError::RejectedRequest));
        }
        let method = Method::GET;
        let initiator_kind = InitiatorKind::WsHandshake;
        let mut headers = request.headers;
        self.prepare_outbound(
            &mut headers,
            &request.url,
            initiator_kind,
            &method,
            request.initiator.as_ref(),
            false,
        )?;
        websocket::connect(
            self,
            &request.url,
            &headers,
            initiator_kind,
            &method,
            request.initiator.as_ref(),
            request.deadline,
        )
        .await
    }
}

/// Decides whether a redirect hop may be followed.
pub type RedirectGate = Arc<dyn Fn(&Url, u32) -> bool + Send + Sync>;

/// One outbound request as plain data. Set fields directly, then hand it to
/// [`Agent::send`] (`http`/`https`) or [`Agent::upgrade`] (`ws`/`wss`).
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
    /// Document URL used as the `SameSite` initiator and WebSocket `Origin`.
    pub initiator: Option<Url>,
    /// Request body bytes. Redirects that convert to GET drop this.
    pub body: Option<Vec<u8>>,
    /// Absolute deadline for this call, covering every redirect hop.
    /// Overrides [`AgentOptions::timeout_global`]; the per-call timeout still
    /// applies hop by hop.
    pub deadline: Option<Instant>,
    /// When set, reject any initial or redirected hop outside this origin
    /// before sending headers or body. Navigation and subresources leave it unset.
    pub same_origin: Option<url::Origin>,
    /// CORS protocol parameters for a cross-origin `fetch`. The caller fills
    /// `origin` from the document it is fetching for, so a compromised
    /// renderer cannot claim another origin. `None` keeps the legacy behaviour
    /// where `same_origin` alone gates the request.
    pub cors: Option<crate::cors::CorsSettings>,
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
            .field("same_origin", &self.same_origin)
            .field("has_cors", &self.cors.is_some())
            .field("on_redirect", &self.on_redirect.is_some())
            .finish()
    }
}

impl Request {
    /// A request for `method` and absolute `url` with no headers, body,
    /// initiator, or deadline.
    #[must_use]
    pub fn new(method: Method, url: Url) -> Self {
        Self {
            method,
            url,
            headers: HeaderMap::new(),
            initiator_kind: InitiatorKind::default(),
            initiator: None,
            body: None,
            deadline: None,
            same_origin: None,
            cors: None,
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
    cors: Option<&'a crate::cors::CorsSettings>,
    initiator_kind: InitiatorKind,
    initiator: Option<&'a Url>,
    same_origin: Option<&'a url::Origin>,
    on_redirect: Option<&'a RedirectGate>,
    credentials_include: bool,
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

/// The response tainting a hop acquires
/// (<https://fetch.spec.whatwg.org/#concept-request-response-tainting>). A
/// `same-origin` request across an origin boundary is a network error.
fn hop_taint(
    cors: Option<&crate::cors::CorsSettings>,
    url: &Url,
) -> Result<crate::cors::ResponseType, NetError> {
    let Some(settings) = cors else {
        return Ok(crate::cors::ResponseType::Basic);
    };
    if settings.origin == url.origin().ascii_serialization() {
        return Ok(crate::cors::ResponseType::Basic);
    }
    match settings.mode {
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
fn cors_gate(
    response: &Response,
    cors: Option<&crate::cors::CorsSettings>,
    response_type: crate::cors::ResponseType,
    credentials_include: bool,
    origin_tainted: bool,
) -> Result<(), NetError> {
    if response_type != crate::cors::ResponseType::Cors {
        return Ok(());
    }
    let Some(settings) = cors else {
        return Err(NetError::Protocol(ProtocolError::RejectedRequest));
    };
    // The CORS check compares against the byte-serialized origin, which is
    // `null` once a redirect has tainted the request
    // (<https://fetch.spec.whatwg.org/#concept-cors-check>).
    let origin = if origin_tainted {
        "null"
    } else {
        settings.origin.as_str()
    };
    if !crate::cors::cors_check(response.headers(), origin, credentials_include) {
        return Err(NetError::Protocol(ProtocolError::RejectedRequest));
    }
    Ok(())
}

/// Applies the CORS check and the response filtering for one hop
/// (<https://fetch.spec.whatwg.org/#http-fetch>).
fn filtered(
    response: Response,
    cors: Option<&crate::cors::CorsSettings>,
    response_type: crate::cors::ResponseType,
    credentials_include: bool,
    origin_tainted: bool,
) -> Result<Response, NetError> {
    cors_gate(
        &response,
        cors,
        response_type,
        credentials_include,
        origin_tainted,
    )?;
    Ok(finalize(response, response_type, credentials_include))
}

/// Applies the response-side CORS rules for one hop, in spec order: the CORS
/// check, then the redirect handling (the credentials rule, the redirect
/// mode, and the final filtering)
/// (<https://fetch.spec.whatwg.org/#http-fetch>,
/// <https://fetch.spec.whatwg.org/#http-redirect-fetch>).
fn classify_response(
    response: Response,
    current_url: &Url,
    cors: Option<&crate::cors::CorsSettings>,
    response_type: crate::cors::ResponseType,
    credentials_include: bool,
    origin_tainted: bool,
) -> Result<HopOutcome, NetError> {
    cors_gate(
        &response,
        cors,
        response_type,
        credentials_include,
        origin_tainted,
    )?;
    let Some(location) =
        followable_location(response.status(), response.headers())?.map(str::to_owned)
    else {
        return Ok(HopOutcome::Final(finalize(
            response,
            response_type,
            credentials_include,
        )));
    };
    let next = resolve_location(current_url, &location)?;
    if let Some(settings) = cors {
        // A redirect to a URL that includes credentials is a network error
        // once the request is `cors`-tainted or would cross an origin
        // (<https://fetch.spec.whatwg.org/#http-redirect-fetch>, steps 9-10).
        let redirects_with_credentials = !next.username().is_empty() || next.password().is_some();
        if redirects_with_credentials
            && (response_type == crate::cors::ResponseType::Cors
                || (settings.mode == crate::cors::RequestMode::Cors
                    && settings.origin != next.origin().ascii_serialization()))
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
                    credentials_include,
                )));
            }
            crate::cors::RedirectMode::Follow => {}
        }
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

/// Appends the request `Origin` header, replacing any page-supplied one.
fn append_origin(headers: &mut HeaderMap, origin: &str) {
    headers.remove("origin");
    let _ = headers.insert("Origin", origin.as_bytes());
}

fn apply_url_credentials(headers: &mut HeaderMap, url: &Url) {
    // https://fetch.spec.whatwg.org/#http-network-or-cache-fetch
    if headers.get("authorization").is_some() || url.username().is_empty() {
        return;
    }
    let value = basic_authorization(url.username(), url.password().unwrap_or(""));
    headers.remove("authorization");
    let _ = headers.insert("Authorization", value.as_bytes());
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
    fn from_incoming(inner: hyper::body::Incoming, budget: CallBudget) -> Self {
        Self { inner, budget }
    }

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
    fn from_parts(
        status: u16,
        headers: HeaderMap,
        body: hyper::body::Incoming,
        final_url: Url,
        budget: CallBudget,
        response_type: crate::cors::ResponseType,
    ) -> Self {
        Self {
            status,
            headers,
            final_url,
            body: Body::from_incoming(body, budget),
            filtering: response_type,
        }
    }

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
