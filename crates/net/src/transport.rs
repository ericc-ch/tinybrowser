use std::cell::Cell;
use std::fmt;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::str::FromStr as _;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine as _;
use native_tls::{TlsConnector, TlsStream};
use ureq::unversioned::resolver::{ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{
    Buffers, ConnectionDetails, Connector, LazyBuffers, NextTimeout, Transport,
};
use url::Url;

use crate::error::{NetError, ProtocolError, TimeoutKind, TransportError};
use crate::protocol::{HeaderMap, Method};
use crate::resolve::{HostMap, Mapped};

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct DialTlsFailure(Box<str>);

thread_local! {
    static CALL_BUDGET: Cell<CallBudget> = const { Cell::new(CallBudget::NONE) };
}

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
    pub const NONE: Self = Self {
        global: None,
        hop: None,
    };

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

pub(crate) fn enter_budget(budget: CallBudget) -> BudgetGuard {
    CALL_BUDGET.set(budget);
    BudgetGuard
}

pub(crate) struct BudgetGuard;

impl Drop for BudgetGuard {
    fn drop(&mut self) {
        CALL_BUDGET.set(CallBudget::NONE);
    }
}

fn current_budget() -> CallBudget {
    CALL_BUDGET.get()
}

#[derive(Clone)]
pub(crate) struct HttpEngine {
    inner: ureq::Agent,
    pub(crate) proxy: Option<String>,
    pub(crate) timeout_global: Option<Duration>,
    pub(crate) timeout_per_call: Option<Duration>,
    pub(crate) host_map: HostMap,
}

impl HttpEngine {
    pub(crate) fn new(
        timeout_global: Option<Duration>,
        timeout_per_call: Option<Duration>,
        proxy: Option<String>,
        host_map: HostMap,
    ) -> Self {
        let config = ureq::config::Config::builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_global(None)
            .timeout_per_call(None)
            .user_agent(ureq::config::AutoHeaderValue::None)
            .accept(ureq::config::AutoHeaderValue::None)
            .accept_encoding(ureq::config::AutoHeaderValue::None)
            .proxy(None)
            .allow_non_standard_methods(true)
            .tls_config(
                ureq::tls::TlsConfig::builder()
                    .provider(ureq::tls::TlsProvider::NativeTls)
                    .build(),
            )
            .build();
        let inner = ureq::Agent::with_parts(
            config,
            NetConnector {
                proxy: proxy.clone(),
                host_map: host_map.clone(),
            },
            DialResolver,
        );
        Self {
            inner,
            proxy,
            timeout_global,
            timeout_per_call,
            host_map,
        }
    }

    pub(crate) fn budget_at(&self, start: Instant) -> CallBudget {
        CallBudget::from_engine(self.timeout_global, self.timeout_per_call, start)
    }

    pub(crate) fn send(
        &self,
        method: &Method,
        wire_url: &Url,
        headers: &HeaderMap,
        body: Option<&[u8]>,
        budget: CallBudget,
    ) -> Result<(u16, HeaderMap, Box<dyn Read + Send>), NetError> {
        if budget.is_expired() {
            return Err(timed_out(budget.timeout_kind(TimeoutKind::Global)));
        }
        let _guard = enter_budget(budget);
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
            && let Some(value) = proxy_basic_token(self.proxy.as_deref())
        {
            builder = builder.header("Proxy-Authorization", value);
        }

        let rejected = |_| NetError::Protocol(ProtocolError::RejectedRequest);
        let response = match body {
            Some(bytes) => self
                .inner
                .run(builder.body(bytes.to_vec()).map_err(rejected)?)
                .map_err(NetError::from)?,
            None => self
                .inner
                .run(builder.body(()).map_err(rejected)?)
                .map_err(NetError::from)?,
        };
        let status = response.status().as_u16();
        let mut mapped = HeaderMap::new();
        for (name, value) in response.headers() {
            mapped
                .insert(name.as_str(), value.as_bytes())
                .map_err(|_| NetError::Protocol(ProtocolError::UnrepresentableHeader))?;
        }
        Ok((
            status,
            mapped,
            Box::new(BudgetedReader {
                inner: response.into_body().into_reader(),
                budget,
            }),
        ))
    }
}

struct BudgetedReader<R> {
    inner: R,
    budget: CallBudget,
}

impl<R: Read> Read for BudgetedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.budget.is_expired() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "global timeout exceeded",
            ));
        }
        let _guard = enter_budget(self.budget);
        self.inner.read(buf)
    }
}

impl From<ureq::Error> for NetError {
    fn from(err: ureq::Error) -> Self {
        use ureq::Error as U;
        match err {
            U::HostNotFound => Self::Transport(TransportError::Dns("host not found".into())),
            U::ConnectionFailed => {
                Self::Transport(TransportError::Connect("connection failed".into()))
            }
            U::ConnectProxyFailed(detail) => {
                Self::Transport(TransportError::Connect(detail.into()))
            }
            U::Io(err) => {
                if let Some(tls) = err
                    .get_ref()
                    .and_then(|inner| inner.downcast_ref::<DialTlsFailure>())
                {
                    return Self::Transport(TransportError::Tls(tls.0.clone()));
                }
                Self::Transport(TransportError::Io(err))
            }
            U::Timeout(which) => {
                use ureq::Timeout as T;
                let kind = match which {
                    T::Global => TimeoutKind::Global,
                    T::PerCall => TimeoutKind::PerCall,
                    T::Resolve => TimeoutKind::Resolve,
                    T::Connect => TimeoutKind::Connect,
                    T::SendRequest => TimeoutKind::SendRequest,
                    T::SendBody => TimeoutKind::SendBody,
                    T::RecvResponse => TimeoutKind::RecvResponse,
                    T::RecvBody => TimeoutKind::RecvBody,
                    other => TimeoutKind::Unknown(format!("{other:?}").into()),
                };
                Self::Transport(TransportError::Timeout(kind))
            }
            U::Tls(detail) => Self::Transport(TransportError::Tls(detail.into())),
            U::NativeTls(err) => Self::Transport(TransportError::Tls(err.to_string().into())),
            U::Der(err) => Self::Transport(TransportError::Tls(err.to_string().into())),
            U::TooManyRedirects => Self::Limit(crate::error::LimitExceeded::Redirect),
            U::BodyExceedsLimit(cap) => Self::Limit(crate::error::LimitExceeded::Size(cap)),
            U::LargeResponseHeader(_, cap) => {
                Self::Limit(crate::error::LimitExceeded::Size(cap as u64))
            }
            U::Http(_) => Self::Protocol(ProtocolError::RejectedRequest),
            other => Self::Protocol(ProtocolError::Other(other.to_string().into())),
        }
    }
}

#[derive(Debug, Default)]
struct DialResolver;

impl Resolver for DialResolver {
    fn resolve(
        &self,
        _uri: &ureq::http::Uri,
        _config: &ureq::config::Config,
        _timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        Ok(self.empty())
    }
}

#[derive(Clone)]
struct NetConnector {
    proxy: Option<String>,
    host_map: HostMap,
}

impl fmt::Debug for NetConnector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NetConnector")
            .field("has_proxy", &self.proxy.is_some())
            .field("has_host_map", &!self.host_map.is_empty())
            .finish_non_exhaustive()
    }
}

impl Connector for NetConnector {
    type Out = StreamTransport;

    fn connect(
        &self,
        details: &ConnectionDetails,
        chained: Option<()>,
    ) -> Result<Option<Self::Out>, ureq::Error> {
        if chained.is_some() {
            return Err(ureq::Error::ConnectionFailed);
        }
        let url =
            Url::parse(&details.uri.to_string()).map_err(|_| ureq::Error::ConnectionFailed)?;
        let stream = open(
            &url,
            self.proxy.as_deref(),
            hop_deadline(details),
            &self.host_map,
        )
        .map_err(to_ureq)?;
        let buffers = LazyBuffers::new(
            details.config.input_buffer_size(),
            details.config.output_buffer_size(),
        );
        Ok(Some(StreamTransport {
            stream: Mutex::new(stream),
            buffers,
        }))
    }
}

struct StreamTransport {
    stream: Mutex<RawStream>,
    buffers: LazyBuffers,
}

impl Transport for StreamTransport {
    fn buffers(&mut self) -> &mut dyn Buffers {
        &mut self.buffers
    }

    fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), ureq::Error> {
        let mut stream = self
            .stream
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        apply_timeout(&stream, timeout, true)?;
        let output = &self.buffers.output()[..amount];
        stream
            .write_all(output)
            .map_err(|err| map_io(err, timeout))?;
        Ok(())
    }

    fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, ureq::Error> {
        let mut stream = self
            .stream
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        apply_timeout(&stream, timeout, false)?;
        let input = self.buffers.input_append_buf();
        let amount = stream.read(input).map_err(|err| map_io(err, timeout))?;
        self.buffers.input_appended(amount);
        Ok(amount > 0)
    }

    fn is_open(&mut self) -> bool {
        let stream = self
            .stream
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        stream.peek_open()
    }

    fn is_tls(&self) -> bool {
        self.stream
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_tls()
    }
}

impl fmt::Debug for StreamTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamTransport").finish_non_exhaustive()
    }
}

fn hop_deadline(details: &ConnectionDetails) -> Option<Instant> {
    let from_ureq = match details.timeout.not_zero() {
        Some(ureq::unversioned::transport::time::Duration::Exact(duration)) => {
            Some(Instant::now() + duration)
        }
        _ => None,
    };
    match (current_budget().deadline(), from_ureq) {
        (Some(budget), Some(ureq_end)) => Some(budget.min(ureq_end)),
        (budget, ureq_end) => budget.or(ureq_end),
    }
}

fn socket_timeout(timeout: NextTimeout) -> Option<Duration> {
    let from_ureq = match timeout.not_zero() {
        Some(ureq::unversioned::transport::time::Duration::Exact(duration)) => Some(duration),
        _ => None,
    };
    match (current_budget().remaining(), from_ureq) {
        (Some(budget), Some(ureq_end)) => Some(budget.min(ureq_end)),
        (budget, ureq_end) => budget.or(ureq_end),
    }
}

fn apply_timeout(stream: &RawStream, timeout: NextTimeout, write: bool) -> Result<(), ureq::Error> {
    let dur = socket_timeout(timeout);
    if dur == Some(Duration::ZERO) {
        return Err(budget_timeout(timeout));
    }
    if write {
        stream.set_write_timeout(dur).map_err(ureq::Error::from)?;
    } else {
        stream.set_read_timeout(dur).map_err(ureq::Error::from)?;
    }
    Ok(())
}

fn budget_timeout(timeout: NextTimeout) -> ureq::Error {
    let kind =
        current_budget().timeout_kind(TimeoutKind::Unknown(format!("{:?}", timeout.reason).into()));
    let mapped = match kind {
        TimeoutKind::PerCall => ureq::Timeout::PerCall,
        TimeoutKind::Connect => ureq::Timeout::Connect,
        TimeoutKind::Resolve => ureq::Timeout::Resolve,
        TimeoutKind::Global
        | TimeoutKind::SendRequest
        | TimeoutKind::SendBody
        | TimeoutKind::RecvResponse
        | TimeoutKind::RecvBody
        | TimeoutKind::Unknown(_) => ureq::Timeout::Global,
    };
    ureq::Error::Timeout(mapped)
}

fn map_io(err: std::io::Error, timeout: NextTimeout) -> ureq::Error {
    match err.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => budget_timeout(timeout),
        _ => ureq::Error::from(err),
    }
}

fn to_ureq(err: NetError) -> ureq::Error {
    match err {
        NetError::Transport(TransportError::Io(e)) => ureq::Error::Io(e),
        NetError::Transport(TransportError::Tls(detail)) => {
            ureq::Error::Io(std::io::Error::other(DialTlsFailure(detail)))
        }
        NetError::Transport(TransportError::Connect(detail)) => {
            ureq::Error::ConnectProxyFailed(detail.into())
        }
        NetError::Transport(TransportError::Dns(_)) => ureq::Error::HostNotFound,
        NetError::Transport(TransportError::Timeout(kind)) => {
            let t = match kind {
                TimeoutKind::PerCall => ureq::Timeout::PerCall,
                TimeoutKind::Connect => ureq::Timeout::Connect,
                TimeoutKind::Resolve => ureq::Timeout::Resolve,
                TimeoutKind::Global
                | TimeoutKind::SendRequest
                | TimeoutKind::SendBody
                | TimeoutKind::RecvResponse
                | TimeoutKind::RecvBody
                | TimeoutKind::Unknown(_) => ureq::Timeout::Global,
            };
            ureq::Error::Timeout(t)
        }
        NetError::Protocol(ProtocolError::InvalidProxy) => ureq::Error::InvalidProxyUrl,
        _ => ureq::Error::ConnectionFailed,
    }
}

pub(crate) struct Socket {
    tcp: TcpStream,
    prefix: Vec<u8>,
}

impl Socket {
    fn new(tcp: TcpStream, prefix: Vec<u8>) -> Self {
        Self { tcp, prefix }
    }
}

impl Read for Socket {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if !self.prefix.is_empty() {
            let n = buf.len().min(self.prefix.len());
            buf[..n].copy_from_slice(&self.prefix[..n]);
            self.prefix.drain(..n);
            return Ok(n);
        }
        self.tcp.read(buf)
    }
}

impl Write for Socket {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.tcp.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.tcp.flush()
    }
}

pub(crate) enum RawStream {
    Plain(Socket),
    Tls(TlsStream<Socket>),
}

impl RawStream {
    fn tcp(&self) -> &TcpStream {
        match self {
            Self::Plain(s) => &s.tcp,
            Self::Tls(s) => &s.get_ref().tcp,
        }
    }

    pub(crate) fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        self.tcp().set_read_timeout(timeout)
    }

    pub(crate) fn set_write_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        self.tcp().set_write_timeout(timeout)
    }

    pub(crate) fn is_tls(&self) -> bool {
        matches!(self, Self::Tls(_))
    }

    pub(crate) fn peek_open(&self) -> bool {
        if let Self::Plain(s) = self
            && !s.prefix.is_empty()
        {
            return true;
        }
        let tcp = self.tcp();
        if tcp.set_nonblocking(true).is_err() {
            return false;
        }
        let mut buf = [0];
        let open = match tcp.peek(&mut buf) {
            Err(err)
                if err.kind() == std::io::ErrorKind::WouldBlock
                    || err.kind() == std::io::ErrorKind::TimedOut =>
            {
                true
            }
            Ok(0) | Err(_) => false,
            Ok(_) => true,
        };
        if tcp.set_nonblocking(false).is_err() {
            return false;
        }
        open
    }
}

impl Read for RawStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(s) => s.read(buf),
            Self::Tls(s) => s.read(buf),
        }
    }
}

impl Write for RawStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(s) => s.write(buf),
            Self::Tls(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(s) => s.flush(),
            Self::Tls(s) => s.flush(),
        }
    }
}

pub(crate) fn open(
    url: &Url,
    proxy: Option<&str>,
    deadline: Option<Instant>,
    host_map: &HostMap,
) -> Result<RawStream, NetError> {
    if remaining(deadline) == Some(Duration::ZERO) {
        return Err(timed_out(
            current_budget().timeout_kind(TimeoutKind::Connect),
        ));
    }
    let host = url
        .host_str()
        .ok_or(NetError::Protocol(ProtocolError::RejectedRequest))?;
    let tls = matches!(url.scheme(), "https" | "wss");
    let tunnel = tls || url.scheme() == "ws";
    let port = url
        .port_or_known_default()
        .unwrap_or(if tls { 443 } else { 80 });
    let socket = if let Some(proxy) = proxy {
        if tunnel {
            connect_via_proxy(proxy, host, port, deadline, host_map)?
        } else {
            tcp_to_proxy(proxy, deadline, host_map)?
        }
    } else {
        Socket::new(connect_tcp(host, port, deadline, host_map)?, Vec::new())
    };
    let _ = socket.tcp.set_nodelay(true);
    if tls {
        let timeout = remaining(deadline);
        if timeout == Some(Duration::ZERO) {
            return Err(timed_out(
                current_budget().timeout_kind(TimeoutKind::Connect),
            ));
        }
        socket
            .tcp
            .set_read_timeout(timeout)
            .map_err(map_connect_io)?;
        socket
            .tcp
            .set_write_timeout(timeout)
            .map_err(map_connect_io)?;
        let connector = TlsConnector::new()
            .map_err(|err| NetError::Transport(TransportError::Tls(err.to_string().into())))?;
        let tls_stream = match connector.connect(host, socket) {
            Ok(s) => s,
            Err(native_tls::HandshakeError::Failure(err)) => {
                return Err(NetError::Transport(TransportError::Tls(
                    err.to_string().into(),
                )));
            }
            Err(_) => {
                return Err(NetError::Transport(TransportError::Tls(
                    "tls handshake interrupted".into(),
                )));
            }
        };
        Ok(RawStream::Tls(tls_stream))
    } else {
        Ok(RawStream::Plain(socket))
    }
}

fn remaining(deadline: Option<Instant>) -> Option<Duration> {
    deadline.map(|end| end.saturating_duration_since(Instant::now()))
}

fn timed_out(kind: TimeoutKind) -> NetError {
    NetError::Transport(TransportError::Timeout(kind))
}

fn connect_tcp(
    host: &str,
    port: u16,
    deadline: Option<Instant>,
    host_map: &HostMap,
) -> Result<TcpStream, NetError> {
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    if remaining(deadline) == Some(Duration::ZERO) {
        return Err(timed_out(
            current_budget().timeout_kind(TimeoutKind::Resolve),
        ));
    }
    let addrs = resolve_addrs(host, port, remaining(deadline), host_map)?;
    let mut last = None;
    for addr in addrs {
        let leftover = remaining(deadline);
        if leftover == Some(Duration::ZERO) {
            return Err(timed_out(
                current_budget().timeout_kind(TimeoutKind::Connect),
            ));
        }
        let result = match leftover {
            Some(limit) => TcpStream::connect_timeout(&addr, limit),
            None => TcpStream::connect(addr),
        };
        match result {
            Ok(stream) => return Ok(stream),
            Err(err) => last = Some(err),
        }
    }
    match last {
        Some(err) => Err(map_connect_io(err)),
        None => Err(NetError::Transport(TransportError::Dns(host.into()))),
    }
}

fn resolve_addrs(
    host: &str,
    port: u16,
    timeout: Option<Duration>,
    host_map: &HostMap,
) -> Result<Vec<SocketAddr>, NetError> {
    match host_map.lookup(host) {
        Some(Mapped::Fail) => {
            return Err(NetError::Transport(TransportError::Dns(host.into())));
        }
        Some(Mapped::Addr(ip)) => return Ok(vec![SocketAddr::from((ip, port))]),
        None => {}
    }
    let Some(limit) = timeout else {
        return (host, port)
            .to_socket_addrs()
            .map(Iterator::collect)
            .map_err(|_| NetError::Transport(TransportError::Dns(host.into())));
    };
    let host_owned = host.to_owned();
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("net-dns".into())
        .spawn(move || {
            let resolved = (host_owned.as_str(), port)
                .to_socket_addrs()
                .map(Iterator::collect);
            let _ = tx.send(resolved);
        })
        .map_err(map_connect_io)?;
    match rx.recv_timeout(limit) {
        Ok(Ok(addrs)) => Ok(addrs),
        Ok(Err(_)) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Err(NetError::Transport(TransportError::Dns(host.into())))
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(timed_out(
            current_budget().timeout_kind(TimeoutKind::Resolve),
        )),
    }
}

fn tcp_to_proxy(
    proxy: &str,
    deadline: Option<Instant>,
    host_map: &HostMap,
) -> Result<Socket, NetError> {
    let proxy_url =
        Url::parse(proxy).map_err(|_| NetError::Protocol(ProtocolError::InvalidProxy))?;
    let phost = proxy_url
        .host_str()
        .ok_or(NetError::Protocol(ProtocolError::InvalidProxy))?;
    let proxy_port = proxy_url.port_or_known_default().unwrap_or(80);
    Ok(Socket::new(
        connect_tcp(phost, proxy_port, deadline, host_map)?,
        Vec::new(),
    ))
}

pub(crate) fn proxy_basic_token(proxy: Option<&str>) -> Option<String> {
    let proxy = proxy?;
    let proxy_url = Url::parse(proxy).ok()?;
    if proxy_url.username().is_empty() {
        return None;
    }
    let password = proxy_url.password().unwrap_or("");
    Some(basic_authorization(proxy_url.username(), password))
}

pub(crate) fn basic_authorization(username: &str, password: &str) -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"))
    )
}

fn connect_via_proxy(
    proxy: &str,
    host: &str,
    port: u16,
    deadline: Option<Instant>,
    host_map: &HostMap,
) -> Result<Socket, NetError> {
    let proxy_url =
        Url::parse(proxy).map_err(|_| NetError::Protocol(ProtocolError::InvalidProxy))?;
    let phost = proxy_url
        .host_str()
        .ok_or(NetError::Protocol(ProtocolError::InvalidProxy))?;
    let proxy_port = proxy_url.port_or_known_default().unwrap_or(80);
    let mut stream = connect_tcp(phost, proxy_port, deadline, host_map)?;
    let timeout = remaining(deadline);
    if timeout == Some(Duration::ZERO) {
        return Err(timed_out(
            current_budget().timeout_kind(TimeoutKind::Connect),
        ));
    }
    stream.set_read_timeout(timeout).map_err(map_connect_io)?;
    stream.set_write_timeout(timeout).map_err(map_connect_io)?;
    let authority = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    let mut req = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
    if let Some(value) = proxy_basic_token(Some(proxy)) {
        req.push_str("Proxy-Authorization: ");
        req.push_str(&value);
        req.push_str("\r\n");
    }
    req.push_str("\r\n");
    stream.write_all(req.as_bytes()).map_err(map_connect_io)?;
    stream.flush().map_err(map_connect_io)?;
    let mut buf = Vec::new();
    loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let leftover = buf[end + 4..].to_vec();
            let head = String::from_utf8_lossy(&buf[..end]);
            let status_line = head.lines().next().unwrap_or("");
            let mut tokens = status_line.split_whitespace();
            let version = tokens.next().unwrap_or("");
            let status = tokens
                .next()
                .and_then(|s| s.parse::<u16>().ok())
                .unwrap_or(0);
            if !version.starts_with("HTTP/") || status != 200 {
                return Err(NetError::Transport(TransportError::Connect(
                    format!("CONNECT {status}").into(),
                )));
            }
            return Ok(Socket::new(stream, leftover));
        }
        if buf.len() > 64 * 1024 {
            return Err(NetError::Protocol(ProtocolError::RejectedRequest));
        }
        let mut chunk = [0u8; 512];
        let n = stream.read(&mut chunk).map_err(map_connect_io)?;
        if n == 0 {
            return Err(NetError::Protocol(ProtocolError::RejectedRequest));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

fn map_connect_io(err: std::io::Error) -> NetError {
    match err.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => {
            timed_out(current_budget().timeout_kind(TimeoutKind::Connect))
        }
        _ => NetError::Transport(TransportError::Io(err)),
    }
}
