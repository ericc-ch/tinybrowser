use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use url::Url;

use crate::context::Context;
use crate::cookie::{CookieJar, CookieOp, RetrievalKind};
use crate::error::{LimitExceeded, NetError, ProtocolError, TransportError};
use crate::protocol::{HeaderError, HeaderMap, Method};
use crate::transport::{HttpEngine, basic_authorization};
use crate::websocket::{self, WebSocket};

const CHUNK_SIZE: usize = 16 * 1024;
// https://fetch.spec.whatwg.org/#http-redirect-fetch
const DEFAULT_MAX_REDIRECTS: u32 = 20;

/// Builds an [`Agent`].
///
/// Debug output records whether a proxy is configured, not its URI or credentials.
#[derive(Clone)]
pub struct AgentBuilder {
    user_agent: Option<String>,
    timeout_global: Option<Duration>,
    timeout_per_call: Option<Duration>,
    max_redirects: u32,
    proxy: Option<String>,
}

impl std::fmt::Debug for AgentBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentBuilder")
            .field("has_proxy", &self.proxy.is_some())
            .field("timeout_global", &self.timeout_global)
            .field("timeout_per_call", &self.timeout_per_call)
            .field("max_redirects", &self.max_redirects)
            .finish_non_exhaustive()
    }
}

impl Default for AgentBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentBuilder {
    /// Agent with default redirect cap 20 and no proxy, timeout, or User-Agent.
    #[must_use]
    pub fn new() -> Self {
        Self {
            user_agent: None,
            timeout_global: None,
            timeout_per_call: None,
            max_redirects: DEFAULT_MAX_REDIRECTS,
            proxy: None,
        }
    }

    /// Default `User-Agent` for requests that do not set that header themselves.
    #[must_use]
    pub fn user_agent(mut self, value: &str) -> Self {
        self.user_agent = Some(value.to_owned());
        self
    }

    /// Cap on the whole call, including redirects.
    #[must_use]
    pub fn timeout_global(mut self, timeout: Duration) -> Self {
        self.timeout_global = Some(timeout);
        self
    }

    /// Cap on a single hop.
    #[must_use]
    pub fn timeout_per_call(mut self, timeout: Duration) -> Self {
        self.timeout_per_call = Some(timeout);
        self
    }

    /// Maximum redirect hops. `0` returns the redirect response without following.
    #[must_use]
    pub fn max_redirects(mut self, max_redirects: u32) -> Self {
        self.max_redirects = max_redirects;
        self
    }

    /// HTTP CONNECT proxy. `authority` must be an `http://` URI with a host.
    ///
    /// # Errors
    ///
    /// [`ProtocolError::InvalidProxy`] when `authority` is not an `http://` URI with a host.
    pub fn proxy(mut self, authority: &str) -> Result<Self, NetError> {
        let parsed = Url::parse(authority).map_err(|_| invalid_proxy())?;
        if parsed.scheme() != "http" || parsed.host_str().is_none() {
            return Err(invalid_proxy());
        }
        self.proxy = Some(authority.to_owned());
        Ok(self)
    }

    /// Builds an agent with a private cookie jar and the selected transport options.
    #[must_use]
    pub fn build(self) -> Agent {
        Agent {
            engine: HttpEngine::new(self.timeout_global, self.timeout_per_call, self.proxy),
            ua: self.user_agent,
            max_redirects: self.max_redirects,
            jar: Arc::new(Mutex::new(CookieJar::default())),
            now: SystemTime::now,
        }
    }
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
    /// Agent with default builder settings.
    #[must_use]
    pub fn new() -> Self {
        AgentBuilder::new().build()
    }

    /// Starts a request. `url` must be absolute; [`RequestBuilder::send`] requires
    /// `http`/`https`, and [`RequestBuilder::upgrade`] requires `ws`/`wss`.
    #[must_use]
    pub fn request(&self, method: Method, url: Url) -> RequestBuilder {
        RequestBuilder::new(self.clone(), method, url)
    }

    /// `document.cookie` getter for `uri`: non-HTTP cookies, semicolon-separated.
    #[must_use]
    pub fn cookies_for(&self, uri: &Url) -> String {
        self.jar
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .cookie_string(CookieOp {
                url: uri,
                now: (self.now)(),
                kind: RetrievalKind::NonHttp,
                context: Context::Fetch,
                method: &Method::GET,
                initiator: Some(uri),
                cross_site_redirect: false,
            })
    }

    /// `document.cookie` setter for `uri`. Invalid `Set-Cookie` lines are ignored.
    pub fn set_cookie(&self, value: &str, uri: &Url) {
        self.jar
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .store(
                value,
                CookieOp {
                    url: uri,
                    now: (self.now)(),
                    kind: RetrievalKind::NonHttp,
                    context: Context::Fetch,
                    method: &Method::GET,
                    initiator: Some(uri),
                    cross_site_redirect: false,
                },
            );
    }

    pub(crate) fn prepare_outbound(
        &self,
        headers: &mut HeaderMap,
        url: &Url,
        context: Context,
        method: &Method,
        initiator: Option<&Url>,
        cross_site_redirect: bool,
    ) {
        let cookie = self
            .jar
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .cookie_string(CookieOp {
                url,
                now: (self.now)(),
                kind: RetrievalKind::Http,
                context,
                method,
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
        if context == Context::WsHandshake
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
        context: Context,
        method: &Method,
        initiator: Option<&Url>,
        cross_site_redirect: bool,
        lines: impl IntoIterator<Item = impl AsRef<str>>,
    ) {
        let now = (self.now)();
        let mut jar = self
            .jar
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for line in lines {
            jar.store(
                line.as_ref(),
                CookieOp {
                    url,
                    now,
                    kind: RetrievalKind::Http,
                    context,
                    method,
                    initiator,
                    cross_site_redirect,
                },
            );
        }
    }
}

impl Default for Agent {
    fn default() -> Self {
        Self::new()
    }
}

/// One outbound request. Default [`Context`] is [`Context::Navigation`].
#[derive(Debug)]
pub struct RequestBuilder {
    agent: Agent,
    method: Method,
    url: Url,
    headers: HeaderMap,
    context: Context,
    initiator: Option<Url>,
    body: Option<Vec<u8>>,
}

impl RequestBuilder {
    fn new(agent: Agent, method: Method, url: Url) -> Self {
        Self {
            agent,
            method,
            url,
            headers: HeaderMap::new(),
            context: Context::default(),
            initiator: None,
            body: None,
        }
    }

    /// Appends a request header. Does not replace earlier values of the same name.
    ///
    /// # Errors
    ///
    /// [`HeaderError`] when `name` or `value` is not a valid HTTP header field.
    pub fn header(mut self, name: &str, value: &str) -> Result<Self, HeaderError> {
        self.headers.insert(name, value.as_bytes())?;
        Ok(self)
    }

    /// Sets the initiator class used for `SameSite` and (later) `Sec-Fetch-*`.
    #[must_use]
    pub fn with_context(mut self, context: Context) -> Self {
        self.context = context;
        self
    }

    /// Document URL used as the `SameSite` initiator and WebSocket `Origin`.
    #[must_use]
    pub fn with_initiator(mut self, initiator: Url) -> Self {
        self.initiator = Some(initiator);
        self
    }

    /// Request body bytes. Redirects that convert to GET drop this.
    #[must_use]
    pub fn body(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.body = Some(bytes.into());
        self
    }

    /// Initiator class for this request.
    #[must_use]
    pub fn context(&self) -> Context {
        self.context
    }

    /// Sends the request and follows HTTP redirects per
    /// [HTTP redirect fetch](https://fetch.spec.whatwg.org/#http-redirect-fetch).
    ///
    /// Status codes are response data, not errors. The URL must be `http` or `https`.
    ///
    /// # Errors
    ///
    /// [`NetError::Protocol`] for a non-`http`/`https` URL, a rejected request, or an
    /// unusable `Location`. [`NetError::Limit`] when the redirect cap is exceeded.
    /// [`NetError::Transport`] when a hop fails.
    pub fn send(self) -> Result<Response, NetError> {
        if !matches!(self.url.scheme(), "http" | "https") {
            return Err(NetError::Protocol(ProtocolError::RejectedRequest));
        }
        let mut method = self.method;
        let mut url = self.url;
        let mut headers = self.headers;
        let mut body = self.body;
        let mut followed = 0u32;
        let agent = self.agent;
        let context = self.context;
        let initiator = self.initiator;
        let mut cross_site_redirect = false;

        loop {
            let mut wire = url.clone();
            wire.set_fragment(None);
            let mut hop_headers = headers.clone();
            agent.prepare_outbound(
                &mut hop_headers,
                &url,
                context,
                &method,
                initiator.as_ref(),
                cross_site_redirect,
            );
            apply_url_credentials(&mut hop_headers, &url);
            let (status, response_headers, reader) =
                agent
                    .engine
                    .send(&method, &wire, &hop_headers, body.as_deref())?;
            let response =
                Response::from_parts(status, response_headers, reader, context, url.clone());
            agent.store_set_cookie_lines(
                &url,
                context,
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

            if agent.max_redirects == 0 {
                return Ok(response);
            }
            if followed == agent.max_redirects {
                return Err(NetError::Limit(LimitExceeded::Redirect));
            }

            let next = resolve_location(&url, location)?;
            cross_site_redirect |= !crate::cookie::schemeful_same_site(&url, &next);
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

    /// WebSocket handshake. The URL must be `ws` or `wss`.
    ///
    /// # Errors
    ///
    /// [`NetError::Protocol`] for a non-WebSocket URL or a failed handshake.
    /// [`NetError::Transport`] when the dial or TLS handshake fails.
    pub fn upgrade(self) -> Result<WebSocket, NetError> {
        if !matches!(self.url.scheme(), "ws" | "wss") {
            return Err(NetError::Protocol(ProtocolError::RejectedRequest));
        }
        let method = Method::GET;
        let context = Context::WsHandshake;
        let mut headers = self.headers;
        self.agent.prepare_outbound(
            &mut headers,
            &self.url,
            context,
            &method,
            self.initiator.as_ref(),
            false,
        );
        websocket::connect(
            &self.agent,
            &self.url,
            &headers,
            context,
            &method,
            self.initiator.as_ref(),
        )
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
    inner: Box<dyn io::Read + Send>,
}

impl std::fmt::Debug for Body {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Body").finish_non_exhaustive()
    }
}

impl Body {
    fn from_reader(inner: Box<dyn io::Read + Send>) -> Self {
        Self { inner }
    }

    /// Next chunk, or `None` at end of body.
    ///
    /// # Errors
    ///
    /// [`NetError::Transport`] when the socket read fails.
    pub fn read_chunk(&mut self) -> Result<Option<Vec<u8>>, NetError> {
        let mut buf = vec![0u8; CHUNK_SIZE];
        loop {
            match self.inner.read(&mut buf) {
                Ok(0) => return Ok(None),
                Ok(n) => {
                    buf.truncate(n);
                    return Ok(Some(buf));
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) => return Err(NetError::Transport(TransportError::Io(err))),
            }
        }
    }

    /// Entire body, failing if it would exceed `limit` bytes.
    ///
    /// # Errors
    ///
    /// [`NetError::Limit`] when the body is larger than `limit`.
    /// [`NetError::Transport`] when a read fails.
    pub fn bytes(mut self, limit: usize) -> Result<Vec<u8>, NetError> {
        let mut out = Vec::new();
        while let Some(chunk) = self.read_chunk()? {
            if out.len() + chunk.len() > limit {
                return Err(NetError::Limit(LimitExceeded::Size(limit as u64)));
            }
            out.extend_from_slice(&chunk);
        }
        Ok(out)
    }

    /// Entire body as lossy UTF-8, with the same `limit` as [`Body::bytes`].
    ///
    /// # Errors
    ///
    /// Same as [`Body::bytes`].
    pub fn text(self, limit: usize) -> Result<String, NetError> {
        self.bytes(limit)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// HTTP response after redirects. Status codes are data, including 4xx and 5xx.
#[derive(Debug)]
pub struct Response {
    status: u16,
    headers: HeaderMap,
    final_url: Url,
    context: Context,
    body: Body,
}

impl Response {
    fn from_parts(
        status: u16,
        headers: HeaderMap,
        body: Box<dyn io::Read + Send>,
        context: Context,
        final_url: Url,
    ) -> Self {
        Self {
            status,
            headers,
            final_url,
            context,
            body: Body::from_reader(body),
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

    /// Initiator class that produced this response.
    #[must_use]
    pub fn context(&self) -> Context {
        self.context
    }

    /// Consumes the response and returns its body stream.
    #[must_use]
    pub fn into_body(self) -> Body {
        self.body
    }
}
