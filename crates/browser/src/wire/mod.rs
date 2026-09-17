//! The renderer wire: what the browser process and a `renderer` child send
//! each other, and the frame codec that carries it.
//!
//! Only value-only messages cross; DOM handles, `QuickJS` values, callbacks,
//! and `net` types never do.
//!
//! Both ends of this conversation live in this crate — the host end in
//! [`crate::link`], the child end in [`crate::child`] — so the message
//! vocabulary and the framing sit next to both. Payloads that describe a page
//! ([`FrameId`], [`Mount`], [`TabEvent`], [`DialRequest`]) come from the
//! engine crate and stay there; this module only carries them.

pub mod channel;

use serde::{Deserialize, Serialize};

use renderer::{
    DialFailure, DialOutcome, DialRequest, FrameId, Mount, RemoteValue, TabError, TabEvent,
};

/// Browser-minted identity of one top-level document hosted by a renderer.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RendererAssignmentId(u64);

impl RendererAssignmentId {
    /// Constructs an assignment id from a browser-process integer.
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
    /// Stop the renderer loop.
    Shutdown,
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
}

/// Host to renderer traffic.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ToRenderer {
    /// First frame from the browser process: protocol handshake.
    Hello,
    /// Creates an isolated page engine inside this renderer process.
    Assign {
        /// Browser-minted assignment identity.
        assignment: RendererAssignmentId,
    },
    /// Removes one page engine from this renderer process.
    Release {
        /// Browser-minted assignment identity.
        assignment: RendererAssignmentId,
    },
    /// One command with its correlation id.
    Request {
        /// Request id chosen by the browser process.
        id: u64,
        /// Top-level document that owns the command.
        assignment: RendererAssignmentId,
        /// The command.
        command: Command,
    },
    /// Starts a streamed top-level response. Raw body frames with the same id
    /// follow before [`ToRenderer::ResponseEnd`].
    ResponseStart {
        /// Request id chosen by the browser process.
        id: u64,
        /// Response metadata needed to mount the completed body.
        response: ResponseStart,
    },
    /// Completes a streamed top-level response.
    ResponseEnd {
        /// Request id from [`ToRenderer::ResponseStart`].
        id: u64,
    },
    /// Aborts a streamed top-level response.
    ResponseError {
        /// Request id from [`ToRenderer::ResponseStart`].
        id: u64,
        /// Typed transport failure.
        failure: DialFailure,
    },
    /// Answer to a [`ServiceCall`].
    ServiceReply {
        /// Service-call id chosen by the renderer.
        id: u64,
        /// The answer.
        reply: ServiceReply,
    },
}

impl ToRenderer {
    /// The sentinel request that stops the renderer loop.
    pub(crate) fn shutdown_request() -> Self {
        Self::Request {
            id: 0,
            assignment: RendererAssignmentId::new(0),
            command: Command::Shutdown,
        }
    }
}

/// Metadata sent before the raw bytes of a top-level response.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResponseStart {
    /// Top-level document receiving this response.
    pub assignment: RendererAssignmentId,
    /// Frame that will receive the document.
    pub frame: FrameId,
    /// Final HTTP status.
    pub status: u16,
    /// Final URL after redirects.
    pub final_url: String,
    /// HTTP `Content-Type`, when present.
    pub content_type: Option<String>,
    /// HTTP `Content-Language`, when present.
    pub content_language: Option<String>,
}

/// Renderer to host traffic.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum FromRenderer {
    /// First message from a `renderer` child: protocol handshake.
    Ready,
    /// Answer to a request.
    Reply {
        /// Request id from [`ToRenderer::Request`].
        id: u64,
        /// Top-level document that produced the reply.
        assignment: RendererAssignmentId,
        /// The answer.
        reply: Reply,
    },
    /// Unsolicited document event.
    Event {
        /// Top-level document that emitted the event.
        assignment: RendererAssignmentId,
        /// Frame that emitted the event.
        frame: FrameId,
        /// The event.
        event: TabEvent,
    },
    /// A browser service the renderer cannot perform itself.
    ServiceCall {
        /// Top-level document requesting the browser service.
        assignment: RendererAssignmentId,
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
    /// Dial result, with a typed failure.
    Dial(Result<DialOutcome, DialFailure>),
    /// Cookie getter result.
    Cookie(String),
    /// No payload (`CookieSet`).
    Unit,
}

#[cfg(test)]
mod tests {
    use super::*;
    use renderer::{DialKind, ScriptFailure};

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
    fn host_to_renderer_messages_round_trip() {
        let messages = [
            ToRenderer::Assign {
                assignment: RendererAssignmentId::new(1),
            },
            ToRenderer::Release {
                assignment: RendererAssignmentId::new(1),
            },
            ToRenderer::Request {
                id: 1,
                assignment: RendererAssignmentId::new(1),
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
                assignment: RendererAssignmentId::new(1),
                command: Command::Eval {
                    frame: FrameId::new(3),
                    source: "1+1".into(),
                },
            },
            ToRenderer::Request {
                id: 3,
                assignment: RendererAssignmentId::new(1),
                command: Command::ExecuteScript {
                    frame: FrameId::MAIN,
                    source: "x".into(),
                    timeout_ms: Some(50),
                },
            },
            ToRenderer::Request {
                id: 6,
                assignment: RendererAssignmentId::new(1),
                command: Command::Shutdown,
            },
            ToRenderer::ResponseStart {
                id: 10,
                response: ResponseStart {
                    assignment: RendererAssignmentId::new(1),
                    frame: FrameId::MAIN,
                    status: 200,
                    final_url: "http://example.test/".into(),
                    content_type: Some("text/html".into()),
                    content_language: None,
                },
            },
            ToRenderer::ResponseEnd { id: 10 },
            ToRenderer::ResponseError {
                id: 11,
                failure: DialFailure::Timeout,
            },
            ToRenderer::ServiceReply {
                id: 7,
                reply: ServiceReply::Cookie("a=1".into()),
            },
            ToRenderer::ServiceReply {
                id: 8,
                reply: ServiceReply::Dial(Ok(DialOutcome {
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
    fn mount_body_is_not_part_of_control_json() {
        let message = ToRenderer::Request {
            id: 1,
            assignment: RendererAssignmentId::new(1),
            command: Command::Mount {
                frame: FrameId::MAIN,
                mount: Mount {
                    url: "http://example.test/".into(),
                    content_type: Some("text/html".into()),
                    content_language: None,
                    body: vec![1, 2, 3],
                },
            },
        };
        let value = serde_json::to_value(message).expect("serialize");
        assert!(value.pointer("/Request/command/Mount/mount/body").is_none());
    }

    #[test]
    fn renderer_to_host_messages_round_trip() {
        let messages = [
            FromRenderer::Ready,
            FromRenderer::Reply {
                id: 1,
                assignment: RendererAssignmentId::new(1),
                reply: Reply::Unit(Ok(())),
            },
            FromRenderer::Reply {
                id: 2,
                assignment: RendererAssignmentId::new(1),
                reply: Reply::Unit(Err(TabError::Script(ScriptFailure::Interrupted))),
            },
            FromRenderer::Reply {
                id: 3,
                assignment: RendererAssignmentId::new(1),
                reply: Reply::Text(Ok("ok".into())),
            },
            FromRenderer::Reply {
                id: 5,
                assignment: RendererAssignmentId::new(1),
                reply: Reply::Value(Ok(RemoteValue::List(vec![RemoteValue::Number(1.0)]))),
            },
            FromRenderer::Event {
                assignment: RendererAssignmentId::new(1),
                frame: FrameId::MAIN,
                event: TabEvent::Fetch { status: 404 },
            },
            FromRenderer::ServiceCall {
                assignment: RendererAssignmentId::new(1),
                id: 6,
                call: ServiceCall::Dial(DialRequest {
                    kind: DialKind::JsFetch,
                    url: "http://example.test/a".into(),
                    initiator: "http://example.test/".into(),
                    read_body: true,
                }),
            },
            FromRenderer::ServiceCall {
                assignment: RendererAssignmentId::new(1),
                id: 7,
                call: ServiceCall::CookieGet {
                    url: "http://example.test/".into(),
                },
            },
            FromRenderer::ServiceCall {
                assignment: RendererAssignmentId::new(1),
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
