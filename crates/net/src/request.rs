use std::str::FromStr as _;

use crate::agent::Agent;
use crate::context::Context;
use crate::error::{LimitExceeded, NetError, ProtocolError};
use crate::header::{HeaderError, HeaderMap};
use crate::method::Method;
use crate::response::Response;
use crate::websocket::{self, WebSocket};
use url::Url;

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

    pub fn header(mut self, name: &str, value: &str) -> Result<Self, HeaderError> {
        self.headers.insert(name, value.as_bytes())?;
        Ok(self)
    }

    #[must_use]
    pub fn with_context(mut self, context: Context) -> Self {
        self.context = context;
        self
    }

    #[must_use]
    pub fn with_initiator(mut self, initiator: Url) -> Self {
        self.initiator = Some(initiator);
        self
    }

    #[must_use]
    pub fn body(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.body = Some(bytes.into());
        self
    }

    #[must_use]
    pub fn context(&self) -> Context {
        self.context
    }

    // https://fetch.spec.whatwg.org/#http-redirect-fetch
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

        loop {
            let mut wire = url.clone();
            wire.set_fragment(None);
            let mut hop_headers = headers.clone();
            agent.prepare_outbound(&mut hop_headers, &url, context, &method, initiator.as_ref());
            apply_url_credentials(&mut hop_headers, &url);
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
    let value = crate::dial::basic_authorization(url.username(), url.password().unwrap_or(""));
    headers.remove("authorization");
    let _ = headers.insert("Authorization", value.as_bytes());
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
    let mut builder = ureq::http::Request::builder()
        .method(
            ureq::http::Method::from_str(method.as_str())
                .map_err(|_| NetError::Protocol(ProtocolError::RejectedRequest))?,
        )
        .uri(wire_url.as_str());

    for (name, value) in headers.iter() {
        builder = builder.header(name, value);
    }
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
