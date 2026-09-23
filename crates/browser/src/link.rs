//! Browser side of the renderer seam: factory, handles, and async routing.
//!
//! One private platform channel per renderer, length-prefixed frames, async
//! reader and writer tasks, and oneshot replies. The handle stays value-only.

use std::io;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::process::Child;

use crate::actor::TabId;
use crate::assignment::{Assignment, AssignmentRegistry};
use crate::broadcast::{BroadcastMessage, ContextEvent, StorageBroadcast};
use crate::context::PartitionServices;
use crate::exchange::{self, Frame, RequestId, ServerInput};
use crate::manager::RendererId;
use crate::site::Site;
use crate::storage::{LocalSetOptions, SessionKeyOptions, SessionSetOptions, SessionStorage};
use crate::wire::{
    BrowserCall, BrowsingContextCall, Command as RendererCommand, FromRenderer, HostNotice,
    MessagingCall, NetworkCall, RendererAssignmentId, RendererCall, RendererNotice, RendererReply,
    Reply, ResponseStart, ServiceCall, ServiceReply, StorageCall, ToRenderer,
};
use renderer::{FrameId, Mount, RendererEvent, StorageChange, StorageKind, TabError};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite};
use tokio::process::Command;
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::timeout;

/// Upper bound on one request to a renderer. The renderer budget is seconds;
/// this is a last-resort wake-up if its reply path dies silently. A renderer
/// that misses it is interrupted so the tab can re-acquire a fresh process
/// instead of paying the timeout on every later operation.
///
/// `TINYBROWSER_RENDERER_REQUEST_TIMEOUT_MS` overrides it so recovery tests
/// do not wait a full minute.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// The effective renderer request deadline.
fn request_timeout() -> Duration {
    std::env::var_os("TINYBROWSER_RENDERER_REQUEST_TIMEOUT_MS")
        .and_then(|value| value.to_str()?.parse::<u64>().ok())
        .filter(|millis| *millis > 0)
        .map_or(REQUEST_TIMEOUT, Duration::from_millis)
}

/// Upper bound for one downloaded renderer payload such as a PNG.
const MAX_RENDERER_STREAM_BYTES: usize = 64 * 1024 * 1024;

/// How long a `renderer` child has to send [`RendererNotice::Ready`].
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// How long teardown waits for transport tasks to finish.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

/// Bounded browser-to-renderer command queue.
const COMMAND_CAPACITY: usize = 256;

type RendererClient =
    exchange::Client<RendererCall, ServiceReply, HostNotice, RendererReply, RendererNotice>;
type BrowserServiceServer =
    exchange::Server<RendererCall, ServiceReply, HostNotice, BrowserCall, RendererNotice>;
type BrowserServiceResponder = exchange::Responder<RendererCall, ServiceReply, HostNotice>;
type RendererRouter = exchange::Router<BrowserCall, RendererReply, RendererNotice>;
type ResponseUpload = exchange::Upload<RendererCall, ServiceReply, HostNotice, RendererReply>;

/// Value-only transport handle to one renderer process.
pub(crate) struct RendererHandle {
    client: RendererClient,
    pub(crate) alive: Arc<AtomicBool>,
    kill: watch::Sender<bool>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    pub(crate) site: Arc<Mutex<Option<Site>>>,
    /// Live assignments counted against this process. The assignment registry
    /// increments while the manager's state lock is held; release decrements
    /// before the manager re-evaluates shutdown.
    pub(crate) assignments: AtomicUsize,
    _slot: tokio::sync::OwnedSemaphorePermit,
}

/// One document mount into the process's renderer.
pub(crate) struct HandleMountOptions {
    pub(crate) assignment: RendererAssignmentId,
    pub(crate) frame: FrameId,
    pub(crate) status: u16,
    pub(crate) mount: Mount,
}

/// One streamed response start into the process's renderer.
pub(crate) struct HandleStartResponseOptions<'a> {
    pub(crate) assignment: RendererAssignmentId,
    pub(crate) frame: FrameId,
    pub(crate) status: u16,
    pub(crate) mount: &'a Mount,
}

pub(crate) struct ResponseWriter {
    assignment: RendererAssignmentId,
    process: Arc<RendererHandle>,
    upload: ResponseUpload,
}

impl ResponseWriter {
    pub(crate) async fn write(&self, payload: Vec<u8>) -> Result<(), TabError> {
        if payload.len() > crate::wire::channel::MAX_BODY_CHUNK_BYTES {
            return Err(TabError::RendererUnavailable {
                message: "response chunk exceeds IPC limit".into(),
            });
        }
        if let Ok(result) = timeout(request_timeout(), self.upload.write(payload)).await {
            result.map_err(|_| TabError::ActorStopped)
        } else {
            self.process.interrupt();
            Err(TabError::ActorStopped)
        }
    }

    pub(crate) async fn finish(self) -> Result<Reply, TabError> {
        let Self {
            assignment,
            process,
            upload,
        } = self;
        let response = if let Ok(result) = timeout(request_timeout(), upload.finish()).await {
            result.map_err(|_| TabError::ActorStopped)?
        } else {
            process.interrupt();
            return Err(TabError::ActorStopped);
        };
        decode_reply(assignment, response)
    }

    pub(crate) async fn abort(self, failure: renderer::DialFailure) -> Result<Reply, TabError> {
        let Self {
            assignment,
            process,
            upload,
        } = self;
        let response =
            if let Ok(result) = timeout(request_timeout(), upload.abort(format!("{failure:?}"))).await
            {
                result.map_err(|_| TabError::ActorStopped)?
            } else {
                process.interrupt();
                return Err(TabError::ActorStopped);
            };
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

    /// Tells the renderer to create the engine for one reserved assignment.
    pub(crate) async fn assign_notify(&self, assignment: RendererAssignmentId) -> io::Result<()> {
        self.client
            .notify(HostNotice::Assign { assignment })
            .await
            .map_err(|_| io::Error::other("renderer stopped"))
    }

    /// Best-effort release notice from the assignment registry's drop path.
    /// A dead renderer needs none.
    pub(crate) fn try_notify_release(&self, assignment: RendererAssignmentId) {
        let _ = self.client.try_notify(HostNotice::Release { assignment });
    }

    /// Sends one command and waits for its reply.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`] when the renderer is gone or the reply does
    /// not arrive within the request timeout. A timeout also interrupts the
    /// renderer: a process that stopped answering must not keep its tab
    /// hostage.
    pub(crate) async fn request(
        &self,
        assignment: RendererAssignmentId,
        command: RendererCommand,
    ) -> Result<Reply, TabError> {
        let response = if let Ok(result) = timeout(
            request_timeout(),
            self.client.call(RendererCall::Command {
                assignment,
                command,
            }),
        )
        .await
        {
            result.map_err(|_| TabError::ActorStopped)?
        } else {
            self.interrupt();
            return Err(TabError::ActorStopped);
        };
        decode_reply(assignment, response)
    }

    /// Sends one command whose reply is a streamed byte payload and waits for
    /// it to complete.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`] when the renderer is gone, the stream is
    /// incomplete when the process dies, or the reply times out.
    pub(crate) async fn request_bytes(
        &self,
        assignment: RendererAssignmentId,
        command: RendererCommand,
    ) -> Result<Vec<u8>, TabError> {
        let (response, bytes) = if let Ok(result) = timeout(
            request_timeout(),
            self.client.call_download(
                RendererCall::Command {
                    assignment,
                    command,
                },
                MAX_RENDERER_STREAM_BYTES,
            ),
        )
        .await
        {
            result.map_err(|_| TabError::ActorStopped)?
        } else {
            self.interrupt();
            return Err(TabError::ActorStopped);
        };
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
    pub(crate) async fn mount(
        self: &Arc<Self>,
        options: HandleMountOptions,
    ) -> Result<Reply, TabError> {
        let HandleMountOptions {
            assignment,
            frame,
            status,
            mount,
        } = options;
        let response = self
            .start_response(HandleStartResponseOptions {
                assignment,
                frame,
                status,
                mount: &mount,
            })
            .await?;
        for chunk in mount
            .body
            .chunks(crate::wire::channel::MAX_BODY_CHUNK_BYTES)
        {
            response.write(chunk.to_vec()).await?;
        }
        response.finish().await
    }

    pub(crate) async fn start_response(
        self: &Arc<Self>,
        options: HandleStartResponseOptions<'_>,
    ) -> Result<ResponseWriter, TabError> {
        let HandleStartResponseOptions {
            assignment,
            frame,
            status,
            mount,
        } = options;
        let start = ResponseStart {
            assignment,
            frame,
            status,
            final_url: mount.url.clone(),
            content_type: mount.content_type.clone(),
            content_language: mount.content_language.clone(),
        };
        let upload = if let Ok(result) = timeout(
            request_timeout(),
            self.client
                .begin_upload(RendererCall::Response { response: start }),
        )
        .await
        {
            result.map_err(|_| TabError::ActorStopped)?
        } else {
            self.interrupt();
            return Err(TabError::ActorStopped);
        };
        Ok(ResponseWriter {
            assignment,
            process: Arc::clone(self),
            upload,
        })
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
    registry: Arc<AssignmentRegistry>,
    partition: PartitionServices,
    sessions: Arc<SessionStorage>,
    kill: watch::Sender<bool>,
    /// Browser command handle: renderer links create tabs for `window.open`.
    browser: crate::browser::BrowserHandle,
}

/// One renderer transport writer task.
struct WriterTaskOptions {
    rx: exchange::Receiver<ToRenderer>,
    writer: Box<dyn AsyncWrite + Send + Unpin>,
    alive: Arc<AtomicBool>,
    router: RendererRouter,
    kill: watch::Sender<bool>,
    kill_rx: watch::Receiver<bool>,
}

/// One renderer transport reader task.
struct ReaderTaskOptions {
    reader: Box<dyn AsyncRead + Send + Unpin>,
    context: ReaderContext,
    ready: Option<oneshot::Sender<bool>>,
}

/// One renderer document event routed to subscribers.
#[derive(Clone, Copy)]
struct RouteEventOptions<'a> {
    context: &'a ServiceContext,
    assignment: RendererAssignmentId,
    frame: FrameId,
    event: RendererEvent,
}

/// One browser service call from a renderer.
struct RouteServiceCallOptions<'a> {
    context: &'a ServiceContext,
    id: RequestId,
    assignment: RendererAssignmentId,
    call: ServiceCall,
}

/// One `window` service call from a renderer.
struct RouteWindowCallOptions<'a> {
    context: &'a ServiceContext,
    assignment: &'a Assignment,
    id: RequestId,
    call: BrowsingContextCall,
}

/// One storage service call from a renderer.
struct RouteStorageCallOptions<'a> {
    context: &'a ServiceContext,
    assignment: RendererAssignmentId,
    authority: &'a Assignment,
    id: RequestId,
    call: StorageCall,
}

/// One benign reply for a call on a released assignment.
#[derive(Clone, Copy)]
struct SendReleasedReplyOptions<'a> {
    responder: &'a BrowserServiceResponder,
    id: RequestId,
    call: &'a ServiceCall,
}

/// One browser service reply to a renderer.
struct SendReplyOptions<'a> {
    responder: &'a BrowserServiceResponder,
    id: RequestId,
    reply: ServiceReply,
}

/// One renderer teardown: mark dead, fail the router, signal kill.
#[derive(Clone, Copy)]
struct FailOptions<'a> {
    alive: &'a Arc<AtomicBool>,
    router: &'a RendererRouter,
    kill: &'a watch::Sender<bool>,
}

/// One renderer event-subscription request.
#[derive(Clone, Copy)]
struct SubscribeRendererEventsOptions<'a> {
    partition: &'a PartitionServices,
    client: &'a RendererClient,
    kill: &'a watch::Sender<bool>,
}

/// One renderer process spawn.
pub(crate) struct SpawnProcessOptions {
    /// Renderer identity to spawn.
    pub(crate) id: RendererId,
    /// Site lock for the new process, if any.
    pub(crate) site: Option<Site>,
    /// Partition services shared with the new process.
    pub(crate) partition: PartitionServices,
    /// Session storage owned by the browser.
    pub(crate) sessions: Arc<SessionStorage>,
    /// Browser command handle for `window.open` callbacks.
    pub(crate) browser: crate::browser::BrowserHandle,
    /// Assignment registry shared by every renderer of this browser context.
    pub(crate) registry: Arc<AssignmentRegistry>,
    /// Process budget slot held for the renderer's life.
    pub(crate) slot: tokio::sync::OwnedSemaphorePermit,
}

/// One local-storage broadcast to same-origin documents.
#[derive(Clone, Copy)]
struct BroadcastLocalStorageEventOptions<'a> {
    context: &'a ServiceContext,
    assignment: RendererAssignmentId,
    source: FrameId,
    origin: &'a str,
    url: &'a str,
    change: &'a StorageChange,
}

async fn writer_task(options: WriterTaskOptions) {
    let WriterTaskOptions {
        mut rx,
        mut writer,
        alive,
        router,
        kill,
        mut kill_rx,
    } = options;
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
            fail(FailOptions {
                alive: &alive,
                router: &router,
                kill: &kill,
            });
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
                crate::wire::channel::WriteFrameOptions {
                    kind: crate::wire::channel::FrameKind::RequestChunk,
                    request: id.get(),
                    payload: bytes,
                },
            )
            .await
        }
        Frame::ResponseChunk { id, bytes } => {
            crate::wire::channel::write_frame_async(
                writer,
                crate::wire::channel::WriteFrameOptions {
                    kind: crate::wire::channel::FrameKind::ResponseChunk,
                    request: id.get(),
                    payload: bytes,
                },
            )
            .await
        }
        _ => crate::wire::channel::write_control_async(writer, message).await,
    }
}

async fn reader_task(options: ReaderTaskOptions) {
    let ReaderTaskOptions {
        mut reader,
        context,
        ready,
    } = options;
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
    fail(FailOptions {
        alive: &context.alive,
        router: &context.router,
        kill: &context.kill,
    });
}

/// Runs browser services and renderer notifications after the exchange router
/// has separated them from replies and stream chunks.
async fn service_task(mut server: BrowserServiceServer, context: ServiceContext) {
    while let Some(input) = server.recv().await {
        let result = match input {
            ServerInput::Call {
                id,
                body: BrowserCall { assignment, call },
            } => {
                route_service_call(RouteServiceCallOptions {
                    context: &context,
                    id,
                    assignment,
                    call,
                })
                .await
            }
            ServerInput::Notify(RendererNotice::Event {
                assignment,
                frame,
                event,
            }) => route_event(RouteEventOptions {
                context: &context,
                assignment,
                frame,
                event,
            }),
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

fn route_event(options: RouteEventOptions<'_>) -> Result<(), RendererViolation> {
    let RouteEventOptions {
        context,
        assignment,
        frame,
        event,
    } = options;
    let Some(assignment) = context.registry.resolve(assignment) else {
        return if context.registry.was_released(assignment) {
            Ok(())
        } else {
            Err(RendererViolation)
        };
    };
    if assignment.publish_event(frame, event) {
        Err(RendererViolation)
    } else {
        Ok(())
    }
}

/// Routes one browser service call from a renderer. This is a dispatch table:
/// each arm validates and forwards one call shape.
#[expect(
    clippy::too_many_lines,
    reason = "dispatch table over ServiceCall; each arm is one validation plus one forward"
)]
async fn route_service_call(options: RouteServiceCallOptions<'_>) -> Result<(), RendererViolation> {
    let RouteServiceCallOptions {
        context,
        id,
        assignment: assignment_id,
        call,
    } = options;
    let Some(assignment) = context.registry.resolve(assignment_id) else {
        // The renderer may have queued the call before it processed
        // `Release`. Answer with a benign failure so a synchronous
        // `document.cookie` caller cannot block forever, and keep the
        // process alive for its other assignments.
        if context.registry.was_released(assignment_id) {
            return send_released_reply(SendReleasedReplyOptions {
                responder: &context.responder,
                id,
                call: &call,
            })
            .await;
        }
        return Err(RendererViolation);
    };
    match call {
        ServiceCall::Network(NetworkCall::Dial(request)) => {
            let Some(initiator) = assignment.site.authorize(&request.initiator) else {
                return Err(RendererViolation);
            };
            let worker_network = assignment.network.clone();
            let responder = context.responder.clone();
            let worker_kill = context.kill.clone();
            let cancel = context.kill.subscribe();
            tokio::spawn(async move {
                let outcome = worker_network
                    .dial_request(crate::network::DialRequestOptions {
                        request: &request,
                        initiator: &initiator,
                        cancel,
                    })
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
            let Some(url) = assignment.site.authorize(&url) else {
                return Err(RendererViolation);
            };
            let reply = ServiceReply::Cookie(assignment.network.cookies_for(&url));
            send_reply(SendReplyOptions {
                responder: &context.responder,
                id,
                reply,
            })
            .await?;
        }
        ServiceCall::Network(NetworkCall::CookieSet { value, url }) => {
            let Some(url) = assignment.site.authorize(&url) else {
                return Err(RendererViolation);
            };
            assignment.network.set_cookie(&value, &url);
            send_reply(SendReplyOptions {
                responder: &context.responder,
                id,
                reply: ServiceReply::Unit,
            })
            .await?;
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
                    source: (assignment_id, channel),
                }));
            send_reply(SendReplyOptions {
                responder: &context.responder,
                id,
                reply: ServiceReply::Unit,
            })
            .await?;
        }
        ServiceCall::Storage(call) => {
            return route_storage_call(RouteStorageCallOptions {
                context,
                assignment: assignment_id,
                authority: &assignment,
                id,
                call,
            })
            .await;
        }
        ServiceCall::BrowsingContext(call) => {
            return route_window_call(RouteWindowCallOptions {
                context,
                assignment: &assignment,
                id,
                call,
            })
            .await;
        }
    }
    Ok(())
}

/// Routes one `window.open`/`window.close`/`postMessage` service call. Step 1
/// created the tab and its first navigation; step 2 adds the opener link and
/// cross-tab messaging. This is a dispatch table over `BrowsingContextCall`.
#[expect(
    clippy::too_many_lines,
    reason = "dispatch table over BrowsingContextCall; each arm is one validation plus one forward"
)]
async fn route_window_call(options: RouteWindowCallOptions<'_>) -> Result<(), RendererViolation> {
    let RouteWindowCallOptions {
        context,
        assignment,
        id,
        call,
    } = options;
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
                    .open_window(crate::browser::OpenWindowOptions {
                        url: spec,
                        source,
                        noopener,
                    })
                    .await
                    .ok()
                    .map(TabId::get),
                None => None,
            };
            send_reply(SendReplyOptions {
                responder: &context.responder,
                id,
                reply: ServiceReply::Window(tab),
            })
            .await?;
        }
        BrowsingContextCall::WindowClose { tab } => {
            let _result = context.browser.close_tab(TabId::new(tab)).await;
            send_reply(SendReplyOptions {
                responder: &context.responder,
                id,
                reply: ServiceReply::Unit,
            })
            .await?;
        }
        BrowsingContextCall::Opener => {
            let opener = context
                .browser
                .opener_tab(assignment.tab)
                .await
                .ok()
                .flatten()
                .map(TabId::get);
            send_reply(SendReplyOptions {
                responder: &context.responder,
                id,
                reply: ServiceReply::Window(opener),
            })
            .await?;
        }
        BrowsingContextCall::WindowMessage { tab, payload } => {
            let _result = context
                .browser
                .window_message(TabId::new(tab), payload)
                .await;
            send_reply(SendReplyOptions {
                responder: &context.responder,
                id,
                reply: ServiceReply::Unit,
            })
            .await?;
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
                .then(|| {
                    context.sessions.get(SessionKeyOptions {
                        tab: target,
                        origin: &origin,
                        key: &key,
                    })
                })
                .flatten();
            send_reply(SendReplyOptions {
                responder: &context.responder,
                id,
                reply: ServiceReply::StorageValue(value),
            })
            .await?;
        }
    }
    Ok(())
}

struct StorageRouter<'a> {
    context: &'a ServiceContext,
    assignment: RendererAssignmentId,
    authority: &'a Assignment,
    request: RequestId,
}

/// One `Storage.getItem(key)` routed to the owning area.
#[derive(Clone, Copy)]
struct StorageGetOptions<'a> {
    kind: StorageKind,
    origin: &'a str,
    key: &'a str,
}

/// One `Storage.setItem(key, value)` routed to the owning area.
#[derive(Clone, Copy)]
struct StorageSetOptions<'a> {
    kind: StorageKind,
    origin: &'a str,
    url: &'a str,
    key: &'a str,
    value: &'a str,
    source: FrameId,
}

/// One `Storage.removeItem(key)` routed to the owning area.
#[derive(Clone, Copy)]
struct StorageRemoveOptions<'a> {
    kind: StorageKind,
    origin: &'a str,
    url: &'a str,
    key: &'a str,
    source: FrameId,
}

/// One `Storage.clear()` routed to the owning area.
#[derive(Clone, Copy)]
struct StorageClearOptions<'a> {
    kind: StorageKind,
    origin: &'a str,
    url: &'a str,
    source: FrameId,
}

/// One storage mutation announced to same-origin documents.
#[derive(Clone, Copy)]
struct StoragePublishOptions<'a> {
    source: FrameId,
    kind: StorageKind,
    origin: &'a str,
    url: &'a str,
    change: Option<&'a StorageChange>,
}

impl StorageRouter<'_> {
    async fn route(&self, call: StorageCall) -> Result<(), RendererViolation> {
        match call {
            StorageCall::Get { kind, origin, key } => {
                self.get(StorageGetOptions {
                    kind,
                    origin: &origin,
                    key: &key,
                })
                .await
            }
            StorageCall::Keys { kind, origin } => self.keys(kind, &origin).await,
            StorageCall::Set {
                kind,
                origin,
                url,
                key,
                value,
                source,
            } => {
                self.set(StorageSetOptions {
                    kind,
                    origin: &origin,
                    url: &url,
                    key: &key,
                    value: &value,
                    source,
                })
                .await
            }
            StorageCall::Remove {
                kind,
                origin,
                url,
                key,
                source,
            } => {
                self.remove(StorageRemoveOptions {
                    kind,
                    origin: &origin,
                    url: &url,
                    key: &key,
                    source,
                })
                .await
            }
            StorageCall::Clear {
                kind,
                origin,
                url,
                source,
            } => {
                self.clear(StorageClearOptions {
                    kind,
                    origin: &origin,
                    url: &url,
                    source,
                })
                .await
            }
        }
    }

    async fn get(&self, read: StorageGetOptions<'_>) -> Result<(), RendererViolation> {
        let StorageGetOptions { kind, origin, key } = read;
        self.authorize(origin, None)?;
        let value = match kind {
            StorageKind::Local => self.context.partition.local_storage.get(origin, key),
            StorageKind::Session => self.context.sessions.get(SessionKeyOptions {
                tab: self.authority.tab,
                origin,
                key,
            }),
        };
        send_reply(SendReplyOptions {
            responder: &self.context.responder,
            id: self.request,
            reply: ServiceReply::StorageValue(value),
        })
        .await
    }

    async fn keys(&self, kind: StorageKind, origin: &str) -> Result<(), RendererViolation> {
        self.authorize(origin, None)?;
        let keys = match kind {
            StorageKind::Local => self.context.partition.local_storage.keys(origin),
            StorageKind::Session => self.context.sessions.keys(self.authority.tab, origin),
        };
        send_reply(SendReplyOptions {
            responder: &self.context.responder,
            id: self.request,
            reply: ServiceReply::StorageKeys(keys),
        })
        .await
    }

    async fn set(&self, write: StorageSetOptions<'_>) -> Result<(), RendererViolation> {
        let StorageSetOptions {
            kind,
            origin,
            url,
            key,
            value,
            source,
        } = write;
        self.authorize(origin, Some(url))?;
        let change = match kind {
            StorageKind::Local => {
                self.context
                    .partition
                    .local_storage
                    .set(LocalSetOptions { origin, key, value })
            }
            StorageKind::Session => self.context.sessions.set(SessionSetOptions {
                tab: self.authority.tab,
                origin,
                key,
                value,
            }),
        };
        self.publish(StoragePublishOptions {
            source,
            kind,
            origin,
            url,
            change: change.as_ref().ok().and_then(Option::as_ref),
        });
        send_reply(SendReplyOptions {
            responder: &self.context.responder,
            id: self.request,
            reply: ServiceReply::StorageChanged(change),
        })
        .await
    }

    async fn remove(&self, removal: StorageRemoveOptions<'_>) -> Result<(), RendererViolation> {
        let StorageRemoveOptions {
            kind,
            origin,
            url,
            key,
            source,
        } = removal;
        self.authorize(origin, Some(url))?;
        let change = match kind {
            StorageKind::Local => self.context.partition.local_storage.remove(origin, key),
            StorageKind::Session => self.context.sessions.remove(SessionKeyOptions {
                tab: self.authority.tab,
                origin,
                key,
            }),
        };
        self.publish(StoragePublishOptions {
            source,
            kind,
            origin,
            url,
            change: change.as_ref(),
        });
        send_reply(SendReplyOptions {
            responder: &self.context.responder,
            id: self.request,
            reply: ServiceReply::StorageChanged(Ok(change)),
        })
        .await
    }

    async fn clear(&self, clear: StorageClearOptions<'_>) -> Result<(), RendererViolation> {
        let StorageClearOptions {
            kind,
            origin,
            url,
            source,
        } = clear;
        self.authorize(origin, Some(url))?;
        let change = match kind {
            StorageKind::Local => self.context.partition.local_storage.clear(origin),
            StorageKind::Session => self.context.sessions.clear(self.authority.tab, origin),
        };
        self.publish(StoragePublishOptions {
            source,
            kind,
            origin,
            url,
            change: change.as_ref(),
        });
        send_reply(SendReplyOptions {
            responder: &self.context.responder,
            id: self.request,
            reply: ServiceReply::StorageChanged(Ok(change)),
        })
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

    fn publish(&self, publication: StoragePublishOptions<'_>) {
        let StoragePublishOptions {
            source,
            kind,
            origin,
            url,
            change,
        } = publication;
        let Some(change) = change else {
            return;
        };
        if kind == StorageKind::Local {
            broadcast_local_storage_event(BroadcastLocalStorageEventOptions {
                context: self.context,
                assignment: self.assignment,
                source,
                origin,
                url,
                change,
            });
        }
    }
}

async fn route_storage_call(options: RouteStorageCallOptions<'_>) -> Result<(), RendererViolation> {
    let RouteStorageCallOptions {
        context,
        assignment,
        authority,
        id,
        call,
    } = options;
    StorageRouter {
        context,
        assignment,
        authority,
        request: id,
    }
    .route(call)
    .await
}

fn broadcast_local_storage_event(options: BroadcastLocalStorageEventOptions<'_>) {
    let BroadcastLocalStorageEventOptions {
        context,
        assignment,
        source,
        origin,
        url,
        change,
    } = options;
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

/// Answers a late call from a released assignment so the renderer's blocking
/// service path stays unblocked while its engine is torn down.
async fn send_released_reply(
    options: SendReleasedReplyOptions<'_>,
) -> Result<(), RendererViolation> {
    let SendReleasedReplyOptions {
        responder,
        id,
        call,
    } = options;
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
    send_reply(SendReplyOptions {
        responder,
        id,
        reply,
    })
    .await
}

async fn send_reply(options: SendReplyOptions<'_>) -> Result<(), RendererViolation> {
    let SendReplyOptions {
        responder,
        id,
        reply,
    } = options;
    responder
        .reply(id, reply)
        .await
        .map_err(|_| RendererViolation)
}

fn fail(options: FailOptions<'_>) {
    let FailOptions {
        alive,
        router,
        kill,
    } = options;
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
fn subscribe_renderer_events(options: SubscribeRendererEventsOptions<'_>) {
    let SubscribeRendererEventsOptions {
        partition,
        client,
        kill,
    } = options;
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

pub(crate) async fn spawn_process(options: SpawnProcessOptions) -> io::Result<RendererHandle> {
    let SpawnProcessOptions {
        id,
        site,
        partition,
        sessions,
        browser,
        registry,
        slot,
    } = options;
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
    subscribe_renderer_events(SubscribeRendererEventsOptions {
        partition: &partition,
        client: &client,
        kill: &kill,
    });
    let alive = Arc::new(AtomicBool::new(true));
    let (ready_tx, ready_rx) = oneshot::channel();
    let writer_task = tokio::spawn(writer_task(WriterTaskOptions {
        rx,
        writer,
        alive: Arc::clone(&alive),
        router: router.clone(),
        kill: kill.clone(),
        kill_rx: kill_rx.clone(),
    }));
    let site = Arc::new(Mutex::new(site));
    let reader_context = ReaderContext {
        router: router.clone(),
        alive: Arc::clone(&alive),
        kill: kill.clone(),
    };
    let service_context = ServiceContext {
        responder: server.responder(),
        registry,
        partition,
        sessions,
        kill: kill.clone(),
        browser,
    };
    let reader_task = tokio::spawn(reader_task(ReaderTaskOptions {
        reader,
        context: reader_context,
        ready: Some(ready_tx),
    }));
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
        kill,
        tasks: Mutex::new(tasks),
        site,
        assignments: AtomicUsize::new(0),
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
