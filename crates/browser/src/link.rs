//! Browser side of the renderer seam: factory, handles, and async routing.
//!
//! [ADR 0019](../../../docs/adrs/0019-async-browser-runtime-and-io.md): one
//! private platform channel per renderer, length-prefixed frames, async reader
//! and writer tasks, and oneshot replies. The handle stays value-only.

use std::collections::HashMap;
use std::io;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::process::Child;

use crate::manager::RendererId;
use crate::network::FetchHandle;
use crate::site::Site;
use renderer::{
    Command as RendererCommand, FrameId, FromRenderer, Mount, RendererAssignmentId, Reply,
    ResponseStart, ServiceCall, ServiceReply, TabError, TabEvent, ToRenderer,
};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::timeout;

/// Upper bound on one request to a renderer. The renderer budget is seconds;
/// this is a last-resort wake-up if its reply path dies silently.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// How long a `renderer` child has to say [`FromRenderer::Ready`].
pub(crate) const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// How long teardown waits for transport tasks to finish.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

/// Bounded renderer-pump handoff to its owning tab coordinator. Saturation is
/// a renderer protocol violation: dropping lifecycle events would corrupt tab
/// state, while blocking the reader could strand a reply behind those events.
const EVENT_SUBSCRIBER_CAPACITY: usize = 4096;

/// Bounded browser-to-renderer command queue.
pub(crate) const COMMAND_CAPACITY: usize = 256;

/// Live subscribers to one renderer's frame-tagged document events.
type EventSubscribers =
    Arc<Mutex<HashMap<RendererAssignmentId, Vec<mpsc::Sender<(FrameId, TabEvent)>>>>>;

pub(crate) struct PendingReply {
    assignment: RendererAssignmentId,
    reply: oneshot::Sender<Reply>,
}

pub(crate) enum Outbound {
    Control(ToRenderer),
    Body { request: u64, payload: Vec<u8> },
}

/// Value-only handle to one renderer.
pub(crate) struct RendererHandle {
    tx: mpsc::Sender<Outbound>,
    pending: Arc<Mutex<HashMap<u64, PendingReply>>>,
    pub(crate) alive: Arc<AtomicBool>,
    subscribers: EventSubscribers,
    next_request: AtomicU64,
    kill: watch::Sender<bool>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    pub(crate) site: Arc<Mutex<Option<Site>>>,
    pub(crate) assignments: AtomicUsize,
    /// Highest released assignment id. Assignment ids increase, so any
    /// unassigned id at or below this watermark was released, not forged.
    released: Arc<AtomicU64>,
    _slot: tokio::sync::OwnedSemaphorePermit,
}

/// One browser-authorized top-level document inside a renderer process.
pub(crate) struct RendererAssignment {
    pub(crate) id: RendererAssignmentId,
    pub(crate) process: Arc<RendererHandle>,
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
    pub(crate) fn subscribe(&self) -> mpsc::Receiver<(FrameId, TabEvent)> {
        self.process.subscribe(self.id)
    }
}

pub(crate) struct ResponseWriter {
    id: u64,
    tx: mpsc::Sender<Outbound>,
    pending: Arc<Mutex<HashMap<u64, PendingReply>>>,
    reply: Option<oneshot::Receiver<Reply>>,
}

impl ResponseWriter {
    pub(crate) async fn write(&self, payload: Vec<u8>) -> Result<(), TabError> {
        if payload.len() > renderer::MAX_BODY_CHUNK_BYTES {
            return Err(TabError::RendererUnavailable {
                message: "response chunk exceeds IPC limit".into(),
            });
        }
        self.tx
            .send(Outbound::Body {
                request: self.id,
                payload,
            })
            .await
            .map_err(|_| TabError::ActorStopped)
    }

    pub(crate) async fn finish(self) -> Result<Reply, TabError> {
        let id = self.id;
        self.terminate(ToRenderer::ResponseEnd { id }).await
    }

    pub(crate) async fn abort(self, failure: renderer::DialFailure) -> Result<Reply, TabError> {
        let id = self.id;
        self.terminate(ToRenderer::ResponseError { id, failure })
            .await
    }

    async fn terminate(mut self, message: ToRenderer) -> Result<Reply, TabError> {
        if self.tx.send(Outbound::Control(message)).await.is_err() {
            return Err(TabError::ActorStopped);
        }
        let Some(reply) = self.reply.take() else {
            return Err(TabError::ActorStopped);
        };
        if let Ok(Ok(reply)) = timeout(REQUEST_TIMEOUT, reply).await {
            Ok(reply)
        } else {
            self.pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&self.id);
            Err(TabError::ActorStopped)
        }
    }
}

impl Drop for ResponseWriter {
    fn drop(&mut self) {
        if self.reply.is_none() {
            return;
        }
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.id);
        let _ = self
            .tx
            .try_send(Outbound::Control(ToRenderer::ResponseError {
                id: self.id,
                failure: renderer::DialFailure::Cancelled,
            }));
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

    pub(crate) async fn assign(&self, assignment: RendererAssignmentId) -> io::Result<()> {
        self.tx
            .send(Outbound::Control(ToRenderer::Assign { assignment }))
            .await
            .map_err(|_| io::Error::other("renderer stopped"))
    }

    pub(crate) async fn release(&self, assignment: RendererAssignmentId) {
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&assignment);
        self.released.fetch_max(assignment.get(), Ordering::Relaxed);
        let _ = self
            .tx
            .send(Outbound::Control(ToRenderer::Release { assignment }))
            .await;
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
        let id = self.next_request.fetch_add(1, Ordering::Relaxed);
        let (reply_tx, reply_rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
            if !self.alive.load(Ordering::Relaxed) {
                return Err(TabError::ActorStopped);
            }
            pending.insert(
                id,
                PendingReply {
                    assignment,
                    reply: reply_tx,
                },
            );
        }
        if self
            .tx
            .send(Outbound::Control(ToRenderer::Request {
                id,
                assignment,
                command,
            }))
            .await
            .is_err()
        {
            self.remove_pending(id);
            return Err(TabError::ActorStopped);
        }
        if let Ok(Ok(reply)) = timeout(REQUEST_TIMEOUT, reply_rx).await {
            Ok(reply)
        } else {
            self.remove_pending(id);
            Err(TabError::ActorStopped)
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
        for chunk in mount.body.chunks(renderer::MAX_BODY_CHUNK_BYTES) {
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
        let id = self.next_request.fetch_add(1, Ordering::Relaxed);
        let (reply_tx, reply_rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
            if !self.alive.load(Ordering::Relaxed) {
                return Err(TabError::ActorStopped);
            }
            pending.insert(
                id,
                PendingReply {
                    assignment,
                    reply: reply_tx,
                },
            );
        }
        let start = ResponseStart {
            assignment,
            frame,
            status,
            final_url: mount.url.clone(),
            content_type: mount.content_type.clone(),
            content_language: mount.content_language.clone(),
        };
        if self
            .tx
            .send(Outbound::Control(ToRenderer::ResponseStart {
                id,
                response: start,
            }))
            .await
            .is_err()
        {
            self.remove_pending(id);
            return Err(TabError::ActorStopped);
        }
        Ok(ResponseWriter {
            id,
            tx: self.tx.clone(),
            pending: Arc::clone(&self.pending),
            reply: Some(reply_rx),
        })
    }
    /// Subscribes to renderer document events after this call.
    #[must_use]
    fn subscribe(&self, assignment: RendererAssignmentId) -> mpsc::Receiver<(FrameId, TabEvent)> {
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
        let _ = self.tx.try_send(Outbound::Control(ToRenderer::Request {
            id: 0,
            assignment: RendererAssignmentId::new(0),
            command: RendererCommand::Shutdown,
        }));
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

    fn remove_pending(&self, id: u64) {
        let _removed = self
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
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

pub(crate) struct ReaderContext {
    tx: mpsc::Sender<Outbound>,
    pending: Arc<Mutex<HashMap<u64, PendingReply>>>,
    pub(crate) alive: Arc<AtomicBool>,
    subscribers: EventSubscribers,
    fetch: FetchHandle,
    site: Arc<Mutex<Option<Site>>>,
    released: Arc<AtomicU64>,
    kill: watch::Sender<bool>,
}

pub(crate) async fn writer_task(
    mut rx: mpsc::Receiver<Outbound>,
    mut writer: Box<dyn AsyncWrite + Send + Unpin>,
    alive: Arc<AtomicBool>,
    pending: Arc<Mutex<HashMap<u64, PendingReply>>>,
    kill: watch::Sender<bool>,
    mut kill_rx: watch::Receiver<bool>,
) {
    loop {
        let message = tokio::select! {
            message = rx.recv() => match message {
                Some(message) => message,
                None => return,
            },
            _ = kill_rx.changed() => return,
        };
        let result = match message {
            Outbound::Control(message) => {
                renderer::write_control_async(&mut *writer, &message).await
            }
            Outbound::Body { request, payload } => {
                renderer::write_body_async(&mut *writer, request, &payload).await
            }
        };
        if result.is_err() {
            fail(&alive, &pending, &kill);
            return;
        }
    }
}

pub(crate) async fn reader_task(
    mut reader: Box<dyn AsyncRead + Send + Unpin>,
    context: ReaderContext,
    ready: Option<oneshot::Sender<bool>>,
) {
    let mut ready = ready;
    let mut buffer = Vec::new();
    loop {
        let message = match renderer::read_control_async::<FromRenderer, _>(
            &mut *reader,
            &mut buffer,
        )
        .await
        {
            Ok(Some(message)) => message,
            Ok(None) => break,
            Err(error) => {
                logging::error!(target: "browser::link", "bad renderer message: {error}");
                break;
            }
        };
        if let Some(ready_tx) = ready.take() {
            let ok = matches!(message, FromRenderer::Ready);
            let _ = ready_tx.send(ok);
            if !ok {
                break;
            }
            continue;
        }
        if route(message, &context).await.is_err() {
            break;
        }
    }
    if let Some(ready_tx) = ready {
        let _ = ready_tx.send(false);
    }
    fail(&context.alive, &context.pending, &context.kill);
}

/// Routes one renderer message; a failure terminates the renderer.
async fn route(message: FromRenderer, context: &ReaderContext) -> Result<(), RendererViolation> {
    match message {
        // Handled by the channel handshake; never routed.
        FromRenderer::Ready => {}
        FromRenderer::Reply {
            id,
            assignment,
            reply,
        } => {
            if let Some(pending) = context
                .pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&id)
            {
                if pending.assignment != assignment {
                    return Err(RendererViolation);
                }
                let _ = pending.reply.send(reply);
            }
        }
        FromRenderer::Event {
            assignment,
            frame,
            event,
        } => {
            let mut saturated = false;
            let mut subscribers = context
                .subscribers
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let Some(assignment_subscribers) = subscribers.get_mut(&assignment) else {
                // A released assignment still had messages in flight when the
                // renderer processed `Release`; drop them instead of failing
                // the process, which may host other assignments.
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
                return Err(RendererViolation);
            }
        }
        FromRenderer::ServiceCall {
            assignment,
            id,
            call,
        } => route_service_call(context, assignment, id, call).await?,
    }
    Ok(())
}

async fn route_service_call(
    context: &ReaderContext,
    assignment: RendererAssignmentId,
    id: u64,
    call: ServiceCall,
) -> Result<(), RendererViolation> {
    if !has_assignment(&context.subscribers, assignment) {
        // The renderer may have queued the call before it processed
        // `Release`. Answer with a benign failure so a synchronous
        // `document.cookie` caller cannot block forever, and keep the
        // process alive for its other assignments.
        if was_released(&context.released, assignment) {
            return send_released_reply(&context.tx, id, &call).await;
        }
        return Err(RendererViolation);
    }
    match call {
        ServiceCall::Dial(request) => {
            let Some(initiator) = authorize(&context.site, &request.initiator) else {
                return Err(RendererViolation);
            };
            let worker_fetch = context.fetch.clone();
            let worker_tx = context.tx.clone();
            let worker_pending = Arc::clone(&context.pending);
            let worker_alive = Arc::clone(&context.alive);
            let worker_kill = context.kill.clone();
            let cancel = context.kill.subscribe();
            tokio::spawn(async move {
                let outcome = worker_fetch
                    .dial_request(&request, &initiator, cancel)
                    .await;
                if worker_tx
                    .try_send(Outbound::Control(ToRenderer::ServiceReply {
                        id,
                        reply: ServiceReply::Dial(outcome),
                    }))
                    .is_err()
                {
                    fail(&worker_alive, &worker_pending, &worker_kill);
                }
            });
        }
        ServiceCall::CookieGet { url } => {
            let Some(url) = authorize(&context.site, &url) else {
                return Err(RendererViolation);
            };
            let reply = ServiceReply::Cookie(context.fetch.cookies_for(&url));
            send_reply(&context.tx, id, reply).await?;
        }
        ServiceCall::CookieSet { value, url } => {
            let Some(url) = authorize(&context.site, &url) else {
                return Err(RendererViolation);
            };
            context.fetch.set_cookie(&value, &url);
            send_reply(&context.tx, id, ServiceReply::Unit).await?;
        }
    }
    Ok(())
}

fn was_released(released: &AtomicU64, assignment: RendererAssignmentId) -> bool {
    assignment.get() <= released.load(Ordering::Relaxed)
}

/// Answers a late call from a released assignment so the renderer's blocking
/// service path stays unblocked while its engine is torn down.
async fn send_released_reply(
    tx: &mpsc::Sender<Outbound>,
    id: u64,
    call: &ServiceCall,
) -> Result<(), RendererViolation> {
    let reply = match call {
        ServiceCall::Dial(_) => ServiceReply::Dial(Err(renderer::DialFailure::Cancelled)),
        ServiceCall::CookieGet { .. } => ServiceReply::Cookie(String::new()),
        ServiceCall::CookieSet { .. } => ServiceReply::Unit,
    };
    send_reply(tx, id, reply).await
}

fn has_assignment(subscribers: &EventSubscribers, assignment: RendererAssignmentId) -> bool {
    subscribers
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .contains_key(&assignment)
}

fn authorize(site: &Mutex<Option<Site>>, spec: &str) -> Option<url::Url> {
    site.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|site| site.authorize(spec))
}

async fn send_reply(
    tx: &mpsc::Sender<Outbound>,
    id: u64,
    reply: ServiceReply,
) -> Result<(), RendererViolation> {
    tx.send(Outbound::Control(ToRenderer::ServiceReply { id, reply }))
        .await
        .map_err(|_| RendererViolation)
}

fn fail(
    alive: &Arc<AtomicBool>,
    pending: &Arc<Mutex<HashMap<u64, PendingReply>>>,
    kill: &watch::Sender<bool>,
) {
    alive.store(false, Ordering::Relaxed);
    fail_pending(pending);
    let _ = kill.send(true);
}

fn fail_pending(pending: &Arc<Mutex<HashMap<u64, PendingReply>>>) {
    let mut pending = pending.lock().unwrap_or_else(PoisonError::into_inner);
    pending.clear();
}

/// Forwards one renderer child's stderr into this process's logger.
///
/// The child formats its own level and target; the browser only forwards the
/// lines, so renderer records land in the daemon's console and file without a
/// second file writer ([ADR 0015](../../../docs/adrs/0015-logging.md)).
pub(crate) async fn forward_stderr(stderr: tokio::process::ChildStderr) {
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

pub(crate) async fn spawn_process(
    id: RendererId,
    site: Option<Site>,
    fetch: FetchHandle,
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
    let (tx, rx) = mpsc::channel::<Outbound>(COMMAND_CAPACITY);
    let (kill, kill_rx) = watch::channel(false);
    let pending = Arc::new(Mutex::new(HashMap::new()));
    let alive = Arc::new(AtomicBool::new(true));
    let subscribers = Arc::new(Mutex::new(HashMap::new()));
    let (ready_tx, ready_rx) = oneshot::channel();
    let writer_task = tokio::spawn(writer_task(
        rx,
        writer,
        Arc::clone(&alive),
        Arc::clone(&pending),
        kill.clone(),
        kill_rx.clone(),
    ));
    let site = Arc::new(Mutex::new(site));
    let released = Arc::new(AtomicU64::new(0));
    let reader_context = ReaderContext {
        tx: tx.clone(),
        pending: Arc::clone(&pending),
        alive: Arc::clone(&alive),
        subscribers: Arc::clone(&subscribers),
        fetch: fetch.clone(),
        site: Arc::clone(&site),
        released: Arc::clone(&released),
        kill: kill.clone(),
    };
    let reader_task = tokio::spawn(reader_task(reader, reader_context, Some(ready_tx)));
    let stderr_task = tokio::spawn(forward_stderr(stderr));
    // Detached child task: it reaps the child when the channel closes or the
    // handle signals a kill. Dropping every kill sender also stops it.
    let _reaper = tokio::spawn(child_task(child, kill_rx));
    if tx.send(Outbound::Control(ToRenderer::Hello)).await.is_err()
        || timeout(HANDSHAKE_TIMEOUT, ready_rx).await != Ok(Ok(true))
    {
        let _ = kill.send(true);
        let _result = writer_task.await;
        let _result = reader_task.await;
        let _result = stderr_task.await;
        return Err(io::Error::other("renderer handshake failed"));
    }
    logging::debug!(
        target: "browser::link",
        "renderer {id:?} ready for site lock {site:?}"
    );
    let tasks = vec![writer_task, reader_task, stderr_task];
    Ok(RendererHandle {
        tx,
        pending,
        alive,
        subscribers,
        next_request: AtomicU64::new(1),
        kill,
        tasks: Mutex::new(tasks),
        site,
        assignments: AtomicUsize::new(0),
        released,
        _slot: slot,
    })
}

async fn child_task(mut child: Child, mut kill: watch::Receiver<bool>) {
    tokio::select! {
        _ = kill.changed() => {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        _ = child.wait() => {}
    }
}
