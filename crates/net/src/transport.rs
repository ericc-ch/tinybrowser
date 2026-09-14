//! Async HTTP transport: hyper client, rustls TLS, tinybrowser deadlines.
//!
//! [ADR 0019](../../../docs/adrs/0019-async-browser-runtime-and-io.md): the
//! browser process owns one Tokio runtime. `net` never implements HTTP framing;
//! `hyper-util` supplies HTTP/1.1, HTTP/2, pooling, and the connector stack,
//! while this module keeps tinybrowser's redirect, cookie, header, timeout, and
//! error policy and applies `--resolve` rules ahead of system DNS.

use std::convert::Infallible;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use base64::Engine as _;
use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::Incoming;
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::connect::dns::{GaiResolver, Name};
use hyper_util::client::legacy::connect::proxy::Tunnel;
use hyper_util::rt::TokioExecutor;
use tower_service::Service;
use url::Url;

use crate::error::{NetError, ProtocolError, TimeoutKind, TransportError};
use crate::protocol::{HeaderMap, Method};
use crate::resolve::{HostMap, Mapped};

/// Absolute deadlines for one `send` / `upgrade` call.
///
/// `global` is fixed at the start of the call and covers every redirect hop.
/// `hop` is `timeout_per_call` measured from the current hop.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CallBudget {
    pub global: Option<Instant>,
    pub hop: Option<Instant>,
}

impl CallBudget {
    pub fn from_engine(
        timeout_global: Option<Duration>,
        timeout_per_call: Option<Duration>,
        start: Instant,
    ) -> Self {
        Self {
            global: timeout_global.map(|limit| start + limit),
            hop: timeout_per_call.map(|limit| start + limit),
        }
    }

    pub fn with_hop_start(self, timeout_per_call: Option<Duration>, hop_start: Instant) -> Self {
        Self {
            global: self.global,
            hop: timeout_per_call.map(|limit| hop_start + limit),
        }
    }

    pub fn deadline(self) -> Option<Instant> {
        match (self.global, self.hop) {
            (Some(global), Some(hop)) => Some(global.min(hop)),
            (global, hop) => global.or(hop),
        }
    }

    pub fn remaining(self) -> Option<Duration> {
        remaining(self.deadline())
    }

    pub fn is_expired(self) -> bool {
        self.remaining() == Some(Duration::ZERO)
    }

    pub fn timeout_kind(self, fallback: TimeoutKind) -> TimeoutKind {
        let now = Instant::now();
        if self.global.is_some_and(|end| now >= end) {
            TimeoutKind::Global
        } else if self.hop.is_some_and(|end| now >= end) {
            TimeoutKind::PerCall
        } else {
            fallback
        }
    }
}

fn remaining(deadline: Option<Instant>) -> Option<Duration> {
    deadline.map(|end| end.saturating_duration_since(Instant::now()))
}

/// Resolver that applies ordered `--resolve` rules before system DNS.
#[derive(Clone)]
pub(crate) struct HostResolver {
    host_map: HostMap,
    system: GaiResolver,
}

impl Service<Name> for HostResolver {
    type Response = std::vec::IntoIter<SocketAddr>;
    type Error = std::io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, name: Name) -> Self::Future {
        match self.host_map.lookup(name.as_str()) {
            Some(Mapped::Addr(ip)) => Box::pin(async move {
                // The connector replaces port zero with the URI's port.
                Ok(vec![SocketAddr::new(ip.into(), 0)].into_iter())
            }),
            Some(Mapped::Fail) => Box::pin(async move {
                Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "host not found",
                ))
            }),
            None => {
                let mut system = self.system.clone();
                Box::pin(async move {
                    let addrs = Service::call(&mut system, name).await?;
                    Ok(addrs.collect::<Vec<_>>().into_iter())
                })
            }
        }
    }
}

type RequestBody = BoxBody<Bytes, Infallible>;

#[derive(Clone)]
pub(crate) struct HttpEngine {
    client: Client<HttpsConnector<HttpConnector<HostResolver>>, RequestBody>,
    pub(crate) proxy: Option<String>,
    pub(crate) timeout_global: Option<Duration>,
    pub(crate) timeout_per_call: Option<Duration>,
    proxied: Option<ProxiedClient>,
    tls_error: Option<Box<str>>,
}

type ProxiedClient = Client<HttpsConnector<Tunnel<HttpConnector<HostResolver>>>, RequestBody>;

impl HttpEngine {
    pub(crate) fn new(
        timeout_global: Option<Duration>,
        timeout_per_call: Option<Duration>,
        proxy: Option<String>,
        host_map: HostMap,
    ) -> Self {
        let resolver = HostResolver {
            host_map,
            system: GaiResolver::new(),
        };
        // `HttpConnector` resolves through `HostResolver` and applies Happy
        // Eyeballs (300 ms default) when a name returns multiple addresses.
        let mut http = HttpConnector::new_with_resolver(resolver);
        http.enforce_http(false);
        let builder = hyper_rustls::HttpsConnectorBuilder::new().with_native_roots();
        let (connector, tls_error) = match builder {
            Ok(builder) => (
                builder
                    .https_or_http()
                    .enable_http1()
                    .enable_http2()
                    .wrap_connector(http.clone()),
                None,
            ),
            Err(error) => (
                hyper_rustls::HttpsConnectorBuilder::new()
                    .with_webpki_roots()
                    .https_or_http()
                    .enable_http1()
                    .enable_http2()
                    .wrap_connector(http.clone()),
                Some(Box::<str>::from(error.to_string())),
            ),
        };
        let client = Client::builder(TokioExecutor::new()).build(connector);
        let proxied = proxy.as_deref().and_then(|proxy| {
            let (destination, auth) = proxy_destination(proxy).ok()?;
            let mut tunnel = Tunnel::new(destination, http);
            if let Some(auth) = auth {
                tunnel = tunnel.with_auth(auth);
            }
            let builder = match hyper_rustls::HttpsConnectorBuilder::new().with_native_roots() {
                Ok(builder) => builder,
                Err(_) => hyper_rustls::HttpsConnectorBuilder::new().with_webpki_roots(),
            };
            let connector = builder
                .https_or_http()
                .enable_http1()
                .enable_http2()
                .wrap_connector(tunnel);
            Some(Client::builder(TokioExecutor::new()).build(connector))
        });
        Self {
            client,
            proxy,
            timeout_global,
            timeout_per_call,
            proxied,
            tls_error,
        }
    }

    pub(crate) fn budget_at(&self, start: Instant) -> CallBudget {
        CallBudget::from_engine(self.timeout_global, self.timeout_per_call, start)
    }

    /// Sends one hop. Redirects, cookies, and header shaping belong to the
    /// caller; this method applies the deadline and maps transport failures.
    ///
    /// # Errors
    ///
    /// [`NetError::Transport`] for DNS, connect, TLS, timeout, or I/O failure.
    /// [`NetError::Protocol`] when the request cannot be represented.
    pub(crate) async fn send(
        &self,
        method: &Method,
        wire_url: &Url,
        headers: &HeaderMap,
        body: Option<&[u8]>,
        budget: CallBudget,
    ) -> Result<(u16, HeaderMap, Incoming), NetError> {
        if let Some(detail) = &self.tls_error {
            return Err(NetError::Transport(TransportError::Tls(detail.clone())));
        }
        if budget.is_expired() {
            return Err(timed_out(budget.timeout_kind(TimeoutKind::Global)));
        }
        let mut request = hyper::Request::builder()
            .method(
                hyper::Method::from_bytes(method.as_str().as_bytes())
                    .map_err(|_| NetError::Protocol(ProtocolError::RejectedRequest))?,
            )
            .uri(
                wire_url
                    .as_str()
                    .parse::<hyper::Uri>()
                    .map_err(|_| NetError::Protocol(ProtocolError::RejectedRequest))?,
            );
        {
            let request_headers = request
                .headers_mut()
                .ok_or(NetError::Protocol(ProtocolError::RejectedRequest))?;
            for (name, value) in headers.iter() {
                let name = hyper::header::HeaderName::from_bytes(name.as_bytes())
                    .map_err(|_| NetError::Protocol(ProtocolError::RejectedRequest))?;
                let value = hyper::header::HeaderValue::from_bytes(value)
                    .map_err(|_| NetError::Protocol(ProtocolError::RejectedRequest))?;
                request_headers.append(name, value);
            }
        }
        let request_body = match body {
            Some(bytes) => Full::new(Bytes::copy_from_slice(bytes)).boxed(),
            None => Empty::<Bytes>::new().boxed(),
        };
        let request = request
            .body(request_body)
            .map_err(|_| NetError::Protocol(ProtocolError::RejectedRequest))?;

        let host = wire_url.host_str().unwrap_or_default().to_owned();
        let request_future = match &self.proxied {
            Some(proxied) => proxied.request(request),
            None => self.client.request(request),
        };
        let response = match wait_for(budget, request_future).await {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => return Err(map_client_error(error, &host)),
            Err(error) => return Err(error),
        };
        let status = response.status().as_u16();
        let mut mapped = HeaderMap::new();
        for (name, value) in response.headers() {
            mapped
                .insert(name.as_str(), value.as_bytes())
                .map_err(|_| NetError::Protocol(ProtocolError::UnrepresentableHeader))?;
        }
        let _ = status;
        Ok((response.status().as_u16(), mapped, response.into_body()))
    }
}

/// Awaits `future` under the call deadline.
async fn wait_for<T>(budget: CallBudget, future: impl Future<Output = T>) -> Result<T, NetError> {
    match budget.deadline() {
        Some(deadline) => {
            let deadline = tokio::time::Instant::from_std(deadline);
            match tokio::time::timeout_at(deadline, future).await {
                Ok(value) => Ok(value),
                Err(_) => Err(timed_out(budget.timeout_kind(TimeoutKind::Global))),
            }
        }
        None => Ok(future.await),
    }
}

fn timed_out(kind: TimeoutKind) -> NetError {
    NetError::Transport(TransportError::Timeout(kind))
}

fn map_client_error(error: hyper_util::client::legacy::Error, host: &str) -> NetError {
    if error.is_connect() {
        let mut source = std::error::Error::source(&error);
        let mut tls_reason: Option<Box<str>> = None;
        while let Some(cause) = source {
            if cause.downcast_ref::<rustls::Error>().is_some() && tls_reason.is_none() {
                tls_reason = Some(cause.to_string().into());
            }
            if let Some(io) = cause.downcast_ref::<std::io::Error>() {
                if io.kind() == std::io::ErrorKind::NotFound {
                    return NetError::Transport(TransportError::Dns(host.into()));
                }
                // A connect-phase io error that wraps another error is a
                // protocol failure (TLS handshake, most often); plain OS
                // errors carry no inner error.
                let wrapped = io.get_ref().is_some();
                if wrapped && tls_reason.is_none() {
                    tls_reason = Some(io.to_string().into());
                }
            }
            let text = cause.to_string();
            let lowered = text.to_ascii_lowercase();
            if tls_reason.is_none()
                && (lowered.contains("tls")
                    || lowered.contains("certificate")
                    || lowered.contains("handshake"))
            {
                tls_reason = Some(text.into());
            }
            source = cause.source();
        }
        if let Some(reason) = tls_reason {
            return NetError::Transport(TransportError::Tls(reason));
        }
        return NetError::Transport(TransportError::Connect(error.to_string().into()));
    }
    NetError::Transport(TransportError::Io(std::io::Error::other(error)))
}

fn proxy_destination(
    proxy: &str,
) -> Result<(hyper::Uri, Option<hyper::header::HeaderValue>), NetError> {
    let url = Url::parse(proxy).map_err(|_| NetError::Protocol(ProtocolError::InvalidProxy))?;
    let host = url
        .host_str()
        .ok_or(NetError::Protocol(ProtocolError::InvalidProxy))?;
    let port = url.port_or_known_default().unwrap_or(80);
    let destination = format!("http://{host}:{port}")
        .parse::<hyper::Uri>()
        .map_err(|_| NetError::Protocol(ProtocolError::InvalidProxy))?;
    let auth = if url.username().is_empty() {
        None
    } else {
        Some(
            basic_authorization(url.username(), url.password().unwrap_or(""))
                .parse::<hyper::header::HeaderValue>()
                .map_err(|_| NetError::Protocol(ProtocolError::InvalidProxy))?,
        )
    };
    Ok((destination, auth))
}

pub(crate) fn basic_authorization(username: &str, password: &str) -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"))
    )
}
