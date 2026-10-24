//! The renderer wire: what the browser process and a `renderer` child send
//! each other, and the frame codec that carries it.
//!
//! Only value-only messages cross; DOM handles, `QuickJS` values, callbacks,
//! and `net` types never do.
//!
//! Both ends of this conversation live in this crate — the host end in
//! [`crate::link`], the child end in [`crate::child`] — so the message
//! vocabulary and the framing sit next to both. Payloads that describe a page
//! ([`FrameId`], [`Mount`](renderer::Mount), [`RendererEvent`], [`DialRequest`])
//! come from the engine crate and stay there; this module only carries them.

pub mod channel;

use serde::{Deserialize, Serialize};

use crate::exchange::{Frame, RequestId};
use renderer::{
    DialFailure, DialOutcome, DialRequest, FrameId, RemoteValue, RendererEvent, StorageChange,
    StorageError, StorageKind, TabError,
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

    /// The raw browser-process integer, used for release high-water marks.
    #[must_use]
    pub(crate) const fn get(self) -> u64 {
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
        source: renderer::ScriptSource,
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
        /// The sender's serialized origin at post time.
        origin: String,
        /// The normalized target origin the sender asked for.
        target_origin: String,
        /// `__tbEncode` payload from the posting window.
        payload: String,
    },
    /// Activate an existing entry in the current document without replacing it.
    HistoryTraverse {
        url: String,
        history: renderer::HistorySnapshot,
    },
    /// Registers a page-scoped init script, identified by `id` so a replaced
    /// renderer can replay it.
    AddInitScript {
        /// Caller-assigned identifier.
        id: u64,
        /// Script source.
        source: String,
        /// Whether to evaluate the script in existing realms immediately
        /// (<https://chromedevtools.github.io/devtools-protocol/tot/Page/#method-addScriptToEvaluateOnNewDocument>).
        run_immediately: bool,
    },
    /// Removes a previously registered init script.
    RemoveInitScript {
        /// Identifier returned by the matching `AddInitScript`.
        id: u64,
    },
    /// Sets a frame's persistent viewport size.
    SetViewport {
        /// Frame to resize.
        frame: FrameId,
        /// Viewport width in device pixels.
        width: u32,
        /// Viewport height in device pixels.
        height: u32,
    },
    /// Reads a frame's scrollable content size.
    ContentSize {
        /// Frame to measure.
        frame: FrameId,
    },
}

/// Renderer reply to one [`Command`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Reply {
    /// Unit result.
    Unit(Result<(), TabError>),
    /// Value-only script result.
    Value(Result<RemoteValue, TabError>),
    /// A PNG follows in body frames for this request id; `len` is its exact
    /// byte length. The control payload never carries the bytes.
    Screenshot {
        /// PNG byte length, or the failure that replaced it.
        result: Result<u32, TabError>,
    },
    /// A frame's scrollable content size in CSS pixels.
    ContentSize(Result<(f32, f32), TabError>),
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

/// Metadata sent before the raw bytes of a top-level response: the document
/// that receives it plus the response head it installs. The body arrives in
/// bounded raw frames after the postcard control message.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResponseStart {
    /// Top-level document receiving this response.
    pub assignment: RendererAssignmentId,
    /// Frame that will receive the document.
    pub frame: FrameId,
    /// The response head the document installs.
    pub head: renderer::ResponseHead,
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
    /// Network and cookie operation.
    Network(NetworkCall),
    /// Web Storage operation.
    Storage(StorageCall),
    /// Browsing-context operation.
    BrowsingContext(BrowsingContextCall),
    /// Cross-context messaging operation.
    Messaging(MessagingCall),
}

/// Network and cookie operation requested by a renderer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum NetworkCall {
    /// HTTP request submitted to the browser-owned network executor.
    Dial(Box<DialRequest>),
    /// Cancels a dial submitted by this assignment.
    CancelDial { id: RequestId },
    /// Reads the next decoded body chunk for a streamed dial.
    DialRead { id: u64 },
    /// Cancels a streamed response body.
    CancelStream { id: u64 },
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

/// Web Storage operation requested by a renderer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum StorageCall {
    /// `localStorage.getItem(key)`.
    Get {
        /// Storage area kind.
        kind: StorageKind,
        /// Serialized origin of the calling document.
        origin: String,
        /// Item key.
        key: String,
    },
    /// Every key of one origin's local storage area.
    Keys {
        /// Storage area kind.
        kind: StorageKind,
        /// Serialized origin of the calling document.
        origin: String,
    },
    /// `localStorage.setItem(key, value)`.
    Set {
        /// Storage area kind.
        kind: StorageKind,
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
    Remove {
        /// Storage area kind.
        kind: StorageKind,
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
    Clear {
        /// Storage area kind.
        kind: StorageKind,
        /// Serialized origin of the calling document.
        origin: String,
        /// Calling document URL.
        url: String,
        /// Frame whose script made the change.
        source: FrameId,
    },
}

/// Browsing-context operation requested by a renderer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum BrowsingContextCall {
    /// `window.open(url, target, features)`.
    WindowOpen {
        /// Absolute URL to load, or empty for `about:blank`.
        url: String,
        /// Browsing context name.
        name: String,
        /// Feature string from the caller.
        features: String,
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
        /// The sender's serialized origin at post time.
        origin: String,
        /// The normalized target origin the sender asked for.
        target_origin: String,
        /// `__tbEncode` payload from the sender's realm.
        payload: String,
    },
    /// Reads one key from another tab's session area.
    RemoteSessionGet {
        /// Target tab identity.
        tab: u64,
        /// Serialized origin of the calling document.
        origin: String,
        /// Encoded item key.
        key: String,
    },
}

/// Cross-context messaging operation requested by a renderer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum MessagingCall {
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
    /// Next streamed body chunk, or `None` at the end.
    StreamChunk(Result<Option<Vec<u8>>, DialFailure>),
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

impl ServiceReply {
    /// The benign answer to `call`: the transport must always answer a
    /// service call so the renderer's synchronous path returns, even when the
    /// call was released, cancels itself, or is fire-and-forget and no one
    /// reads the reply. Each answer is that call type's empty value.
    #[must_use]
    pub fn benign(call: &ServiceCall) -> Self {
        match call {
            ServiceCall::Network(NetworkCall::Dial(_)) => {
                ServiceReply::Dial(Err(DialFailure::Connect))
            }
            ServiceCall::Network(NetworkCall::DialRead { .. }) => {
                ServiceReply::StreamChunk(Err(DialFailure::Connect))
            }
            ServiceCall::Network(NetworkCall::CookieGet { .. }) => {
                ServiceReply::Cookie(String::new())
            }
            ServiceCall::Network(
                NetworkCall::CancelDial { .. }
                | NetworkCall::CancelStream { .. }
                | NetworkCall::CookieSet { .. },
            )
            | ServiceCall::Messaging(MessagingCall::BroadcastPost { .. })
            | ServiceCall::BrowsingContext(
                BrowsingContextCall::WindowClose { .. } | BrowsingContextCall::WindowMessage { .. },
            ) => ServiceReply::Unit,
            ServiceCall::Storage(StorageCall::Get { .. })
            | ServiceCall::BrowsingContext(BrowsingContextCall::RemoteSessionGet { .. }) => {
                ServiceReply::StorageValue(None)
            }
            ServiceCall::Storage(StorageCall::Keys { .. }) => ServiceReply::StorageKeys(Vec::new()),
            ServiceCall::Storage(
                StorageCall::Set { .. } | StorageCall::Remove { .. } | StorageCall::Clear { .. },
            ) => ServiceReply::StorageChanged(Ok(None)),
            ServiceCall::BrowsingContext(
                BrowsingContextCall::WindowOpen { .. } | BrowsingContextCall::Opener,
            ) => ServiceReply::Window(None),
        }
    }
}
