/// Which timeout budget was exceeded.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TimeoutKind {
    #[error("global")]
    Global,
    #[error("per-call")]
    PerCall,
    #[error("recv-body")]
    RecvBody,
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
    /// Redirect hop count reached [`crate::AgentOptions::max_redirects`].
    #[error("redirect cap exceeded")]
    Redirect,
    /// Response body would exceed the caller-supplied byte cap.
    #[error("size cap exceeded: {0} bytes")]
    Size(u64),
}

/// Failure from [`crate::Agent::send`] or body reads.
#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("transport: {0}")]
    Transport(#[source] TransportError),
    /// The URL, method, or assembled request could not be represented as
    /// HTTP, so the request was refused before it left the agent.
    #[error("protocol violation: backend rejected the assembled request")]
    RejectedRequest,
    #[error("limit exceeded: {0}")]
    Limit(#[source] LimitExceeded),
    #[error("content decoding failed: {0}")]
    Decode(Box<str>),
}
