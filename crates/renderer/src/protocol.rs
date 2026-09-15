//! What a carrier exchanges with the engine, and the port it calls back on.
//!
//! These are the engine's own types: frame identity, mounts, page events,
//! dials, script results, and explicit errors. DOM handles, `QuickJS` values,
//! callbacks, and `net` types never cross. The messages that carry them over a
//! socket live in the browser crate's wire module, because only a carrier
//! needs to know how they travel.

use std::fmt;

use serde::{Deserialize, Serialize};
use url::Url;

/// Maximum aggregate bytes retained for one streamed response.
pub const MAX_RESPONSE_BODY_BYTES: usize = 1_048_576;

/// Renderer-process identity of one frame.
///
/// The renderer mints ids for the frames it hosts; the browser process routes
/// frame-addressed commands and events by it. The tab's main frame is
/// [`FrameId::MAIN`] in every renderer.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FrameId(u64);

impl FrameId {
    /// The tab's main frame, present in every renderer.
    pub const MAIN: Self = Self(0);

    /// Constructs a frame id from a protocol integer.
    #[must_use]
    pub fn new(raw: u64) -> Self {
        Self(raw)
    }

    /// Stable numeric identity for protocol messages.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Why a renderer API call was refused.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TabError {
    /// The URL string could not be parsed, joined, or was not `http`/`https`.
    InvalidUrl {
        /// The spec the caller passed.
        spec: String,
    },
    /// The renderer does not host the addressed frame.
    UnknownFrame {
        /// The frame id the caller passed.
        frame: u64,
    },
    /// `QuickJS` eval or a host callback failed.
    Script(ScriptFailure),
    /// The renderer is stopped.
    ActorStopped,
    /// The host could not start or reach a renderer.
    RendererUnavailable {
        /// Diagnostics from the spawn or handshake.
        message: String,
    },
    /// The browser refused work that would exceed a retained-resource budget.
    ResourceLimit {
        /// The exhausted budget.
        resource: ResourceLimit,
    },
}

impl fmt::Display for TabError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl { spec } => write!(f, "invalid url: {spec}"),
            Self::UnknownFrame { frame } => write!(f, "unknown frame: {frame}"),
            Self::Script(failure) => write!(f, "script: {failure}"),
            Self::ActorStopped => f.write_str("renderer stopped"),
            Self::RendererUnavailable { message } => {
                write!(f, "renderer unavailable: {message}")
            }
            Self::ResourceLimit { resource } => write!(f, "resource limit reached: {resource}"),
        }
    }
}

impl std::error::Error for TabError {}

/// Browser-owned retained-resource budgets exposed through [`TabError`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourceLimit {
    /// Concurrent waits retained by one tab coordinator.
    TabWaiters,
    /// Event subscriptions retained by one tab coordinator.
    TabSubscribers,
}

impl fmt::Display for ResourceLimit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TabWaiters => f.write_str("tab waiters"),
            Self::TabSubscribers => f.write_str("tab subscribers"),
        }
    }
}

/// Why `QuickJS` eval or a host callback failed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScriptFailure {
    /// Engine, parse, or thrown script error. `message` is diagnostics.
    Engine {
        /// Engine wording.
        message: Box<str>,
    },
    /// A timer slot id was not a valid array index.
    BadTimerId,
    /// The host was not present after construction. A defect if it surfaces.
    HostMissing,
    /// `QuickJS` interrupt handler stopped a long-running script.
    Interrupted,
}

impl fmt::Display for ScriptFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Engine { message } => f.write_str(message),
            Self::BadTimerId => f.write_str("bad timer id"),
            Self::HostMissing => f.write_str("js host missing"),
            Self::Interrupted => f.write_str("interrupted"),
        }
    }
}

impl std::error::Error for ScriptFailure {}

/// Observable HTML-job outcomes, in the order the renderer ran them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TabEvent {
    /// The document reached `readyState = "complete"` and dispatched `load`.
    Load,
    /// A child frame reached `load`; the top frame is unaffected.
    ChildLoad,
    /// A navigation committed; the document URL now reflects the final URL.
    Navigated,
    /// A navigation dial failed; the tab keeps its previous document.
    NavigationFailed,
    /// A host timer whose delay elapsed.
    Timer(u32),
    /// A `fetch` or navigation job finished with this HTTP status.
    Fetch {
        /// HTTP status.
        status: u16,
    },
    /// `send` or the body read failed.
    FetchFailed,
    /// A timer or `fetch` callback threw, or `execute_pending_job` failed.
    ScriptFailed,
}

/// One document to mount.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mount {
    /// Absolute document URL.
    pub url: String,
    /// HTTP `Content-Type`, when the document came from the network.
    pub content_type: Option<String>,
    /// HTTP `Content-Language`, when the document came from the network.
    pub content_language: Option<String>,
    /// Raw document bytes; the renderer decodes them.
    #[serde(skip, default)]
    pub body: Vec<u8>,
}

/// Why a browser-service dial failed. Preserved across the renderer seam.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DialFailure {
    /// Host lookup failed.
    Dns,
    /// TCP or proxy connect failed.
    Connect,
    /// TLS handshake or certificate verification failed.
    Tls,
    /// A deadline expired.
    Timeout,
    /// A body or redirect cap was exceeded.
    Limit,
    /// A network permit was not available before the deadline.
    QueueFull,
    /// Navigation replacement, tab close, or renderer death cancelled the dial.
    Cancelled,
}

/// Why the renderer is dialing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DialKind {
    /// A JavaScript `fetch`.
    JsFetch,
    /// A classic `<script src>` load.
    ClassicScript,
}

/// One blocking GET the renderer asks the browser process to perform.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DialRequest {
    /// Why the dial happens.
    pub kind: DialKind,
    /// Absolute request URL.
    pub url: String,
    /// Document URL used as the `SameSite`/`Sec-Fetch-*` initiator.
    pub initiator: String,
    /// Read the response body.
    pub read_body: bool,
}

/// Result of one blocking GET.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DialOutcome {
    /// HTTP status.
    pub status: u16,
    /// Final URL after redirects.
    pub final_url: String,
    /// `Content-Type` header, when present and UTF-8.
    pub content_type: Option<String>,
    /// `Content-Language` header, when present and a single tag.
    pub content_language: Option<String>,
    /// Response body, when [`DialRequest::read_body`].
    pub body: Vec<u8>,
}

/// Completion for a dial submitted to the browser process.
pub type DialCompletion = Box<dyn FnOnce(Result<DialOutcome, DialFailure>) + Send + 'static>;

/// Effects the page engine asks its host to perform.
///
/// A native renderer process receives an implementation that forwards calls
/// to the browser process. An embedded renderer receives an implementation
/// from its caller. Renderer code never names `net`.
pub trait BrowserServices: Send + Sync + 'static {
    /// Submits one GET without blocking the renderer thread. The completion
    /// receives [`DialFailure`] for transport, timeout, queue, body-limit, or
    /// cancellation failure.
    /// Implementations must invoke it exactly once, including when submission
    /// is rejected.
    fn start_dial(&self, request: DialRequest, completion: DialCompletion);

    /// `document.cookie` getter for `url`.
    fn cookies_for(&self, url: &Url) -> String;

    /// `document.cookie` setter for `url`.
    fn set_cookie(&self, value: &str, url: &Url);
}

#[cfg(test)]
mod tests {

    use crate::RemoteValue;

    fn round_trip<T>(message: &T)
    where
        T: serde::Serialize + serde::de::DeserializeOwned + std::fmt::Debug,
    {
        let value = serde_json::to_value(message).expect("serialize");
        let text = value.to_string();
        let back: T = serde_json::from_str(&text).expect("deserialize");
        let back_value = serde_json::to_value(&back).expect("serialize back");
        assert_eq!(value, back_value, "round trip {message:?}");
    }

    #[test]
    fn remote_value_round_trips_every_variant() {
        let values = [
            RemoteValue::Undefined,
            RemoteValue::Null,
            RemoteValue::Bool(true),
            RemoteValue::Number(1.5),
            RemoteValue::Number(-0.0),
            RemoteValue::String("x".into()),
            RemoteValue::List(vec![RemoteValue::Number(1.0), RemoteValue::Null]),
            RemoteValue::Map(vec![("k".into(), RemoteValue::Node(7))]),
            RemoteValue::Node(9),
        ];
        for value in values {
            round_trip(&value);
        }

        // JSON has no NaN or Infinity, so the value seam encodes them itself;
        // a carrier must be able to hand back exactly what it was given.
        for number in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let message = RemoteValue::Number(number);
            round_trip(&message);
            let text = serde_json::to_string(&message).expect("serialize");
            let value: RemoteValue = serde_json::from_str(&text).expect("deserialize");
            let RemoteValue::Number(value) = value else {
                panic!("non-finite number did not round trip");
            };
            if number.is_nan() {
                assert!(value.is_nan());
            } else {
                assert_eq!(value.to_bits(), number.to_bits());
            }
        }
    }
}
