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
    ) {
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
            if let Some(existing) = headers.get("cookie") {
                let merged = format!("{}; {cookie}", String::from_utf8_lossy(existing));
                headers.remove("cookie");
                let _ = headers.insert("Cookie", merged.as_bytes());
            } else {
                let _ = headers.insert("Cookie", cookie.as_bytes());
            }
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
    /// [`NetError::Protocol`] for a non-`http`/`https` URL, a rejected request, or an
    /// unusable `Location`. [`NetError::Limit`] when the redirect cap is exceeded.
    /// [`NetError::Transport`] when a hop fails.
    pub async fn send(&self, request: Request) -> Result<Response, NetError> {
        if !matches!(request.url.scheme(), "http" | "https") {
            return Err(NetError::Protocol(ProtocolError::RejectedRequest));
        }
        let mut method = request.method;
        let mut url = request.url;
        let mut headers = request.headers;
        let mut body = request.body;
        let mut followed = 0u32;
        let initiator_kind = request.initiator_kind;
        let initiator = request.initiator;
        let mut cross_site_redirect = false;
        let started = Instant::now();
        let mut budget = self.engine.budget_at(started);
        if let Some(deadline) = request.deadline {
            budget.global = Some(deadline);
        }

        loop {
            if budget.is_expired() {
                return Err(NetError::Transport(TransportError::Timeout(
                    budget.timeout_kind(TimeoutKind::Global),
                )));
            }
            budget = budget.with_hop_start(self.engine.timeout_per_call, Instant::now());
            let mut wire = url.clone();
            wire.set_fragment(None);
            let mut hop_headers = headers.clone();
            self.prepare_outbound(
                &mut hop_headers,
                &url,
                initiator_kind,
                &method,
                initiator.as_ref(),
                cross_site_redirect,
            );
            apply_url_credentials(&mut hop_headers, &url);
            let (status, response_headers, reader) = self
                .engine
                .send(&method, &wire, &hop_headers, body.as_deref(), budget)
                .await?;
            let response =
                Response::from_parts(status, response_headers, reader, url.clone(), budget);
            self.store_set_cookie_lines(
                &url,
                initiator_kind,
                &method,
                initiator.as_ref(),
                cross_site_redirect,
                response
                    .headers()
                    .get_all("set-cookie")
                    .filter_map(|v| std::str::from_utf8(v).ok()),
            );

            let Some(location) = followable_location(response.status(), response.headers())? else {
                return Ok(response);
            };

            if self.max_redirects == 0 {
                return Ok(response);
            }
            if followed == self.max_redirects {
                return Err(NetError::Limit(LimitExceeded::Redirect));
            }

            let next = resolve_location(&url, location)?;
            cross_site_redirect |= !cookies::schemeful_same_site(&url, &next);
            apply_redirect_policy(
                response.status(),
                &url,
                &next,
                &mut method,
                &mut headers,
                &mut body,
            );
            url = next;
            followed += 1;
            drop(response);
        }
    }

    /// WebSocket handshake for `request`. The URL must be `ws` or `wss`.
    ///
    /// # Errors
    ///
    /// [`NetError::Protocol`] for a non-WebSocket URL or a failed handshake.
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
        );
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

/// One outbound request as plain data. Set fields directly, then hand it to
/// [`Agent::send`] (`http`/`https`) or [`Agent::upgrade`] (`ws`/`wss`).
///
/// Default [`InitiatorKind`] is [`InitiatorKind::Navigation`].
#[derive(Debug)]
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
fn apply_redirect_policy(
    status: u16,
    current: &Url,
    next: &Url,
    method: &mut Method,
    headers: &mut HeaderMap,
    body: &mut Option<Vec<u8>>,
) {
    let post_to_get = matches!(status, 301 | 302) && *method == Method::POST;
    let see_other = status == 303 && *method != Method::GET && *method != Method::HEAD;
    if post_to_get || see_other {
        *method = Method::GET;
        *body = None;
        headers.remove("content-encoding");
        headers.remove("content-language");
        headers.remove("content-location");
        headers.remove("content-type");
        headers.remove("content-length");
        headers.remove("transfer-encoding");
    }
    if current.origin() != next.origin() {
        headers.remove("authorization");
        headers.remove("cookie");
        headers.remove("host");
    }
    if next.scheme() != "https" && !loopback_http(next) {
        headers.remove("sec-ch-ua");
        headers.remove("sec-ch-ua-mobile");
        headers.remove("sec-ch-ua-platform");
        headers.remove("sec-ch-prefers-color-scheme");
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
}

impl Response {
    fn from_parts(
        status: u16,
        headers: HeaderMap,
        body: hyper::body::Incoming,
        final_url: Url,
        budget: CallBudget,
    ) -> Self {
        Self {
            status,
            headers,
            final_url,
            body: Body::from_incoming(body, budget),
        }
    }

    /// HTTP status code.
    #[must_use]
    pub fn status(&self) -> u16 {
        self.status
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
