//! [`RequestBuilder`]: everything a dial carries, up to `send()` or `upgrade()`.

use std::str::FromStr as _;

use crate::agent::Agent;
use crate::context::Context;
use crate::error::{LimitExceeded, NetError, ProtocolError};
use crate::header::{HeaderError, HeaderMap};
use crate::method::Method;
use crate::response::Response;
use crate::websocket::{self, WebSocket};
use url::Url;

/// A request under construction: method, absolute URL, headers, initiator
/// [`Context`], optional body.
///
/// Built from [`Agent::request`]; finished with [`RequestBuilder::send`]
/// (HTTP) or [`RequestBuilder::upgrade`] (WebSocket).
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
    pub(super) fn new(agent: Agent, method: Method, url: Url) -> Self {
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

    /// Add one request header, keeping case and insertion order.
    ///
    /// Duplicate names are legal and stay in order (RFC 9110 §5.2).
    /// `User-Agent` set here suppresses the agent-level default instead of
    /// stacking with it; additional request-level `User-Agent` entries
    /// still append.
    ///
    /// # Errors
    ///
    /// [`HeaderError`] for invalid names or values — same grammar rules as
    /// [`HeaderMap::insert`](crate::HeaderMap::insert).
    pub fn header(mut self, name: &str, value: &str) -> Result<Self, HeaderError> {
        self.headers.insert(name, value.as_bytes())?;
        Ok(self)
    }

    /// Set the initiator context. Defaults to [`Context::Navigation`].
    ///
    /// [`RequestBuilder::upgrade`] forces [`Context::WsHandshake`] regardless
    /// of this value (the finish path is the handshake).
    #[must_use]
    pub fn with_context(mut self, context: Context) -> Self {
        self.context = context;
        self
    }

    /// Document URL used for schemeful same-site cookie checks
    /// ([RFC 6265bis §5.2](https://httpwg.org/http-extensions/draft-ietf-httpbis-rfc6265bis.html#name-same-site-and-cross-site-re))
    /// and, on [`RequestBuilder::upgrade`], the handshake `Origin` header.
    ///
    /// `None` (the default) means a first-party / embedder load: the request
    /// is same-site with itself and no `Origin` is sent on upgrade.
    /// `browser` sets this to the document URL for page-initiated dials.
    #[must_use]
    pub fn with_initiator(mut self, initiator: Url) -> Self {
        self.initiator = Some(initiator);
        self
    }

    /// Attach a request body (any `POST`/`PUT`-style payload).
    ///
    /// Ignored by [`RequestBuilder::upgrade`] (the handshake is always GET
    /// with no body).
    #[must_use]
    pub fn body(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.body = Some(bytes.into());
        self
    }

    /// The initiator context this request will carry on [`RequestBuilder::send`].
    #[must_use]
    pub fn context(&self) -> Context {
        self.context
    }

    /// Dial, following redirects up to the agent's cap.
    ///
    /// Intermediate 301/302/303/307/308 hops with a `Location` are
    /// followed ([fetch #http-redirect-fetch](https://fetch.spec.whatwg.org/#http-redirect-fetch));
    /// a 3xx with no Location, or when the cap is 0, is itself the final
    /// response. The HTTP status of that final response, including 3xx,
    /// comes back as `Ok` (decision 02, statuses-as-data). Transport
    /// failures and protocol violations arrive as `Err`.
    ///
    /// The fragment is stripped from each request line
    /// ([fetch #http-network-or-cache-fetch](https://fetch.spec.whatwg.org/#http-network-or-cache-fetch))
    /// and kept on [`Response::final_url`], inherited across hops when the
    /// `Location` has none.
    ///
    /// # Errors
    ///
    /// [`NetError::Transport`] when the dial or socket fails;
    /// [`NetError::Protocol`] when the peer speaks malformed HTTP, the
    /// backend rejects the built request, or a `Location` is unusable;
    /// [`NetError::Limit`] when a size or redirect cap fires.
    pub fn send(self) -> Result<Response, NetError> {
        let mut method = self.method;
        let mut url = self.url;
        let mut headers = self.headers;
        let mut body = self.body;
        let mut followed = 0u32;
        let agent = self.agent;
        let context = self.context;
        let initiator = self.initiator;

        loop {
            let mut wire = url.clone();
            wire.set_fragment(None);
            let mut hop_headers = headers.clone();
            agent.prepare_outbound(&mut hop_headers, &url, context, &method, initiator.as_ref());
            let response = dispatch(
                &agent,
                &method,
                &wire,
                &hop_headers,
                body.as_deref(),
                context,
                url.clone(),
            )?;
            agent.store_set_cookie_lines(
                &url,
                context,
                &method,
                initiator.as_ref(),
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

    /// Open a WebSocket through the shared dial path.
    ///
    /// Forces GET and [`Context::WsHandshake`]. Cookie, agent `User-Agent`,
    /// and `Origin` (when [`RequestBuilder::with_initiator`] was set) run
    /// through the same [`Agent::prepare_outbound`] path as
    /// [`RequestBuilder::send`]. Upgrade / `Sec-WebSocket-*` headers are
    /// written at the tungstenite conversion point.
    ///
    /// # Errors
    ///
    /// [`NetError::Protocol`] for a non-`ws`/`wss` URL; transport/TLS
    /// failures as usual.
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

/// 301/302/303/307/308 with a non-empty Location, per fetch's redirect
/// status set. Other 3xx (300, 304, …) stay as the final response.
fn followable_location(status: u16, headers: &HeaderMap) -> Result<Option<&str>, NetError> {
    if !matches!(status, 301 | 302 | 303 | 307 | 308) {
        return Ok(None);
    }
    let Some(raw) = headers.get("location") else {
        return Ok(None);
    };
    if raw.is_empty() {
        return Ok(None);
    }
    let location =
        std::str::from_utf8(raw).map_err(|_| NetError::Protocol(ProtocolError::RejectedRequest))?;
    Ok(Some(location))
}

/// Resolve `Location` against the current URL and inherit the fragment
/// when the redirect doesn't supply one (fetch #http-redirect-fetch).
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

/// Method/body/header mutations for one hop (fetch #http-redirect-fetch).
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
        // fetch "request-body-header name"
        headers.remove("content-encoding");
        headers.remove("content-language");
        headers.remove("content-location");
        headers.remove("content-type");
    }
    if current.origin() != next.origin() {
        headers.remove("authorization");
        headers.remove("cookie");
    }
}

fn dispatch(
    agent: &Agent,
    method: &Method,
    wire_url: &Url,
    headers: &HeaderMap,
    body: Option<&[u8]>,
    context: Context,
    logical_url: Url,
) -> Result<Response, NetError> {
    // ureq types appear only here and in `Response::from_backend` /
    // `From<ureq::Error>` (decision 01).
    let mut builder = ureq::http::Request::builder()
        .method(
            ureq::http::Method::from_str(method.as_str())
                .map_err(|_| NetError::Protocol(ProtocolError::RejectedRequest))?,
        )
        .uri(wire_url.as_str());

    for (name, value) in headers.iter() {
        builder = builder.header(name, value);
    }
    // ureq writes origin-form only. Forward-proxy credentials ride this
    // header; CONNECT puts them on the CONNECT request in dial instead.
    if wire_url.scheme() == "http"
        && headers.get("proxy-authorization").is_none()
        && let Some(value) = crate::dial::proxy_basic_token(agent.proxy.as_deref())
    {
        builder = builder.header("Proxy-Authorization", value);
    }

    let rejected = |_| NetError::Protocol(ProtocolError::RejectedRequest);
    let response = match body {
        Some(bytes) => agent
            .inner
            .run(builder.body(bytes.to_vec()).map_err(rejected)?)
            .map_err(NetError::from)?,
        None => agent
            .inner
            .run(builder.body(()).map_err(rejected)?)
            .map_err(NetError::from)?,
    };
    Response::from_backend(response, context, logical_url)
}
