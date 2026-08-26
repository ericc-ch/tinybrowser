//! [`RequestBuilder`]: everything a dial carries, up to `send()`.

use std::str::FromStr as _;

use crate::agent::Agent;
use crate::context::Context;
use crate::error::{NetError, ProtocolError};
use crate::header::{HeaderError, HeaderMap};
use crate::method::Method;
use crate::response::Response;
use url::Url;

/// A request under construction: method, absolute URL, headers, initiator
/// [`Context`], optional body.
///
/// Built from [`Agent::request`]; finished with [`RequestBuilder::send`].
#[derive(Debug)]
pub struct RequestBuilder {
    agent: Agent,
    method: Method,
    url: Url,
    headers: HeaderMap,
    context: Context,
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
    #[must_use]
    pub fn with_context(mut self, context: Context) -> Self {
        self.context = context;
        self
    }

    /// Attach a request body (any `POST`/`PUT`-style payload).
    #[must_use]
    pub fn body(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.body = Some(bytes.into());
        self
    }

    /// The initiator context this request will carry.
    #[must_use]
    pub fn context(&self) -> Context {
        self.context
    }

    /// Dial once.
    ///
    /// The HTTP status, including 3xx, comes back as `Ok(Response)`
    /// (decision 02, statuses-as-data). This crate does not follow
    /// `Location`; that loop is `browser`'s (WHATWG fetch
    /// #http-redirect-fetch). Transport failures and protocol violations
    /// arrive as `Err`.
    ///
    /// The fragment is stripped from the request line
    /// ([fetch #http-network-or-cache-fetch](https://fetch.spec.whatwg.org/#http-network-or-cache-fetch))
    /// and kept on [`Response::final_url`].
    ///
    /// # Errors
    ///
    /// [`NetError::Transport`] when the dial or socket fails;
    /// [`NetError::Protocol`] when the peer speaks malformed HTTP or the
    /// backend rejects the built request;
    /// [`NetError::Limit`] when a size cap fires.
    pub fn send(self) -> Result<Response, NetError> {
        let mut wire = self.url.clone();
        wire.set_fragment(None);
        dispatch(
            &self.agent,
            &self.method,
            &wire,
            &self.headers,
            self.body.as_deref(),
            self.context,
            self.url,
        )
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

    let request_sets_ua = headers.get_all("user-agent").next().is_some();
    if !request_sets_ua && let Some(ua) = &agent.ua {
        builder = builder.header("User-Agent", ua);
    }
    for (name, value) in headers.iter() {
        builder = builder.header(name, value);
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
