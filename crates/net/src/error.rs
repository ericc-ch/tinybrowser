use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimeoutKind {
    Global,
    PerCall,
    Resolve,
    Connect,
    SendRequest,
    SendBody,
    RecvResponse,
    RecvBody,
    Unknown(Box<str>),
}

impl fmt::Display for TimeoutKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Global => f.write_str("global"),
            Self::PerCall => f.write_str("per-call"),
            Self::Resolve => f.write_str("resolve"),
            Self::Connect => f.write_str("connect"),
            Self::SendRequest => f.write_str("send-request"),
            Self::SendBody => f.write_str("send-body"),
            Self::RecvResponse => f.write_str("recv-response"),
            Self::RecvBody => f.write_str("recv-body"),
            Self::Unknown(name) => write!(f, "unknown ({name})"),
        }
    }
}

#[derive(Debug)]
pub enum TransportError {
    Dns(Box<str>),
    Connect(Box<str>),
    Tls(Box<str>),
    Timeout(TimeoutKind),
    Io(std::io::Error),
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dns(host) => write!(f, "dns lookup failed for {host}"),
            Self::Connect(detail) => write!(f, "connection failed: {detail}"),
            Self::Tls(detail) => write!(f, "tls failure: {detail}"),
            Self::Timeout(kind) => write!(f, "{kind} timeout exceeded"),
            Self::Io(err) => write!(f, "io error: {err}"),
        }
    }
}

impl std::error::Error for TransportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitExceeded {
    Redirect,
    Size(u64),
}

impl fmt::Display for LimitExceeded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Redirect => f.write_str("redirect cap exceeded"),
            Self::Size(cap) => write!(f, "size cap exceeded: {cap} bytes"),
        }
    }
}

impl std::error::Error for LimitExceeded {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProtocolError {
    UnrepresentableHeader,
    RejectedRequest,
    InvalidProxy,
    Other(Box<str>),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnrepresentableHeader => {
                f.write_str("backend produced an unrepresentable header")
            }
            Self::RejectedRequest => f.write_str("backend rejected the assembled request"),
            Self::InvalidProxy => {
                f.write_str("proxy URI must be an http:// HTTP CONNECT authority")
            }
            Self::Other(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for ProtocolError {}

#[derive(Debug)]
pub enum NetError {
    Transport(TransportError),
    Protocol(ProtocolError),
    Limit(LimitExceeded),
}

impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(err) => write!(f, "transport: {err}"),
            Self::Protocol(err) => write!(f, "protocol violation: {err}"),
            Self::Limit(limit) => write!(f, "limit exceeded: {limit}"),
        }
    }
}

impl std::error::Error for NetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transport(err) => Some(err),
            Self::Protocol(err) => Some(err),
            Self::Limit(err) => Some(err),
        }
    }
}

#[derive(Debug)]
pub(crate) struct DialTlsFailure(pub Box<str>);

impl fmt::Display for DialTlsFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DialTlsFailure {}

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
            U::TooManyRedirects => Self::Limit(LimitExceeded::Redirect),
            U::BodyExceedsLimit(cap) => Self::Limit(LimitExceeded::Size(cap)),
            U::LargeResponseHeader(_, cap) => Self::Limit(LimitExceeded::Size(cap as u64)),
            U::Http(_) => Self::Protocol(ProtocolError::RejectedRequest),
            other => Self::Protocol(ProtocolError::Other(other.to_string().into())),
        }
    }
}
