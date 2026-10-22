//! Async HTTP transport: hyper client, native-tls (OpenSSL) TLS, tinybrowser deadlines.
//!
//! The browser process owns one Tokio runtime. `net` never implements HTTP
//! framing; `hyper-util` supplies HTTP/1.1, HTTP/2, pooling, and the connector
//! stack, while this module keeps tinybrowser's redirect, cookie, header,
//! timeout, and error policy and applies `--resolve` rules ahead of system
//! DNS.

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
use hyper_tls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::connect::dns::{GaiResolver, Name};
use hyper_util::client::legacy::connect::proxy::Tunnel;
use hyper_util::rt::TokioExecutor;
use tower_service::Service;
use url::Url;

use crate::error::{NetError, ProtocolError, TimeoutKind, TransportError};
use crate::protocol::{HeaderMap, Method, field_value};
use crate::resolve::{HostMap, ResolveFailure, Target};

/// Absolute deadlines for one `send` call.
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
    type Error = ResolveFailure;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, name: Name) -> Self::Future {
        match self.host_map.lookup(name.as_str()) {
            Some(Target::Addr(ip)) => Box::pin(async move {
                // The connector replaces port zero with the URI's port.
                Ok(vec![SocketAddr::new(ip.into(), 0)].into_iter())
            }),
            Some(Target::Fail) => Box::pin(async move { Err(ResolveFailure::Denied) }),
            None => {
                let mut system = self.system.clone();
                Box::pin(async move {
                    let addrs = Service::call(&mut system, name)
                        .await
                        .map_err(ResolveFailure::System)?;
                    Ok(addrs.collect::<Vec<_>>().into_iter())
                })
            }
        }
    }
}

type RequestBody = BoxBody<Bytes, Infallible>;

/// An HTTP CONNECT proxy parsed once from [`crate::AgentOptions::proxy`].
///
/// The basic-auth credential lives in a sensitive field value, never in the
/// raw configuration string.
pub(crate) struct Proxy {
    destination: hyper::Uri,
    auth: Option<hyper::header::HeaderValue>,
}

impl Proxy {
    /// Parses `proxy` into a CONNECT destination and optional credentials.
    ///
    /// # Errors
    ///
    /// [`ProtocolError::InvalidProxy`] unless `proxy` is an `http://` URI with
    /// a host.
    pub(crate) fn parse(proxy: &str) -> Result<Self, NetError> {
        let url = Url::parse(proxy).map_err(|_| NetError::Protocol(ProtocolError::InvalidProxy))?;
        if url.scheme() != "http" {
            return Err(NetError::Protocol(ProtocolError::InvalidProxy));
        }
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
            let mut value = field_value(basic_authorization(
                url.username(),
                url.password().unwrap_or(""),
            ));
            value.set_sensitive(true);
            Some(value)
        };
        Ok(Self { destination, auth })
    }

    fn tunnel(&self, http: HttpConnector<HostResolver>) -> Tunnel<HttpConnector<HostResolver>> {
        let mut tunnel = Tunnel::new(self.destination.clone(), http);
        if let Some(auth) = &self.auth {
            tunnel = tunnel.with_auth(auth.clone());
        }
        tunnel
    }
}

#[derive(Clone)]
pub(crate) struct HttpEngine {
    client: Client<HttpsConnector<HttpConnector<HostResolver>>, RequestBody>,
    pub(crate) timeout_global: Option<Duration>,
    pub(crate) timeout_per_call: Option<Duration>,
    proxied: Option<ProxiedClient>,
}

type ProxiedClient = Client<HttpsConnector<Tunnel<HttpConnector<HostResolver>>>, RequestBody>;

/// `native-tls` connector trusting `cas`, offering `alpns` in order.
///
/// ALPN is configured here, not by `hyper-tls`: its `alpn` feature only reports
/// the protocol the server picked. OpenSSL initialization failure is
/// unrecoverable for TLS in this process, and `hyper-tls`'s own constructors
/// panic on it for the same reason.
fn tls_connector(
    cas: &[native_tls::Certificate],
    alpns: &[&str],
) -> tokio_native_tls::TlsConnector {
    let mut tls = native_tls::TlsConnector::builder();
    for ca in cas {
        tls.add_root_certificate(ca.clone());
    }
    let tls = tls
        .request_alpns(alpns)
        .build()
        .expect("openssl tls connector");
    tokio_native_tls::TlsConnector::from(tls)
}

impl HttpEngine {
    pub(crate) fn new(
        timeout_global: Option<Duration>,
        timeout_per_call: Option<Duration>,
        proxy: Option<Proxy>,
        host_map: HostMap,
        tls_cas: &[native_tls::Certificate],
    ) -> Self {
        let resolver = HostResolver {
            host_map,
            system: GaiResolver::new(),
        };
        // `HttpConnector` resolves through `HostResolver` and applies Happy
        // Eyeballs (300 ms default) when a name returns multiple addresses.
        let mut http = HttpConnector::new_with_resolver(resolver);
        http.enforce_http(false);
        let tls = tls_connector(tls_cas, &["h2", "http/1.1"]);
        let connector = HttpsConnector::from((http.clone(), tls.clone()));
        let client = Client::builder(TokioExecutor::new()).build(connector);
        let proxied = proxy.map(|proxy| {
            let connector = HttpsConnector::from((proxy.tunnel(http.clone()), tls.clone()));
            Client::builder(TokioExecutor::new()).build(connector)
        });
        Self {
            client,
            timeout_global,
            timeout_per_call,
            proxied,
        }
    }

    /// Whether a CONNECT proxy is configured.
    pub(crate) fn has_proxy(&self) -> bool {
        self.proxied.is_some()
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
        let response = match within(budget, TimeoutKind::Global, request_future).await {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => return Err(map_client_error(error, &host)),
            Err(error) => return Err(error),
        };
        let mut mapped = HeaderMap::new();
        for (name, value) in response.headers() {
            mapped.insert_wire(name.as_str(), value.as_bytes().to_vec());
        }
        Ok((response.status().as_u16(), mapped, response.into_body()))
    }
}

/// Awaits `future` under the call deadline. `fallback` names the phase when
/// neither the global nor the per-call deadline is the one that expired.
pub(crate) async fn within<T>(
    budget: CallBudget,
    fallback: TimeoutKind,
    future: impl Future<Output = T>,
) -> Result<T, NetError> {
    match budget.deadline() {
        Some(deadline) => {
            match tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), future).await {
                Ok(value) => Ok(value),
                Err(_) => Err(timed_out(budget.timeout_kind(fallback))),
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
            // The resolver phase fails before any socket exists: `hyper-util`
            // wraps `HostResolver`'s error in its connect error, and the typed
            // cause survives the wrapping.
            if cause.downcast_ref::<ResolveFailure>().is_some() {
                return NetError::Transport(TransportError::Dns(host.into()));
            }
            if cause.downcast_ref::<native_tls::Error>().is_some() && tls_reason.is_none() {
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

pub(crate) fn basic_authorization(username: &str, password: &str) -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"))
    )
}
