//! The [`NetError`] taxonomy: every failure `send()` can surface.
//!
//! Statuses are data (decision 02), so nothing status-shaped lives here.
//! Three arms, fixed by decision 02: transport failures (the dial or the
//! socket died), protocol violations (the peer spoke nonsense), and limits
//! (we refused to go further). The `From<ureq::Error>` impl is the single
//! backend-conversion point for errors; when the stealth swap replaces
//! ureq, only this mapping changes.

use std::fmt;

/// Which knob fired when an operation took too long.
///
/// Modeled after the backend's own timeout enumeration; [`TimeoutKind::Unknown`]
/// catches knobs this version doesn't model yet (the upstream enum is
/// `#[non_exhaustive]`) without inventing a plausible-sounding lie.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimeoutKind {
    /// End-to-end budget including body reads.
    Global,
    /// Budget reset at each redirect hop.
    PerCall,
    /// DNS resolution.
    Resolve,
    /// TCP connect plus TLS handshake.
    Connect,
    /// Writing the request head.
    SendRequest,
    /// Writing the request body.
    SendBody,
    /// Waiting for the response head.
    RecvResponse,
    /// Reading the response body.
    RecvBody,
    /// A timeout knob this net version doesn't model; carries the
    /// backend's own spelling of it so diagnostics stay truthful.
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

/// Why a transport-level operation failed: the dial, the handshake, or the
/// socket itself.
#[derive(Debug)]
pub enum TransportError {
    /// The host did not resolve.
    Dns(Box<str>),
    /// The TCP connection could not be established (or a CONNECT proxy
    /// refused us).
    Connect(Box<str>),
    /// TLS failed: handshake, certificate, or policy rejection. Populated
    /// from ticket 12 onward.
    Tls(Box<str>),
    /// A configured timeout fired; the payload names which knob.
    Timeout(TimeoutKind),
    /// The socket died mid-operation.
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

/// Which configured cap was hit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitExceeded {
    /// More redirects than the agent's cap allows.
    Redirect,
    /// Response bytes beyond a cap; the payload is the cap that fired —
    /// the caller's own limit for [`crate::Body::bytes`] /
    /// [`crate::Body::text`], or the backend's received-header size cap
    /// (same refusal shape: response data too big to accept).
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

/// Everything `send()` can fail with.
///
/// Expected failures only, as values (repo rule): callers match and recover.
/// Nothing here panics.
#[derive(Debug)]
pub enum NetError {
    /// The dial or the socket failed.
    Transport(TransportError),
    /// The peer violated HTTP enough that we refuse to continue, or the
    /// backend rejected a locally-built request.
    Protocol(Box<str>),
    /// A configured cap was hit.
    Limit(LimitExceeded),
}

impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(err) => write!(f, "transport: {err}"),
            Self::Protocol(reason) => write!(f, "protocol violation: {reason}"),
            Self::Limit(limit) => write!(f, "limit exceeded: {limit}"),
        }
    }
}

impl std::error::Error for NetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transport(err) => Some(err),
            _ => None,
        }
    }
}

/// The error conversion point: ureq's flat error surface folded into our
/// three-arm taxonomy. Every arm is exhaustive over ureq 3's
/// `#[non_exhaustive]` enum with one deliberate fallback (`Other`-style
/// variants and feature-disabled arms land in `Protocol`).
impl From<ureq::Error> for NetError {
    fn from(err: ureq::Error) -> Self {
        use ureq::Error as U;
        match err {
            // Transport: dial and socket failures.
            U::HostNotFound => Self::Transport(TransportError::Dns("host not found".into())),
            U::ConnectionFailed => {
                Self::Transport(TransportError::Connect("connection failed".into()))
            }
            U::ConnectProxyFailed(detail) => {
                Self::Transport(TransportError::Connect(detail.into()))
            }
            U::Io(err) => Self::Transport(TransportError::Io(err)),
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
                    // Unknown future knobs (and `Await100`, which is
                    // #[doc(hidden)] upstream and "never seen outside
                    // ureq" — ureq 3.4 src/timings.rs) carry the backend's
                    // own spelling rather than a fabricated category.
                    other => TimeoutKind::Unknown(format!("{other:?}").into()),
                };
                Self::Transport(TransportError::Timeout(kind))
            }
            U::Tls(detail) => Self::Transport(TransportError::Tls(detail.into())),

            // Limits: keep the cap that fired so diagnostics explain
            // themselves.
            U::TooManyRedirects => Self::Limit(LimitExceeded::Redirect),
            U::BodyExceedsLimit(cap) => Self::Limit(LimitExceeded::Size(cap)),
            U::LargeResponseHeader(_, cap) => {
                // Lossless on this crate's x86_64 target; no `From<usize> for u64`.
                Self::Limit(LimitExceeded::Size(cap as u64))
            }

            // Everything else is a protocol-shaped refusal, including
            // unreachable-by-config arms (StatusCode cannot fire with
            // http_status_as_error(false); cookie/charset/json features are
            // compiled out). TLS handshake failures are the `U::Tls` arm
            // above; ticket 12 adds native-tls so that arm can actually
            // fire from a dial.
            other => Self::Protocol(other.to_string().into()),
        }
    }
}
