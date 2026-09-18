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
    DialFailure, DialOutcome, DialRequest, FrameId, Mount, RemoteValue, StorageChange,
    StorageError, StorageKind, StorageSeed, TabError, TabEvent,
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
    /// Render one frame to a PNG and stream it back in body frames.
    Screenshot {
        /// Frame to render.
        frame: FrameId,
        /// Viewport and crop window.
        request: renderer::ScreenshotRequest,
    },
    /// Stop the renderer loop.
    Shutdown,
    /// Delivers one remote `message` event, encoded by the sender's realm.
    WindowMessage {
        /// `__tbEncode` payload from the posting window.
        payload: String,
    },
    /// Copies one `sessionStorage` seed into the engine.
    SeedSession {
        /// The opener's session area for one origin.
        seed: StorageSeed,
    },
    /// Reads one key of this engine's session area for `origin`.
    RemoteSessionGet {
        /// Serialized origin of the calling document.
        origin: String,
        /// Item key, encoded by the calling realm.
        key: String,
    },
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
    /// Optional string result (`RemoteSessionGet`).
    Optional(Option<String>),
    /// A PNG follows in body frames for this request id; `len` is its exact
    /// byte length. The JSON control plane never carries the bytes.
    Screenshot {
        /// PNG byte length, or the failure that replaced it.
        result: Result<u32, TabError>,
    },
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
    /// One `localStorage` change another renderer made; every frame of
    /// `origin` except the source window fires a `storage` event
    /// (<https://html.spec.whatwg.org/multipage/webstorage.html#concept-storage-broadcast>).
    StorageEvent {
        /// Serialized origin whose area changed.
        origin: String,
        /// Which area changed.
        kind: StorageKind,
        /// The changed key, or `None` for `clear()`.
        key: Option<String>,
        /// The value before the change.
        old_value: Option<String>,
        /// The value after the change.
        new_value: Option<String>,
        /// URL of the document whose script made the change.
        url: String,
        /// The assignment and frame whose script made the change. Only the
        /// matching assignment excludes the frame; other renderers and
        /// assignments see a change with no local source.
        source: Option<(RendererAssignmentId, FrameId)>,
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
    /// `localStorage.getItem(key)`.
    StorageGet {
        /// Serialized origin of the calling document.
        origin: String,
        /// Item key.
        key: String,
    },
    /// Every key of one origin's local storage area.
    StorageKeys {
        /// Serialized origin of the calling document.
        origin: String,
    },
    /// `localStorage.setItem(key, value)`.
    StorageSet {
        /// Serialized origin of the calling document.
        origin: String,
        /// Calling document URL; the `storage` event reports it.
        url: String,
        /// Item key.
        key: String,
        /// Item value.
        value: String,
        /// Frame whose script made the change.
        source: FrameId,
    },
    /// `localStorage.removeItem(key)`.
    StorageRemove {
        /// Serialized origin of the calling document.
        origin: String,
        /// Calling document URL.
        url: String,
        /// Item key.
        key: String,
        /// Frame whose script made the change.
        source: FrameId,
    },
    /// `localStorage.clear()`.
    StorageClear {
        /// Serialized origin of the calling document.
        origin: String,
        /// Calling document URL.
        url: String,
        /// Frame whose script made the change.
        source: FrameId,
    },
    /// `window.open(url, target, features)`.
    WindowOpen {
        /// Absolute URL to load, or empty for `about:blank`.
        url: String,
        /// Browsing context name.
        name: String,
        /// Feature string from the caller.
        features: String,
        /// Session copy for the new tab, when the opener sent one.
        seed: Option<StorageSeed>,
    },
    /// `window.close()` on a window this renderer opened.
    WindowClose {
        /// Browser-minted tab identity.
        tab: u64,
    },
    /// `window.opener` for this assignment's tab.
    Opener,
    /// `postMessage` to a window this renderer opened.
    WindowMessage {
        /// Target tab identity.
        tab: u64,
        /// `__tbEncode` payload from the sender's realm.
        payload: String,
    },
    /// Reads one key of another tab's session area for `origin`.
    RemoteSessionGet {
        /// Target tab identity.
        tab: u64,
        /// Serialized origin of the calling document.
        origin: String,
        /// Item key, encoded by the calling realm.
        key: String,
    },
}

/// Answer to a [`ServiceCall`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ServiceReply {
    /// Dial result, with a typed failure.
    Dial(Result<DialOutcome, DialFailure>),
    /// Cookie getter result.
    Cookie(String),
    /// Local storage getter result.
    StorageValue(Option<String>),
    /// Local storage key list, in iteration order.
    StorageKeys(Vec<String>),
    /// Local storage mutation result; `Ok(None)` means nothing changed.
    StorageChanged(Result<Option<StorageChange>, StorageError>),
    /// `window.open` result: the new tab, or `None` when it was refused.
    Window(Option<u64>),
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
                id: 4,
                assignment: RendererAssignmentId::new(1),
                command: Command::Screenshot {
                    frame: FrameId::MAIN,
                    request: renderer::ScreenshotRequest {
                        viewport_width: 800.0,
                        viewport_height: 600.0,
                        clip: None,
                    },
                },
            },
            ToRenderer::Request {
                id: 6,
                assignment: RendererAssignmentId::new(1),
                command: Command::Shutdown,
            },
            ToRenderer::Request {
                id: 7,
                assignment: RendererAssignmentId::new(1),
                command: Command::WindowMessage {
                    payload: "tb1:null".into(),
                },
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
        ];
        for message in messages {
            round_trip(&message);
        }
    }

    #[test]
    fn host_to_renderer_service_messages_round_trip() {
        let messages = [
            ToRenderer::ServiceReply {
                id: 7,
                reply: ServiceReply::Cookie("a=1".into()),
            },
            ToRenderer::ServiceReply {
                id: 12,
                reply: ServiceReply::StorageValue(Some("v".into())),
            },
            ToRenderer::ServiceReply {
                id: 13,
                reply: ServiceReply::StorageKeys(vec!["k".into()]),
            },
            ToRenderer::ServiceReply {
                id: 14,
                reply: ServiceReply::StorageChanged(Ok(Some(StorageChange {
                    key: Some("k".into()),
                    old_value: None,
                    new_value: Some("v".into()),
                }))),
            },
            ToRenderer::ServiceReply {
                id: 15,
                reply: ServiceReply::StorageChanged(Err(StorageError::QuotaExceeded)),
            },
            ToRenderer::ServiceReply {
                id: 17,
                reply: ServiceReply::Window(Some(3)),
            },
            ToRenderer::StorageEvent {
                origin: "http://example.test".into(),
                kind: StorageKind::Local,
                key: Some("k".into()),
                old_value: None,
                new_value: Some("v".into()),
                url: "http://example.test/".into(),
                source: Some((RendererAssignmentId::new(1), FrameId::MAIN)),
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
    fn host_to_renderer_session_commands_round_trip() {
        let messages = [
            ToRenderer::Request {
                id: 20,
                assignment: RendererAssignmentId::new(1),
                command: Command::WindowMessage {
                    payload: "tb1:null".into(),
                },
            },
            ToRenderer::Request {
                id: 21,
                assignment: RendererAssignmentId::new(1),
                command: Command::SeedSession {
                    seed: StorageSeed {
                        origin: "http://example.test".into(),
                        entries: vec![("\"k\"".into(), "\"v\"".into())],
                    },
                },
            },
            ToRenderer::Request {
                id: 22,
                assignment: RendererAssignmentId::new(1),
                command: Command::RemoteSessionGet {
                    origin: "http://example.test".into(),
                    key: "\"k\"".into(),
                },
            },
            ToRenderer::ServiceReply {
                id: 23,
                reply: ServiceReply::StorageValue(Some("\"v\"".into())),
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
            FromRenderer::Reply {
                id: 9,
                assignment: RendererAssignmentId::new(1),
                reply: Reply::Screenshot { result: Ok(3) },
            },
            FromRenderer::Event {
                assignment: RendererAssignmentId::new(1),
                frame: FrameId::MAIN,
                event: TabEvent::Fetch { status: 404 },
            },
        ];
        for message in messages {
            round_trip(&message);
        }
    }

    #[test]
    fn renderer_to_host_service_calls_round_trip() {
        let messages = [
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

    #[test]
    fn renderer_to_host_storage_calls_round_trip() {
        let messages = [
            FromRenderer::ServiceCall {
                assignment: RendererAssignmentId::new(1),
                id: 10,
                call: ServiceCall::StorageGet {
                    origin: "http://example.test".into(),
                    key: "k".into(),
                },
            },
            FromRenderer::ServiceCall {
                assignment: RendererAssignmentId::new(1),
                id: 11,
                call: ServiceCall::StorageKeys {
                    origin: "http://example.test".into(),
                },
            },
            FromRenderer::ServiceCall {
                assignment: RendererAssignmentId::new(1),
                id: 12,
                call: ServiceCall::StorageSet {
                    origin: "http://example.test".into(),
                    url: "http://example.test/".into(),
                    key: "k".into(),
                    value: "v".into(),
                    source: FrameId::MAIN,
                },
            },
            FromRenderer::ServiceCall {
                assignment: RendererAssignmentId::new(1),
                id: 13,
                call: ServiceCall::StorageRemove {
                    origin: "http://example.test".into(),
                    url: "http://example.test/".into(),
                    key: "k".into(),
                    source: FrameId::MAIN,
                },
            },
            FromRenderer::ServiceCall {
                assignment: RendererAssignmentId::new(1),
                id: 14,
                call: ServiceCall::StorageClear {
                    origin: "http://example.test".into(),
                    url: "http://example.test/".into(),
                    source: FrameId::MAIN,
                },
            },
        ];
        for message in messages {
            round_trip(&message);
        }
    }

    #[test]
    fn renderer_to_host_window_calls_round_trip() {
        let messages = [
            FromRenderer::ServiceCall {
                assignment: RendererAssignmentId::new(1),
                id: 15,
                call: ServiceCall::WindowOpen {
                    url: "http://example.test/".into(),
                    name: "popup".into(),
                    features: "noopener".into(),
                    seed: Some(StorageSeed {
                        origin: "http://example.test".into(),
                        entries: vec![("\"k\"".into(), "\"v\"".into())],
                    }),
                },
            },
            FromRenderer::ServiceCall {
                assignment: RendererAssignmentId::new(1),
                id: 16,
                call: ServiceCall::WindowClose { tab: 3 },
            },
            FromRenderer::ServiceCall {
                assignment: RendererAssignmentId::new(1),
                id: 17,
                call: ServiceCall::Opener,
            },
            FromRenderer::ServiceCall {
                assignment: RendererAssignmentId::new(1),
                id: 18,
                call: ServiceCall::WindowMessage {
                    tab: 3,
                    payload: "tb1:null".into(),
                },
            },
            FromRenderer::ServiceCall {
                assignment: RendererAssignmentId::new(1),
                id: 19,
                call: ServiceCall::RemoteSessionGet {
                    tab: 3,
                    origin: "http://example.test".into(),
                    key: "\"k\"".into(),
                },
            },
        ];
        for message in messages {
            round_trip(&message);
        }
    }
}
