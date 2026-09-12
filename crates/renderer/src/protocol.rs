//! The value-only seam between browser process and renderer process.
//!
//! [ADR 0011](../../../docs/adrs/0011-renderer-processes-per-site.md): commands,
//! request ids, events, script results, and explicit errors cross. DOM handles,
//! `QuickJS` values, callbacks, and `net` types never do. The same types back the
//! in-process backend and the `--renderer` pipe.

use std::fmt;
use std::io::{self, BufRead, Read};
use std::sync::Arc;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use url::Url;

use crate::RemoteValue;

/// Maximum encoded size of one browser/renderer IPC message.
///
/// Navigation bodies are capped at 1 MiB by the browser. The larger wire cap
/// leaves room for JSON's byte-array expansion and structured script results
/// while bounding allocation before deserialization.
pub const MAX_IPC_MESSAGE_BYTES: usize = 8 * 1024 * 1024;

/// Encodes one newline-delimited IPC message after enforcing the wire budget.
///
/// # Errors
///
/// Serialization failure or a message larger than [`MAX_IPC_MESSAGE_BYTES`].
pub fn encode_ipc_message<T: Serialize>(message: &T) -> io::Result<Vec<u8>> {
    let encoded = serde_json::to_vec(message).map_err(io::Error::other)?;
    if encoded.len() > MAX_IPC_MESSAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "renderer IPC message exceeds limit",
        ));
    }
    Ok(encoded)
}

/// Reads one newline-delimited IPC message without permitting an unbounded
/// line allocation.
///
/// # Errors
///
/// I/O failure, invalid JSON, a missing delimiter, or an oversized message.
pub fn read_ipc_message<T: DeserializeOwned>(
    reader: &mut impl BufRead,
    buffer: &mut Vec<u8>,
) -> io::Result<Option<T>> {
    buffer.clear();
    let mut limited = reader.take((MAX_IPC_MESSAGE_BYTES + 2) as u64);
    let read = limited.read_until(b'\n', buffer)?;
    if read == 0 {
        return Ok(None);
    }
    if !buffer.ends_with(b"\n") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "renderer IPC message exceeds limit or is not delimited",
        ));
    }
    buffer.pop();
    if buffer.ends_with(b"\r") {
        buffer.pop();
    }
    if buffer.len() > MAX_IPC_MESSAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "renderer IPC message exceeds limit",
        ));
    }
    serde_json::from_slice(buffer)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Renderer-process identity of one frame.
///
/// [ADR 0014](../../../docs/adrs/0014-frames-and-per-frame-realms.md): the
/// renderer mints ids for the frames it hosts; the browser process routes
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
        }
    }
}

impl std::error::Error for TabError {}

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

/// Host command to a renderer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Command {
    /// Replace one frame's document: decode, reset the realm, parse, run to load.
    Mount {
        /// Frame to replace.
        frame: FrameId,
        /// The document to mount.
        mount: Mount,
    },
    /// Evaluate `source` in one frame and return its string coercion.
    Eval {
        /// Frame to evaluate in.
        frame: FrameId,
        /// Script source.
        source: String,
    },
    /// Evaluate `source` in one frame and return a value-only result.
    ExecuteScript {
        /// Frame to evaluate in.
        frame: FrameId,
        /// Script source.
        source: String,
        /// Optional execution budget in milliseconds.
        timeout_ms: Option<u64>,
    },
    /// Set one frame's document URL used as cookie initiator and relative-URL base.
    SetDocumentUrl {
        /// Frame to update.
        frame: FrameId,
        /// Absolute URL.
        url: String,
    },
    /// True when the renderer has no jobs, timers, dials, or pending JS work.
    IsIdle,
    /// Stop the renderer loop.
    Shutdown,
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
    pub body: Vec<u8>,
}

/// Renderer reply to one [`Command`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Reply {
    /// Unit result.
    Unit(Result<(), TabError>),
    /// String result.
    Text(Result<String, TabError>),
    /// Value-only script result.
    Value(Result<RemoteValue, TabError>),
    /// Boolean result.
    Bool(bool),
}

/// Host to renderer traffic.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ToRenderer {
    /// One command with its correlation id.
    Request {
        /// Request id chosen by the browser process.
        id: u64,
        /// The command.
        command: Command,
    },
    /// Answer to a [`ServiceCall`].
    ServiceReply {
        /// Service-call id chosen by the renderer.
        id: u64,
        /// The answer.
        reply: ServiceReply,
    },
}

/// Renderer to host traffic.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum FromRenderer {
    /// First message from a `--renderer` child: protocol handshake.
    Ready,
    /// Answer to a request.
    Reply {
        /// Request id from [`ToRenderer::Request`].
        id: u64,
        /// The answer.
        reply: Reply,
    },
    /// Unsolicited document event.
    Event {
        /// Frame that emitted the event.
        frame: FrameId,
        /// The event.
        event: TabEvent,
    },
    /// A browser service the renderer cannot perform itself.
    ServiceCall {
        /// Service-call id chosen by the renderer.
        id: u64,
        /// The call.
        call: ServiceCall,
    },
}

/// What the renderer needs the browser process to do.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ServiceCall {
    /// HTTP GET submitted to the browser-owned network executor.
    Dial(DialRequest),
    /// `document.cookie` getter.
    CookieGet {
        /// Document URL.
        url: String,
    },
    /// `document.cookie` setter.
    CookieSet {
        /// Cookie string.
        value: String,
        /// Document URL.
        url: String,
    },
}

/// Answer to a [`ServiceCall`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ServiceReply {
    /// Dial result; `None` is a transport/limit failure.
    Dial(Option<DialOutcome>),
    /// Cookie getter result.
    Cookie(String),
    /// No payload (`CookieSet`).
    Unit,
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
pub type DialCompletion = Arc<dyn Fn(Option<DialOutcome>) + Send + Sync + 'static>;

/// Host services the renderer reaches through the browser-process seam.
///
/// In-process this is the `browser` crate's network adapter. In the
/// `--renderer` child it is a pipe proxy. Renderer code never names `net`.
pub trait BrowserServices: Send + Sync + 'static {
    /// Submits one GET without blocking the renderer thread. The completion
    /// receives `None` for transport, timeout, queue, or body-limit failure.
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
    use super::*;

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

        for number in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let message = FromRenderer::Reply {
                id: 3,
                reply: Reply::Value(Ok(RemoteValue::Number(number))),
            };
            round_trip(&message);
            let text = serde_json::to_string(&message).expect("serialize");
            let back: FromRenderer = serde_json::from_str(&text).expect("deserialize");
            let FromRenderer::Reply {
                reply: Reply::Value(Ok(RemoteValue::Number(value))),
                ..
            } = back
            else {
                panic!("non-finite number did not round trip");
            };
            if number.is_nan() {
                assert!(value.is_nan());
            } else {
                assert_eq!(value.to_bits(), number.to_bits());
            }
        }
    }

    #[test]
    fn ipc_codec_rejects_messages_over_the_wire_budget() {
        let message = ToRenderer::Request {
            id: 1,
            command: Command::Eval {
                frame: FrameId::MAIN,
                source: "x".repeat(MAX_IPC_MESSAGE_BYTES),
            },
        };
        let error = encode_ipc_message(&message).expect_err("oversized message");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);

        let mut bytes = vec![b'0'; MAX_IPC_MESSAGE_BYTES + 1];
        bytes.push(b'\n');
        let mut reader = io::Cursor::new(bytes);
        let mut buffer = Vec::new();
        let error = read_ipc_message::<serde_json::Value>(&mut reader, &mut buffer)
            .expect_err("oversized frame");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn host_to_renderer_messages_round_trip() {
        let messages = [
            ToRenderer::Request {
                id: 1,
                command: Command::Mount {
                    frame: FrameId::MAIN,
                    mount: Mount {
                        url: "about:blank".into(),
                        content_type: None,
                        content_language: None,
                        body: vec![1, 2, 3],
                    },
                },
            },
            ToRenderer::Request {
                id: 2,
                command: Command::Eval {
                    frame: FrameId::new(3),
                    source: "1+1".into(),
                },
            },
            ToRenderer::Request {
                id: 3,
                command: Command::ExecuteScript {
                    frame: FrameId::MAIN,
                    source: "x".into(),
                    timeout_ms: Some(50),
                },
            },
            ToRenderer::Request {
                id: 4,
                command: Command::SetDocumentUrl {
                    frame: FrameId::MAIN,
                    url: "http://example.test/".into(),
                },
            },
            ToRenderer::Request {
                id: 5,
                command: Command::IsIdle,
            },
            ToRenderer::Request {
                id: 6,
                command: Command::Shutdown,
            },
            ToRenderer::ServiceReply {
                id: 7,
                reply: ServiceReply::Cookie("a=1".into()),
            },
            ToRenderer::ServiceReply {
                id: 8,
                reply: ServiceReply::Dial(Some(DialOutcome {
                    status: 200,
                    final_url: "http://example.test/".into(),
                    content_type: Some("text/html".into()),
                    content_language: None,
                    body: vec![1],
                })),
            },
            ToRenderer::ServiceReply {
                id: 9,
                reply: ServiceReply::Unit,
            },
        ];
        for message in messages {
            round_trip(&message);
        }
    }

    #[test]
    fn renderer_to_host_messages_round_trip() {
        let messages = [
            FromRenderer::Ready,
            FromRenderer::Reply {
                id: 1,
                reply: Reply::Unit(Ok(())),
            },
            FromRenderer::Reply {
                id: 2,
                reply: Reply::Unit(Err(TabError::Script(ScriptFailure::Interrupted))),
            },
            FromRenderer::Reply {
                id: 3,
                reply: Reply::Text(Ok("ok".into())),
            },
            FromRenderer::Reply {
                id: 4,
                reply: Reply::Bool(false),
            },
            FromRenderer::Reply {
                id: 5,
                reply: Reply::Value(Ok(RemoteValue::List(vec![RemoteValue::Number(1.0)]))),
            },
            FromRenderer::Event {
                frame: FrameId::MAIN,
                event: TabEvent::Fetch { status: 404 },
            },
            FromRenderer::ServiceCall {
                id: 6,
                call: ServiceCall::Dial(DialRequest {
                    kind: DialKind::JsFetch,
                    url: "http://example.test/a".into(),
                    initiator: "http://example.test/".into(),
                    read_body: true,
                }),
            },
            FromRenderer::ServiceCall {
                id: 7,
                call: ServiceCall::CookieGet {
                    url: "http://example.test/".into(),
                },
            },
            FromRenderer::ServiceCall {
                id: 8,
                call: ServiceCall::CookieSet {
                    value: "a=1".into(),
                    url: "http://example.test/".into(),
                },
            },
        ];
        for message in messages {
            round_trip(&message);
        }
    }
}
