/// Which timeout budget was exceeded.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TimeoutKind {
    #[error("global")]
    Global,
    #[error("per-call")]
    PerCall,
    #[error("resolve")]
    Resolve,
    #[error("connect")]
    Connect,
    #[error("send-request")]
    SendRequest,
    #[error("send-body")]
    SendBody,
    #[error("recv-response")]
    RecvResponse,
    #[error("recv-body")]
    RecvBody,
    /// A backend timeout name this crate does not map.
    #[error("unknown ({0})")]
    Unknown(Box<str>),
}

/// Failure to dial, complete TLS, or transfer bytes.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// DNS lookup failed for this host.
    #[error("dns lookup failed for {0}")]
    Dns(Box<str>),
    /// TCP or proxy CONNECT failed. The string is a short reason, such as `CONNECT 403`.
    #[error("connection failed: {0}")]
    Connect(Box<str>),
    /// TLS handshake or certificate verification failed.
    #[error("tls failure: {0}")]
    Tls(Box<str>),
    #[error("{0} timeout exceeded")]
    Timeout(TimeoutKind),
    #[error("io error: {0}")]
    Io(#[source] std::io::Error),
}

/// A configured cap was exceeded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LimitExceeded {
    /// Redirect hop count reached [`AgentBuilder::max_redirects`].
    #[error("redirect cap exceeded")]
    Redirect,
    /// Response body would exceed the caller-supplied byte cap.
    #[error("size cap exceeded: {0} bytes")]
    Size(u64),
}

/// The request or response could not be represented as HTTP.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProtocolError {
    /// A response header name or value is not a valid HTTP field.
    #[error("backend produced an unrepresentable header")]
    UnrepresentableHeader,
    /// The URL, method, or assembled request was rejected.
    #[error("backend rejected the assembled request")]
    RejectedRequest,
    /// Proxy URI is not an `http://` HTTP CONNECT authority with a host.
    #[error("proxy URI must be an http:// HTTP CONNECT authority")]
    InvalidProxy,
    /// `--resolve=PATTERN=ADDR` is not `PATTERN=IPv4` or `PATTERN=fail`.
    #[error("resolve spec must be PATTERN=IPv4 or PATTERN=fail")]
    InvalidResolve,
    /// Another protocol failure, with the backend's wording.
    #[error("{0}")]
    Other(Box<str>),
}

/// Failure from [`RequestBuilder::send`], [`RequestBuilder::upgrade`], or body reads.
#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("transport: {0}")]
    Transport(#[source] TransportError),
    #[error("protocol violation: {0}")]
    Protocol(#[source] ProtocolError),
    #[error("limit exceeded: {0}")]
    Limit(#[source] LimitExceeded),
}
