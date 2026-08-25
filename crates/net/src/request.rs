//! [`RequestBuilder`]: everything a dial carries, up to `send()`.

use std::str::FromStr as _;

use crate::agent::Agent;
use crate::context::Context;
use crate::error::{LimitExceeded, NetError};
use crate::header::{HeaderError, HeaderMap};
use crate::method::Method;
use crate::response::Response;
use url::Url;

/// Fetch #http-redirect-fetch: only these statuses auto-follow.
const REDIRECT_STATUSES: &[u16] = &[301, 302, 303, 307, 308];

/// Request-body-header names deleted when a redirect turns the method into
/// GET (fetch #http-redirect-fetch).
const REQUEST_BODY_HEADERS: &[&str] = &[
    "content-encoding",
    "content-language",
    "content-location",
    "content-type",
    "content-length",
];

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

    /// Dial.
    ///
    /// The **final** HTTP status after redirect policy comes back as
    /// `Ok(Response)` (decision 02, statuses-as-data). Intermediate 301 /
    /// 302 / 303 / 307 / 308 hops with a `Location` are followed here,
    /// matching WHATWG fetch #http-redirect-fetch — not the v1 backend's
    /// own redirect table. Transport failures, protocol violations, and
    /// the redirect cap arrive as `Err`.
    ///
    /// # Errors
    ///
    /// [`NetError::Transport`] when the dial or socket fails;
    /// [`NetError::Protocol`] when the peer speaks malformed HTTP, the
    /// backend rejects the built request, or `Location` is unusable;
    /// [`NetError::Limit`] when the redirect cap fires.
    pub fn send(self) -> Result<Response, NetError> {
        let mut method = self.method;
        let mut url = self.url;
        url.set_fragment(None);
        let mut headers = self.headers;
        let mut body = self.body;
        let mut followed: u32 = 0;

        loop {
            let response = dispatch(
                &self.agent,
                &method,
                &url,
                &headers,
                body.as_deref(),
                self.context,
            )?;

            if !REDIRECT_STATUSES.contains(&response.status()) {
                return Ok(response);
            }

            let Some(next) = location_url(&url, response.headers())? else {
                // No Location: the 3xx itself is the final response
                // (fetch: locationURL is null → return response).
                return Ok(response);
            };

            // Cap 0: do not follow; 3xx is data. Cap N: N follows, then
            // Limit — Chrome's 20 and ticket 08's "exceeded → Limit".
            if self.agent.redirect_cap == 0 || followed >= self.agent.redirect_cap {
                if self.agent.redirect_cap == 0 {
                    return Ok(response);
                }
                return Err(NetError::Limit(LimitExceeded::Redirect));
            }
            followed += 1;

            apply_fetch_redirect(&mut method, &mut body, &mut headers, response.status());
            if url.origin() != next.origin() {
                // fetch #http-redirect-fetch: drop Authorization the
                // moment another origin is seen.
                headers.remove("authorization");
            }

            url = next;
            url.set_fragment(None);
            drop(response);
        }
    }
}

fn dispatch(
    agent: &Agent,
    method: &Method,
    url: &Url,
    headers: &HeaderMap,
    body: Option<&[u8]>,
    context: Context,
) -> Result<Response, NetError> {
    // ureq types appear only here (decision 01's single conversion point).
    let mut builder = ureq::http::Request::builder()
        .method(
            ureq::http::Method::from_str(method.as_str())
                .map_err(|err| NetError::Protocol(err.to_string().into()))?,
        )
        .uri(url.as_str());

    let request_sets_ua = headers.get_all("user-agent").next().is_some();
    if !request_sets_ua && let Some(ua) = &agent.ua {
        builder = builder.header("User-Agent", ua);
    }
    for (name, value) in headers.iter() {
        builder = builder.header(name, value);
    }

    let protocol_error = |err: ureq::http::Error| NetError::Protocol(err.to_string().into());
    let response = match body {
        Some(bytes) => agent
            .inner
            .run(builder.body(bytes.to_vec()).map_err(protocol_error)?)
            .map_err(NetError::from)?,
        None => agent
            .inner
            .run(builder.body(()).map_err(protocol_error)?)
            .map_err(NetError::from)?,
    };
    Response::from_backend(response, context)
}

fn location_url(base: &Url, headers: &HeaderMap) -> Result<Option<Url>, NetError> {
    let Some(raw) = headers.get("location") else {
        return Ok(None);
    };
    let location = std::str::from_utf8(raw)
        .map_err(|_| NetError::Protocol("redirect Location is not UTF-8".into()))?;
    base.join(location).map(Some).map_err(|err| {
        NetError::Protocol(format!("redirect Location {location:?} is not a URL: {err}").into())
    })
}

/// WHATWG fetch #http-redirect-fetch method/body rewrite.
///
/// 301/302 + POST, or 303 + anything but GET/HEAD → GET and drop the body.
/// 307/308 keep method and body.
fn apply_fetch_redirect(
    method: &mut Method,
    body: &mut Option<Vec<u8>>,
    headers: &mut HeaderMap,
    status: u16,
) {
    let rewrite_to_get = (matches!(status, 301 | 302) && *method == Method::POST)
        || (status == 303 && *method != Method::GET && *method != Method::HEAD);
    if !rewrite_to_get {
        return;
    }
    *method = Method::GET;
    *body = None;
    for name in REQUEST_BODY_HEADERS {
        headers.remove(name);
    }
}
