//! Browser side of the renderer seam: factory, handles, and async routing.
//!
//! One private platform channel per renderer, length-prefixed frames, async
//! reader and writer tasks, and oneshot replies. The handle stays value-only.

use std::collections::{HashMap, HashSet};
use std::io;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::process::Child;

use crate::actor::TabId;
use crate::broadcast::{BroadcastMessage, ContextEvent, StorageBroadcast};
use crate::context::PartitionServices;
use crate::exchange::{self, Frame, RequestId, ServerInput};
use crate::manager::RendererId;
use crate::network::TabNetworkHandle;
use crate::site::Site;
use crate::storage::SessionStorage;
use crate::wire::{
    BrowserCall, BrowsingContextCall, Command as RendererCommand, FromRenderer, HostNotice,
    MessagingCall, NetworkCall, RendererAssignmentId, RendererCall, RendererNotice, RendererReply,
    Reply, ResponseStart, ServiceCall, ServiceReply, StorageCall, ToRenderer,
};
use renderer::{FrameId, Mount, RendererEvent, StorageChange, StorageKind, TabError};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::timeout;

/// Upper bound on one request to a renderer. The renderer budget is seconds;
/// this is a last-resort wake-up if its reply path dies silently.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Upper bound for one downloaded renderer payload such as a PNG.
const MAX_RENDERER_STREAM_BYTES: usize = 64 * 1024 * 1024;

/// How long a `renderer` child has to send [`RendererNotice::Ready`].
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// How long teardown waits for transport tasks to finish.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

/// Bounded renderer-event handoff to its owning tab coordinator. Saturation is
/// a renderer protocol violation: dropping lifecycle events would corrupt tab
/// state, while blocking the reader could strand a reply behind those events.
const EVENT_SUBSCRIBER_CAPACITY: usize = 4096;

/// Bounded browser-to-renderer command queue.
const COMMAND_CAPACITY: usize = 256;

/// Live subscribers to one renderer's frame-tagged document events.
type EventSubscribers =
    Arc<Mutex<HashMap<RendererAssignmentId, Vec<mpsc::Sender<(FrameId, RendererEvent)>>>>>;

type AssignmentContexts = Arc<Mutex<HashMap<RendererAssignmentId, AssignmentContext>>>;

type RendererClient =
    exchange::Client<RendererCall, ServiceReply, HostNotice, RendererReply, RendererNotice>;
type BrowserServiceServer =
    exchange::Server<RendererCall, ServiceReply, HostNotice, BrowserCall, RendererNotice>;
type BrowserServiceResponder = exchange::Responder<RendererCall, ServiceReply, HostNotice>;
type RendererRouter = exchange::Router<BrowserCall, RendererReply, RendererNotice>;
type ResponseUpload = exchange::Upload<RendererCall, ServiceReply, HostNotice, RendererReply>;

/// Value-only handle to one renderer.
pub(crate) struct RendererHandle {
    client: RendererClient,
    pub(crate) alive: Arc<AtomicBool>,
    subscribers: EventSubscribers,
    contexts: AssignmentContexts,
    kill: watch::Sender<bool>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    pub(crate) site: Arc<Mutex<Option<Site>>>,
    pub(crate) assignments: AtomicUsize,
    /// Assignment ids this process has released. A call for an id that was
    /// never assigned here is a protocol violation, not a late release.
    released: Arc<Mutex<HashSet<RendererAssignmentId>>>,
    _slot: tokio::sync::OwnedSemaphorePermit,
}

/// One browser-authorized top-level document inside a renderer process.
pub(crate) struct RendererAssignment {
    pub(crate) id: RendererAssignmentId,
    pub(crate) process: Arc<RendererHandle>,
}

/// Browser-owned authority attached to one renderer assignment.
#[derive(Clone)]
pub(crate) struct AssignmentContext {
    pub(crate) tab: TabId,
    pub(crate) site: Site,
    pub(crate) network: TabNetworkHandle,
}

impl RendererAssignment {
    pub(crate) async fn request(&self, command: RendererCommand) -> Result<Reply, TabError> {
        self.process.request(self.id, command).await
    }

    pub(crate) async fn mount(
        &self,
        frame: FrameId,
        status: u16,
        mount: Mount,
    ) -> Result<Reply, TabError> {
        self.process.mount(self.id, frame, status, mount).await
    }

    pub(crate) async fn start_response(
        &self,
        frame: FrameId,
        status: u16,
        mount: &Mount,
    ) -> Result<ResponseWriter, TabError> {
        self.process
            .start_response(self.id, frame, status, mount)
            .await
    }

    #[must_use]
    pub(crate) fn subscribe(&self) -> mpsc::Receiver<(FrameId, RendererEvent)> {
        self.process.subscribe(self.id)
    }

    /// Streams one request whose reply is bytes (screenshots).
    pub(crate) async fn request_bytes(
        &self,
        command: RendererCommand,
    ) -> Result<Vec<u8>, TabError> {
        self.process.request_bytes(self.id, command).await
    }
}

pub(crate) struct ResponseWriter {
    assignment: RendererAssignmentId,
    upload: ResponseUpload,
}

impl ResponseWriter {
    pub(crate) async fn write(&self, payload: Vec<u8>) -> Result<(), TabError> {
        if payload.len() > crate::wire::channel::MAX_BODY_CHUNK_BYTES {
            return Err(TabError::RendererUnavailable {
                message: "response chunk exceeds IPC limit".into(),
            });
        }
        self.upload
            .write(payload)
            .await
            .map_err(|_| TabError::ActorStopped)
    }

    pub(crate) async fn finish(self) -> Result<Reply, TabError> {
        let assignment = self.assignment;
        let response = self
            .upload
            .finish()
            .await
            .map_err(|_| TabError::ActorStopped)?;
        decode_reply(assignment, response)
    }

    pub(crate) async fn abort(self, failure: renderer::DialFailure) -> Result<Reply, TabError> {
        let assignment = self.assignment;
        let response = self
            .upload
            .abort(format!("{failure:?}"))
            .await
            .map_err(|_| TabError::ActorStopped)?;
        decode_reply(assignment, response)
    }
}

impl RendererHandle {
    pub(crate) fn bind(&self, site: &Site) -> io::Result<()> {
        let mut lock = self.site.lock().unwrap_or_else(PoisonError::into_inner);
        match &*lock {
            Some(existing) if existing != site => {
                Err(io::Error::other("renderer has a different site lock"))
            }
            Some(_) => Ok(()),
            None => {
                *lock = Some(site.clone());
                Ok(())
            }
        }
    }

    /// Counts an assignment that the manager selected this process for. The
    /// manager holds its state lock across this call, so a concurrent release
    /// cannot observe zero and shut the process down.
    pub(crate) fn reserve_assignment(&self) {
        self.assignments.fetch_add(1, Ordering::Relaxed);
    }

    /// Rolls back a reservation whose `Assign` message never reached the
    /// renderer.
    pub(crate) fn unreserve_assignment(&self) {
        self.assignments.fetch_sub(1, Ordering::Relaxed);
    }

    pub(crate) async fn assign(
        &self,
        assignment: RendererAssignmentId,
        context: AssignmentContext,
    ) -> io::Result<()> {
        self.contexts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(assignment, context);
        let result = self
            .client
            .notify(HostNotice::Assign { assignment })
            .await
            .map_err(|_| io::Error::other("renderer stopped"));
        if result.is_err() {
            self.contexts
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&assignment);
        }
        result
    }

    pub(crate) async fn release(&self, assignment: RendererAssignmentId) {
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&assignment);
        self.contexts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&assignment);
        self.released
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(assignment);
        let _result = self.client.notify(HostNotice::Release { assignment }).await;
        self.assignments.fetch_sub(1, Ordering::Relaxed);
    }

    /// Sends one command and waits for its reply.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`] when the renderer is gone or the reply does
    /// not arrive within the request timeout.
    async fn request(
        &self,
        assignment: RendererAssignmentId,
        command: RendererCommand,
    ) -> Result<Reply, TabError> {
        let response = timeout(
            REQUEST_TIMEOUT,
            self.client.call(RendererCall::Command {
                assignment,
                command,
            }),
        )
        .await
        .map_err(|_| TabError::ActorStopped)?
        .map_err(|_| TabError::ActorStopped)?;
        decode_reply(assignment, response)
    }

    /// Sends one command whose reply is a streamed byte payload and waits for
    /// it to complete.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`] when the renderer is gone, the stream is
    /// incomplete when the process dies, or the reply times out.
    async fn request_bytes(
        &self,
        assignment: RendererAssignmentId,
        command: RendererCommand,
    ) -> Result<Vec<u8>, TabError> {
        let (response, bytes) = timeout(
            REQUEST_TIMEOUT,
            self.client.call_download(
                RendererCall::Command {
                    assignment,
                    command,
                },
                MAX_RENDERER_STREAM_BYTES,
            ),
        )
        .await
        .map_err(|_| TabError::ActorStopped)?
        .map_err(|_| TabError::ActorStopped)?;
        match decode_reply(assignment, response)? {
            Reply::Screenshot {
                result: Ok(expected),
            } if usize::try_from(expected).ok() == Some(bytes.len()) => Ok(bytes),
            Reply::Screenshot { result: Ok(_) } => Err(TabError::RendererUnavailable {
                message: "screenshot stream length mismatch".into(),
            }),
            Reply::Screenshot { result: Err(error) } => Err(error),
            other => Err(TabError::RendererUnavailable {
                message: format!("unexpected reply for byte request: {other:?}"),
            }),
        }
    }

    /// Streams one top-level response to the renderer and waits for its mount
    /// result. The body is carried only in bounded raw IPC frames.
    async fn mount(
        &self,
        assignment: RendererAssignmentId,
        frame: FrameId,
        status: u16,
        mount: Mount,
    ) -> Result<Reply, TabError> {
        let response = self
            .start_response(assignment, frame, status, &mount)
            .await?;
        for chunk in mount
            .body
            .chunks(crate::wire::channel::MAX_BODY_CHUNK_BYTES)
        {
            response.write(chunk.to_vec()).await?;
        }
        response.finish().await
    }

    async fn start_response(
        &self,
        assignment: RendererAssignmentId,
        frame: FrameId,
        status: u16,
        mount: &Mount,
    ) -> Result<ResponseWriter, TabError> {
        let start = ResponseStart {
            assignment,
            frame,
            status,
            final_url: mount.url.clone(),
            content_type: mount.content_type.clone(),
            content_language: mount.content_language.clone(),
        };
        let upload = self
            .client
            .begin_upload(RendererCall::Response { response: start })
            .await
            .map_err(|_| TabError::ActorStopped)?;
        Ok(ResponseWriter { assignment, upload })
    }
    /// Subscribes to renderer document events after this call.
    #[must_use]
    fn subscribe(
        &self,
        assignment: RendererAssignmentId,
    ) -> mpsc::Receiver<(FrameId, RendererEvent)> {
        let (tx, rx) = mpsc::channel(EVENT_SUBSCRIBER_CAPACITY);
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(assignment)
            .or_default()
            .push(tx);
        rx
    }

    /// Interrupts a blocked script by killing the renderer process.
    pub(crate) fn interrupt(&self) {
        let _ = self.kill.send(true);
    }

    /// Asks the renderer loop to stop without waiting.
    pub(crate) fn request_shutdown(&self) {
        let _result = self.client.try_notify(HostNotice::Shutdown);
    }

    /// Stops the renderer and waits for its transport tasks.
    pub(crate) async fn shutdown(&self) {
        self.request_shutdown();
        self.interrupt();
        let tasks = std::mem::take(&mut *self.tasks.lock().unwrap_or_else(PoisonError::into_inner));
        for mut task in tasks {
            if timeout(SHUTDOWN_TIMEOUT, &mut task).await.is_err() {
                task.abort();
                let _result = task.await;
            }
        }
    }
}

fn decode_reply(
    assignment: RendererAssignmentId,
    response: RendererReply,
) -> Result<Reply, TabError> {
    if response.assignment == assignment {
        Ok(response.reply)
    } else {
        Err(TabError::ActorStopped)
    }
}

impl Drop for RendererHandle {
    fn drop(&mut self) {
        // Dropping `kill` stops the child task's watch and reaps the child.
        for task in std::mem::take(&mut *self.tasks.lock().unwrap_or_else(PoisonError::into_inner))
        {
            task.abort();
        }
    }
}

struct RendererViolation;

struct ReaderContext {
    router: RendererRouter,
    alive: Arc<AtomicBool>,
    kill: watch::Sender<bool>,
}

struct ServiceContext {
    responder: BrowserServiceResponder,
    subscribers: EventSubscribers,
    assignments: AssignmentContexts,
    partition: PartitionServices,
    sessions: Arc<SessionStorage>,
    released: Arc<Mutex<HashSet<RendererAssignmentId>>>,
    kill: watch::Sender<bool>,
    /// Browser command handle: renderer links create tabs for `window.open`.
    browser: crate::browser::BrowserHandle,
}

async fn writer_task(
    mut rx: exchange::Receiver<ToRenderer>,
    mut writer: Box<dyn AsyncWrite + Send + Unpin>,
    alive: Arc<AtomicBool>,
    router: RendererRouter,
    kill: watch::Sender<bool>,
    mut kill_rx: watch::Receiver<bool>,
) {
    if *kill_rx.borrow() {
        return;
    }
    loop {
        let message = tokio::select! {
            biased;
            message = rx.recv() => match message {
                Some(message) => message,
                None => return,
            },
            _ = kill_rx.changed() => return,
        };
        let result = write_to_renderer(&mut *writer, &message).await;
        if result.is_err() {
            fail(&alive, &router, &kill);
            return;
        }
    }
}

async fn write_to_renderer<W>(writer: &mut W, message: &ToRenderer) -> io::Result<()>
where
    W: AsyncWrite + Unpin + ?Sized,
{
    match message {
        Frame::RequestChunk { id, bytes } => {
            crate::wire::channel::write_frame_async(
                writer,
                crate::wire::channel::FrameKind::RequestChunk,
                id.get(),
                bytes,
            )
            .await
        }
        Frame::ResponseChunk { id, bytes } => {
            crate::wire::channel::write_frame_async(
                writer,
                crate::wire::channel::FrameKind::ResponseChunk,
                id.get(),
                bytes,
            )
            .await
        }
        _ => crate::wire::channel::write_control_async(writer, message).await,
    }
}

async fn reader_task(
    mut reader: Box<dyn AsyncRead + Send + Unpin>,
    context: ReaderContext,
    ready: Option<oneshot::Sender<bool>>,
) {
    let mut ready = ready;
    let mut buffer = Vec::new();
    loop {
        let frame = match crate::wire::channel::read_frame_async(&mut *reader, &mut buffer).await {
            Ok(Some(frame)) => frame,
            Ok(None) => break,
            Err(error) => {
                logging::error!(target: "browser::link", "bad renderer message: {error}");
                break;
            }
        };
        let message = match frame.kind {
            crate::wire::channel::FrameKind::Control => {
                match crate::wire::channel::decode_control::<FromRenderer>(&buffer) {
                    Ok(message) => message,
                    Err(error) => {
                        logging::error!(target: "browser::link", "bad renderer message: {error}");
                        break;
                    }
                }
            }
            crate::wire::channel::FrameKind::RequestChunk => Frame::RequestChunk {
                id: RequestId::new(frame.request),
                bytes: buffer.clone(),
            },
            crate::wire::channel::FrameKind::ResponseChunk => Frame::ResponseChunk {
                id: RequestId::new(frame.request),
                bytes: buffer.clone(),
            },
        };
        if let Some(ready_tx) = ready.take() {
            let ok = matches!(message, Frame::Notify(RendererNotice::Ready));
            let _ = ready_tx.send(ok);
            if !ok {
                break;
            }
            continue;
        }
        if context.router.route(message).is_err() {
            break;
        }
    }
    if let Some(ready_tx) = ready {
        let _ = ready_tx.send(false);
    }
    fail(&context.alive, &context.router, &context.kill);
}

/// Runs browser services and renderer notifications after the exchange router
/// has separated them from replies and stream chunks.
async fn service_task(mut server: BrowserServiceServer, context: ServiceContext) {
    while let Some(input) = server.recv().await {
        let result = match input {
            ServerInput::Call {
                id,
                body: BrowserCall { assignment, call },
            } => route_service_call(&context, id, assignment, call).await,
            ServerInput::Notify(RendererNotice::Event {
                assignment,
                frame,
                event,
            }) => route_event(&context, assignment, frame, event),
            ServerInput::Notify(RendererNotice::Ready)
            | ServerInput::RequestChunk { .. }
            | ServerInput::RequestEnd { .. }
            | ServerInput::Cancel { .. } => Err(RendererViolation),
        };
        if result.is_err() {
            let _result = context.kill.send(true);
            return;
        }
    }
}

fn route_event(
    context: &ServiceContext,
    assignment: RendererAssignmentId,
    frame: FrameId,
    event: RendererEvent,
) -> Result<(), RendererViolation> {
    let mut saturated = false;
    let mut subscribers = context
        .subscribers
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let Some(assignment_subscribers) = subscribers.get_mut(&assignment) else {
        return if was_released(&context.released, assignment) {
            Ok(())
        } else {
            Err(RendererViolation)
        };
    };
    assignment_subscribers.retain(|subscriber| match subscriber.try_send((frame, event)) {
        Ok(()) => true,
        Err(mpsc::error::TrySendError::Closed(_)) => false,
        Err(mpsc::error::TrySendError::Full(_)) => {
            saturated = true;
            false
        }
    });
    if saturated {
        Err(RendererViolation)
    } else {
        Ok(())
    }
}

async fn route_service_call(
    context: &ServiceContext,
    id: RequestId,
    assignment: RendererAssignmentId,
    call: ServiceCall,
) -> Result<(), RendererViolation> {
    let assignment_context = context
        .assignments
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&assignment)
        .cloned();
    let Some(assignment_context) = assignment_context else {
        // The renderer may have queued the call before it processed
        // `Release`. Answer with a benign failure so a synchronous
        // `document.cookie` caller cannot block forever, and keep the
        // process alive for its other assignments.
        if was_released(&context.released, assignment) {
            return send_released_reply(&context.responder, id, &call).await;
        }
        return Err(RendererViolation);
    };
    match call {
        ServiceCall::Network(NetworkCall::Dial(request)) => {
            let Some(initiator) = assignment_context.site.authorize(&request.initiator) else {
                return Err(RendererViolation);
            };
            let worker_network = assignment_context.network.clone();
            let responder = context.responder.clone();
            let worker_kill = context.kill.clone();
            let cancel = context.kill.subscribe();
            tokio::spawn(async move {
                let outcome = worker_network
                    .dial_request(&request, &initiator, cancel)
                    .await;
                if responder
                    .reply(id, ServiceReply::Dial(outcome))
                    .await
                    .is_err()
                {
                    let _result = worker_kill.send(true);
                }
            });
        }
        ServiceCall::Network(NetworkCall::CookieGet { url }) => {
            let Some(url) = assignment_context.site.authorize(&url) else {
                return Err(RendererViolation);
            };
            let reply = ServiceReply::Cookie(assignment_context.network.cookies_for(&url));
            send_reply(&context.responder, id, reply).await?;
        }
        ServiceCall::Network(NetworkCall::CookieSet { value, url }) => {
            let Some(url) = assignment_context.site.authorize(&url) else {
                return Err(RendererViolation);
            };
            assignment_context.network.set_cookie(&value, &url);
            send_reply(&context.responder, id, ServiceReply::Unit).await?;
        }
        ServiceCall::Messaging(MessagingCall::BroadcastPost {
            origin,
            name,
            payload,
            channel,
        }) => {
            context
                .partition
                .events
                .broadcast(&ContextEvent::Broadcast(BroadcastMessage {
                    origin,
                    name,
                    payload,
                    source: (assignment, channel),
                }));
            send_reply(&context.responder, id, ServiceReply::Unit).await?;
        }
        ServiceCall::Storage(call) => {
            return route_storage_call(context, assignment, &assignment_context, id, call).await;
        }
        ServiceCall::BrowsingContext(call) => {
            return route_window_call(context, &assignment_context, id, call).await;
        }
    }
    Ok(())
}

/// Routes one `window.open`/`window.close`/`postMessage` service call. Step 1
/// created the tab and its first navigation; step 2 adds the opener link and
/// cross-tab messaging.
async fn route_window_call(
    context: &ServiceContext,
    assignment: &AssignmentContext,
    id: RequestId,
    call: BrowsingContextCall,
) -> Result<(), RendererViolation> {
    match call {
        BrowsingContextCall::WindowOpen { url, features, .. } => {
            let spec = if url.is_empty() || url == "about:blank" {
                Some(String::new())
            } else {
                url::Url::parse(&url)
                    .ok()
                    .filter(|url| matches!(url.scheme(), "http" | "https"))
                    .map(|url| url.to_string())
            };
            let source = Some(assignment.tab);
            let noopener = features
                .split(|character: char| character.is_ascii_whitespace() || character == ',')
                .any(|feature| {
                    feature.eq_ignore_ascii_case("noopener")
                        || feature.eq_ignore_ascii_case("noreferrer")
                });
            let tab = match spec {
                Some(spec) => context
                    .browser
                    .open_window(spec, source, noopener)
                    .await
                    .ok()
                    .map(TabId::get),
                None => None,
            };
            send_reply(&context.responder, id, ServiceReply::Window(tab)).await?;
        }
        BrowsingContextCall::WindowClose { tab } => {
            let _result = context.browser.close_tab(TabId::new(tab)).await;
            send_reply(&context.responder, id, ServiceReply::Unit).await?;
        }
        BrowsingContextCall::Opener => {
            let opener = context
                .browser
                .opener_tab(assignment.tab)
                .await
                .ok()
                .flatten()
                .map(TabId::get);
            send_reply(&context.responder, id, ServiceReply::Window(opener)).await?;
        }
        BrowsingContextCall::WindowMessage { tab, payload } => {
            let _result = context
                .browser
                .window_message(TabId::new(tab), payload)
                .await;
            send_reply(&context.responder, id, ServiceReply::Unit).await?;
        }
        BrowsingContextCall::RemoteSessionGet { tab, origin, key } => {
            if assignment.site.authorize(&origin).is_none() {
                return Err(RendererViolation);
            }
            let target = TabId::new(tab);
            let related = target == assignment.tab
                || context.browser.opener_tab(target).await.ok().flatten() == Some(assignment.tab)
                || context
                    .browser
                    .opener_tab(assignment.tab)
                    .await
                    .ok()
                    .flatten()
                    == Some(target);
            let value = related
                .then(|| context.sessions.get(target, &origin, &key))
                .flatten();
            send_reply(&context.responder, id, ServiceReply::StorageValue(value)).await?;
        }
    }
    Ok(())
}

struct StorageRouter<'a> {
    context: &'a ServiceContext,
    assignment: RendererAssignmentId,
    authority: &'a AssignmentContext,
    request: RequestId,
}

impl StorageRouter<'_> {
    async fn route(&self, call: StorageCall) -> Result<(), RendererViolation> {
        match call {
            StorageCall::Get { kind, origin, key } => self.get(kind, &origin, &key).await,
            StorageCall::Keys { kind, origin } => self.keys(kind, &origin).await,
            StorageCall::Set {
                kind,
                origin,
                url,
                key,
                value,
                source,
            } => self.set(kind, &origin, &url, &key, &value, source).await,
            StorageCall::Remove {
                kind,
                origin,
                url,
                key,
                source,
            } => self.remove(kind, &origin, &url, &key, source).await,
            StorageCall::Clear {
                kind,
                origin,
                url,
                source,
            } => self.clear(kind, &origin, &url, source).await,
        }
    }

    async fn get(
        &self,
        kind: StorageKind,
        origin: &str,
        key: &str,
    ) -> Result<(), RendererViolation> {
        self.authorize(origin, None)?;
        let value = match kind {
            StorageKind::Local => self.context.partition.local_storage.get(origin, key),
            StorageKind::Session => self.context.sessions.get(self.authority.tab, origin, key),
        };
        send_reply(
            &self.context.responder,
            self.request,
            ServiceReply::StorageValue(value),
        )
        .await
    }

    async fn keys(&self, kind: StorageKind, origin: &str) -> Result<(), RendererViolation> {
        self.authorize(origin, None)?;
        let keys = match kind {
            StorageKind::Local => self.context.partition.local_storage.keys(origin),
            StorageKind::Session => self.context.sessions.keys(self.authority.tab, origin),
        };
        send_reply(
            &self.context.responder,
            self.request,
            ServiceReply::StorageKeys(keys),
        )
        .await
    }

    async fn set(
        &self,
        kind: StorageKind,
        origin: &str,
        url: &str,
        key: &str,
        value: &str,
        source: FrameId,
    ) -> Result<(), RendererViolation> {
        self.authorize(origin, Some(url))?;
        let change = match kind {
            StorageKind::Local => self.context.partition.local_storage.set(origin, key, value),
            StorageKind::Session => {
                self.context
                    .sessions
                    .set(self.authority.tab, origin, key, value)
            }
        };
        self.publish(
            source,
            kind,
            origin,
            url,
            change.as_ref().ok().and_then(Option::as_ref),
        );
        send_reply(
            &self.context.responder,
            self.request,
            ServiceReply::StorageChanged(change),
        )
        .await
    }

    async fn remove(
        &self,
        kind: StorageKind,
        origin: &str,
        url: &str,
        key: &str,
        source: FrameId,
    ) -> Result<(), RendererViolation> {
        self.authorize(origin, Some(url))?;
        let change = match kind {
            StorageKind::Local => self.context.partition.local_storage.remove(origin, key),
            StorageKind::Session => self
                .context
                .sessions
                .remove(self.authority.tab, origin, key),
        };
        self.publish(source, kind, origin, url, change.as_ref());
        send_reply(
            &self.context.responder,
            self.request,
            ServiceReply::StorageChanged(Ok(change)),
        )
        .await
    }

    async fn clear(
        &self,
        kind: StorageKind,
        origin: &str,
        url: &str,
        source: FrameId,
    ) -> Result<(), RendererViolation> {
        self.authorize(origin, Some(url))?;
        let change = match kind {
            StorageKind::Local => self.context.partition.local_storage.clear(origin),
            StorageKind::Session => self.context.sessions.clear(self.authority.tab, origin),
        };
        self.publish(source, kind, origin, url, change.as_ref());
        send_reply(
            &self.context.responder,
            self.request,
            ServiceReply::StorageChanged(Ok(change)),
        )
        .await
    }

    fn authorize(&self, origin: &str, url: Option<&str>) -> Result<(), RendererViolation> {
        if self.authority.site.authorize(origin).is_none()
            || url.is_some_and(|url| self.authority.site.authorize(url).is_none())
        {
            return Err(RendererViolation);
        }
        Ok(())
    }

    fn publish(
        &self,
        source: FrameId,
        kind: StorageKind,
        origin: &str,
        url: &str,
        change: Option<&StorageChange>,
    ) {
        let Some(change) = change else {
            return;
        };
        if kind == StorageKind::Local {
            broadcast_local_storage_event(
                self.context,
                self.assignment,
                source,
                origin,
                url,
                change,
            );
        }
    }
}

async fn route_storage_call(
    context: &ServiceContext,
    assignment: RendererAssignmentId,
    assignment_context: &AssignmentContext,
    id: RequestId,
    call: StorageCall,
) -> Result<(), RendererViolation> {
    StorageRouter {
        context,
        assignment,
        authority: assignment_context,
        request: id,
    }
    .route(call)
    .await
}

fn broadcast_local_storage_event(
    context: &ServiceContext,
    assignment: RendererAssignmentId,
    source: FrameId,
    origin: &str,
    url: &str,
    change: &StorageChange,
) {
    context
        .partition
        .events
        .broadcast(&ContextEvent::Storage(StorageBroadcast {
            origin: origin.to_owned(),
            kind: StorageKind::Local,
            key: change.key.clone(),
            old_value: change.old_value.clone(),
            new_value: change.new_value.clone(),
            url: url.to_owned(),
            source: (assignment, source),
        }));
}

fn was_released(
    released: &Mutex<HashSet<RendererAssignmentId>>,
    assignment: RendererAssignmentId,
) -> bool {
    released
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .contains(&assignment)
}

/// Answers a late call from a released assignment so the renderer's blocking
/// service path stays unblocked while its engine is torn down.
async fn send_released_reply(
    responder: &BrowserServiceResponder,
    id: RequestId,
    call: &ServiceCall,
) -> Result<(), RendererViolation> {
    let reply = match call {
        ServiceCall::Network(NetworkCall::Dial(_)) => {
            ServiceReply::Dial(Err(renderer::DialFailure::Cancelled))
        }
        ServiceCall::Network(NetworkCall::CookieGet { .. }) => ServiceReply::Cookie(String::new()),
        ServiceCall::Network(NetworkCall::CookieSet { .. })
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
    };
    send_reply(responder, id, reply).await
}

async fn send_reply(
    responder: &BrowserServiceResponder,
    id: RequestId,
    reply: ServiceReply,
) -> Result<(), RendererViolation> {
    responder
        .reply(id, reply)
        .await
        .map_err(|_| RendererViolation)
}

fn fail(alive: &Arc<AtomicBool>, router: &RendererRouter, kill: &watch::Sender<bool>) {
    alive.store(false, Ordering::Relaxed);
    router.close();
    let _ = kill.send(true);
}

/// Forwards one renderer child's stderr into this process's logger.
///
/// The child formats its own level and target; the browser only forwards the
/// lines, so renderer records land in the daemon's console and file without a
/// second file writer.
async fn forward_stderr(stderr: tokio::process::ChildStderr) {
    let mut reader = tokio::io::BufReader::new(stderr);
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        match reader.read_until(b'\n', &mut bytes).await {
            Ok(0) | Err(_) => break,
            Ok(_) => logging::log_forwarded(&String::from_utf8_lossy(&bytes)),
        }
    }
}

/// One spawned child and its host-side channel endpoints.
struct SpawnedRenderer {
    child: Child,
    reader: Box<dyn AsyncRead + Send + Unpin>,
    writer: Box<dyn AsyncWrite + Send + Unpin>,
}

/// Spawns one renderer child and returns its host-side reader and writer.
///
/// Unix passes one end of an unnamed socket pair as the child's file
/// descriptor 0. Other platforms keep the piped stdin/stdout transport until
/// their platform channel lands.
#[cfg(unix)]
fn spawn_transport(command: &mut Command) -> io::Result<SpawnedRenderer> {
    let (host, child_end) = std::os::unix::net::UnixStream::pair()?;
    command
        .stdin(Stdio::from(std::os::fd::OwnedFd::from(child_end)))
        .stdout(Stdio::null());
    let child = command.spawn()?;
    let writer_std = host.try_clone()?;
    host.set_nonblocking(true)?;
    writer_std.set_nonblocking(true)?;
    let reader = tokio::net::UnixStream::from_std(host)?;
    let writer = tokio::net::UnixStream::from_std(writer_std)?;
    Ok(SpawnedRenderer {
        child,
        reader: Box::new(reader),
        writer: Box::new(writer),
    })
}

#[cfg(not(unix))]
fn spawn_transport(command: &mut Command) -> io::Result<SpawnedRenderer> {
    command.stdin(Stdio::piped()).stdout(Stdio::piped());
    let mut child = command.spawn()?;
    let Some(stdin) = child.stdin.take() else {
        let _ = child.start_kill();
        return Err(io::Error::other("renderer stdin missing"));
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.start_kill();
        return Err(io::Error::other("renderer stdout missing"));
    };
    Ok(SpawnedRenderer {
        child,
        reader: Box::new(stdout),
        writer: Box::new(stdin),
    })
}

/// Registers browser-to-renderer notifications. A saturated command inbox
/// drops storage and broadcast notices instead of disconnecting the renderer;
/// the renderer may be blocked in a synchronous service call, and losing an
/// observable event is better than killing the page. A closed client removes
/// the subscription and stops the renderer.
fn subscribe_renderer_events(
    partition: &PartitionServices,
    client: &RendererClient,
    kill: &watch::Sender<bool>,
) {
    let client = client.clone();
    let kill = kill.clone();
    partition.events.subscribe(Box::new(move |event| {
        let notice = match event {
            ContextEvent::Storage(event) => HostNotice::StorageEvent {
                origin: event.origin.clone(),
                kind: event.kind,
                key: event.key.clone(),
                old_value: event.old_value.clone(),
                new_value: event.new_value.clone(),
                url: event.url.clone(),
                source: Some(event.source),
            },
            ContextEvent::Broadcast(message) => HostNotice::BroadcastMessage {
                origin: message.origin.clone(),
                name: message.name.clone(),
                payload: message.payload.clone(),
                source: Some(message.source),
            },
        };
        if client.try_notify_lossy(notice).is_ok() {
            true
        } else {
            let _result = kill.send(true);
            false
        }
    }));
}

pub(crate) async fn spawn_process(
    id: RendererId,
    site: Option<Site>,
    partition: PartitionServices,
    sessions: Arc<SessionStorage>,
    browser: crate::browser::BrowserHandle,
    slot: tokio::sync::OwnedSemaphorePermit,
) -> io::Result<RendererHandle> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("renderer")
        .env("TINYBROWSER_LOG", logging::level().as_str())
        .stderr(Stdio::piped());
    let SpawnedRenderer {
        mut child,
        reader,
        writer,
    } = spawn_transport(&mut command)?;
    let Some(stderr) = child.stderr.take() else {
        let _ = child.start_kill();
        return Err(io::Error::other("renderer stderr missing"));
    };
    let (tx, rx) = exchange::pair(COMMAND_CAPACITY, COMMAND_CAPACITY);
    let (client, server, router) = exchange::endpoint(tx, COMMAND_CAPACITY);
    let (kill, kill_rx) = watch::channel(false);
    subscribe_renderer_events(&partition, &client, &kill);
    let alive = Arc::new(AtomicBool::new(true));
    let subscribers = Arc::new(Mutex::new(HashMap::new()));
    let contexts = Arc::new(Mutex::new(HashMap::new()));
    let (ready_tx, ready_rx) = oneshot::channel();
    let writer_task = tokio::spawn(writer_task(
        rx,
        writer,
        Arc::clone(&alive),
        router.clone(),
        kill.clone(),
        kill_rx.clone(),
    ));
    let site = Arc::new(Mutex::new(site));
    let released = Arc::new(Mutex::new(HashSet::new()));
    let reader_context = ReaderContext {
        router: router.clone(),
        alive: Arc::clone(&alive),
        kill: kill.clone(),
    };
    let service_context = ServiceContext {
        responder: server.responder(),
        subscribers: Arc::clone(&subscribers),
        assignments: Arc::clone(&contexts),
        partition,
        sessions,
        released: Arc::clone(&released),
        kill: kill.clone(),
        browser,
    };
    let reader_task = tokio::spawn(reader_task(reader, reader_context, Some(ready_tx)));
    let service_task = tokio::spawn(service_task(server, service_context));
    let stderr_task = tokio::spawn(forward_stderr(stderr));
    // Detached child task: it reaps the child when the channel closes or the
    // handle signals a kill. Dropping every kill sender also stops it.
    let _reaper = tokio::spawn(child_task(child, kill_rx));
    if client.notify(HostNotice::Hello).await.is_err()
        || timeout(HANDSHAKE_TIMEOUT, ready_rx).await != Ok(Ok(true))
    {
        let _ = kill.send(true);
        let _result = writer_task.await;
        let _result = reader_task.await;
        let _result = service_task.await;
        let _result = stderr_task.await;
        return Err(io::Error::other("renderer handshake failed"));
    }
    logging::debug!(
        target: "browser::link",
        "renderer {id:?} ready for site lock {site:?}"
    );
    let tasks = vec![writer_task, reader_task, service_task, stderr_task];
    Ok(RendererHandle {
        client,
        alive,
        subscribers,
        contexts,
        kill,
        tasks: Mutex::new(tasks),
        site,
        assignments: AtomicUsize::new(0),
        released,
        _slot: slot,
    })
}

async fn child_task(mut child: Child, mut kill: watch::Receiver<bool>) {
    if *kill.borrow() {
        let _ = child.start_kill();
        let _ = child.wait().await;
        return;
    }
    tokio::select! {
        _ = kill.changed() => {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        _ = child.wait() => {}
    }
}
