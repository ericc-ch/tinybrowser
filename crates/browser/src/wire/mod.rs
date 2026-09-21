//! The renderer wire: what the browser process and a `renderer` child send
//! each other, and the frame codec that carries it.
//!
//! Only value-only messages cross; DOM handles, `QuickJS` values, callbacks,
//! and `net` types never do.
//!
//! Both ends of this conversation live in this crate — the host end in
//! [`crate::link`], the child end in [`crate::child`] — so the message
//! vocabulary and the framing sit next to both. Payloads that describe a page
//! ([`FrameId`], [`Mount`], [`RendererEvent`], [`DialRequest`]) come from the
//! engine crate and stay there; this module only carries them.

pub mod channel;

use serde::{Deserialize, Serialize};

use crate::exchange::Frame;
use renderer::{
    DialFailure, DialOutcome, DialRequest, FrameId, RemoteValue, RendererEvent, StorageChange,
    StorageError, StorageKind, StorageSeed, TabError,
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

/// One browser-originated renderer call.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RendererCall {
    /// One operation on an assigned page engine.
    Command {
        /// Top-level document that owns the command.
        assignment: RendererAssignmentId,
        /// The command.
        command: Command,
    },
    /// Starts a streamed top-level response. Request chunks follow before the
    /// exchange's request-stream terminator.
    Response {
        /// Response metadata needed to mount the completed body.
        response: ResponseStart,
    },
}

/// Browser-originated notifications that do not have replies.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum HostNotice {
    /// First frame from the browser process: protocol handshake.
    Hello,
    /// Stops the renderer process.
    Shutdown,
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
    /// One same-origin `BroadcastChannel` message.
    BroadcastMessage {
        /// Serialized origin whose channels receive it.
        origin: String,
        /// Channel name.
        name: String,
        /// `__tbEncode` payload from the posting realm.
        payload: String,
        /// The posting assignment and channel; the matching assignment skips
        /// that channel.
        source: Option<(RendererAssignmentId, u64)>,
    },
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

/// One renderer-originated browser-service call.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BrowserCall {
    /// Top-level document requesting the browser service.
    pub assignment: RendererAssignmentId,
    /// The call.
    pub call: ServiceCall,
}

/// Answer from a renderer, including the assignment used for authorization.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RendererReply {
    /// Top-level document that produced the reply.
    pub assignment: RendererAssignmentId,
    /// The answer.
    pub reply: Reply,
}

/// Renderer-originated notifications that do not have replies.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RendererNotice {
    /// First message from a `renderer` child: protocol handshake.
    Ready,
    /// Unsolicited document event.
    Event {
        /// Top-level document that produced the reply.
        assignment: RendererAssignmentId,
        /// Frame that emitted the event.
        frame: FrameId,
        /// The event.
        event: RendererEvent,
    },
}

/// Decoded browser-to-renderer exchange messages.
pub(crate) type ToRenderer = Frame<RendererCall, ServiceReply, HostNotice>;

/// Decoded renderer-to-browser exchange messages.
pub(crate) type FromRenderer = Frame<BrowserCall, RendererReply, RendererNotice>;

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
    /// One `BroadcastChannel.postMessage` for every same-origin channel.
    BroadcastPost {
        /// Serialized origin of the posting document.
        origin: String,
        /// Channel name.
        name: String,
        /// `__tbEncode` payload from the posting realm.
        payload: String,
        /// Posting realm's channel id, so the sender is skipped.
        channel: u64,
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
    use crate::exchange::RequestId;
    use renderer::DialKind;

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
    fn host_to_renderer_control_messages_round_trip() {
        let assignment = RendererAssignmentId::new(1);
        let messages: Vec<ToRenderer> = vec![
            Frame::Notify(HostNotice::Hello),
            Frame::Notify(HostNotice::Assign { assignment }),
            Frame::Call {
                id: RequestId::new(1),
                body: RendererCall::Command {
                    assignment,
                    command: Command::ExecuteScript {
                        frame: FrameId::MAIN,
                        source: "1 + 1".into(),
                        timeout_ms: Some(50),
                    },
                },
            },
            Frame::Call {
                id: RequestId::new(2),
                body: RendererCall::Response {
                    response: ResponseStart {
                        assignment,
                        frame: FrameId::MAIN,
                        status: 200,
                        final_url: "http://example.test/".into(),
                        content_type: Some("text/html".into()),
                        content_language: None,
                    },
                },
            },
            Frame::RequestEnd {
                id: RequestId::new(2),
                error: None,
            },
            Frame::Reply {
                id: RequestId::new(3),
                body: ServiceReply::StorageChanged(Ok(Some(StorageChange {
                    key: Some("k".into()),
                    old_value: None,
                    new_value: Some("v".into()),
                }))),
            },
            Frame::Notify(HostNotice::StorageEvent {
                origin: "http://example.test".into(),
                kind: StorageKind::Local,
                key: Some("k".into()),
                old_value: None,
                new_value: Some("v".into()),
                url: "http://example.test/".into(),
                source: Some((assignment, FrameId::MAIN)),
            }),
            Frame::Notify(HostNotice::BroadcastMessage {
                origin: "http://example.test".into(),
                name: "chan".into(),
                payload: "tb1:null".into(),
                source: Some((assignment, 4)),
            }),
            Frame::Notify(HostNotice::Release { assignment }),
            Frame::Notify(HostNotice::Shutdown),
        ];
        for message in messages {
            round_trip(&message);
        }
    }

    #[test]
    fn renderer_to_host_control_messages_round_trip() {
        let assignment = RendererAssignmentId::new(1);
        let messages: Vec<FromRenderer> = vec![
            Frame::Notify(RendererNotice::Ready),
            Frame::Reply {
                id: RequestId::new(1),
                body: RendererReply {
                    assignment,
                    reply: Reply::Value(Ok(RemoteValue::List(vec![RemoteValue::Number(1.0)]))),
                },
            },
            Frame::Notify(RendererNotice::Event {
                assignment,
                frame: FrameId::MAIN,
                event: RendererEvent::Fetch { status: 404 },
            }),
            Frame::Call {
                id: RequestId::new(2),
                body: BrowserCall {
                    assignment,
                    call: ServiceCall::Dial(DialRequest {
                        kind: DialKind::JsFetch,
                        url: "http://example.test/a".into(),
                        initiator: "http://example.test/".into(),
                        read_body: true,
                    }),
                },
            },
            Frame::Cancel {
                id: RequestId::new(2),
            },
        ];
        for message in messages {
            round_trip(&message);
        }
    }

    #[test]
    fn browser_service_calls_round_trip() {
        let assignment = RendererAssignmentId::new(1);
        let calls = vec![
            ServiceCall::CookieGet {
                url: "http://example.test/".into(),
            },
            ServiceCall::CookieSet {
                value: "a=1".into(),
                url: "http://example.test/".into(),
            },
            ServiceCall::StorageGet {
                origin: "http://example.test".into(),
                key: "k".into(),
            },
            ServiceCall::StorageKeys {
                origin: "http://example.test".into(),
            },
            ServiceCall::StorageSet {
                origin: "http://example.test".into(),
                url: "http://example.test/".into(),
                key: "k".into(),
                value: "v".into(),
                source: FrameId::MAIN,
            },
            ServiceCall::StorageRemove {
                origin: "http://example.test".into(),
                url: "http://example.test/".into(),
                key: "k".into(),
                source: FrameId::MAIN,
            },
            ServiceCall::StorageClear {
                origin: "http://example.test".into(),
                url: "http://example.test/".into(),
                source: FrameId::MAIN,
            },
            ServiceCall::WindowOpen {
                url: "http://example.test/".into(),
                name: "popup".into(),
                features: "noopener".into(),
                seed: Some(StorageSeed {
                    origin: "http://example.test".into(),
                    entries: vec![("k".into(), "v".into())],
                }),
            },
            ServiceCall::WindowClose { tab: 3 },
            ServiceCall::Opener,
            ServiceCall::WindowMessage {
                tab: 3,
                payload: "tb1:null".into(),
            },
            ServiceCall::RemoteSessionGet {
                tab: 3,
                origin: "http://example.test".into(),
                key: "k".into(),
            },
            ServiceCall::BroadcastPost {
                origin: "http://example.test".into(),
                name: "chan".into(),
                payload: "tb1:null".into(),
                channel: 4,
            },
        ];
        for (raw_id, call) in (1_u64..).zip(calls) {
            let message: FromRenderer = Frame::Call {
                id: RequestId::new(raw_id),
                body: BrowserCall { assignment, call },
            };
            round_trip(&message);
        }
    }

    #[test]
    fn response_metadata_has_no_body_bytes() {
        let message: ToRenderer = Frame::Call {
            id: RequestId::new(1),
            body: RendererCall::Response {
                response: ResponseStart {
                    assignment: RendererAssignmentId::new(1),
                    frame: FrameId::MAIN,
                    status: 200,
                    final_url: "http://example.test/".into(),
                    content_type: Some("text/html".into()),
                    content_language: None,
                },
            },
        };
        let value = serde_json::to_value(message).expect("serialize");
        let response = value
            .pointer("/Call/body/Response/response")
            .expect("response metadata");
        assert!(response.get("body").is_none());
    }
}
