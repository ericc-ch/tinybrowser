//! Browser side of the renderer seam: factory, handles, and async routing.
//!
//! [ADR 0019](../../../docs/adrs/0019-async-browser-runtime-and-io.md): one
//! private platform channel per renderer, length-prefixed frames, async reader
//! and writer tasks, and oneshot replies. The handle stays value-only.

use std::collections::HashMap;
use std::io;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crate::network::FetchHandle;
use crate::site::Site;
use renderer::{
    Command as RendererCommand, FrameId, FromRenderer, Reply, ServiceCall, ServiceReply, TabError,
    TabEvent, ToRenderer,
};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::timeout;

/// Upper bound on one request to a renderer. The renderer budget is seconds;
/// this is a last-resort wake-up if its reply path dies silently.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// How long a `renderer` child has to say [`FromRenderer::Ready`].
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// How long teardown waits for transport tasks to finish.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

/// Bounded renderer-pump handoff to its owning tab coordinator. Saturation is
/// a renderer protocol violation: dropping lifecycle events would corrupt tab
/// state, while blocking the reader could strand a reply behind those events.
const EVENT_SUBSCRIBER_CAPACITY: usize = 4096;

/// Bounded browser-to-renderer command queue.
const COMMAND_CAPACITY: usize = 256;

/// Identity of one live renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct RendererId(u64);

/// Live subscribers to one renderer's frame-tagged document events.
type EventSubscribers = Arc<Mutex<Vec<mpsc::Sender<(FrameId, TabEvent)>>>>;

/// Value-only handle to one renderer.
pub(crate) struct RendererHandle {
    tx: mpsc::Sender<ToRenderer>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Reply>>>>,
    alive: Arc<AtomicBool>,
    subscribers: EventSubscribers,
    next_request: AtomicU64,
    kill: watch::Sender<bool>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl RendererHandle {
    /// Sends one command and waits for its reply.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`] when the renderer is gone or the reply does
    /// not arrive within the request timeout.
    pub(crate) async fn request(&self, command: RendererCommand) -> Result<Reply, TabError> {
        let id = self.next_request.fetch_add(1, Ordering::Relaxed);
        let (reply_tx, reply_rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
            if !self.alive.load(Ordering::Relaxed) {
                return Err(TabError::ActorStopped);
            }
            pending.insert(id, reply_tx);
        }
        if self
            .tx
            .send(ToRenderer::Request { id, command })
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
    /// Subscribes to renderer document events after this call.
    #[must_use]
    pub(crate) fn subscribe(&self) -> mpsc::Receiver<(FrameId, TabEvent)> {
        let (tx, rx) = mpsc::channel(EVENT_SUBSCRIBER_CAPACITY);
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(tx);
        rx
    }

    /// Interrupts a blocked script by killing the renderer process.
    pub(crate) fn interrupt(&self) {
        let _ = self.kill.send(true);
    }

    /// Asks the renderer loop to stop without waiting.
    pub(crate) fn request_shutdown(&self) {
        let _ = self.tx.try_send(ToRenderer::Request {
            id: 0,
            command: RendererCommand::Shutdown,
        });
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

/// Renderer-process factory.
pub(crate) struct RendererFactory {
    fetch: FetchHandle,
    next: AtomicU64,
}

impl RendererFactory {
    pub(crate) fn new(fetch: FetchHandle) -> Self {
        Self {
            fetch,
            next: AtomicU64::new(1),
        }
    }

    /// Creates a renderer locked to `site`.
    ///
    /// # Errors
    ///
    /// Process spawn failure or a failed protocol handshake.
    pub(crate) async fn acquire(&self, site: &Site) -> io::Result<RendererHandle> {
        let id = RendererId(self.next.fetch_add(1, Ordering::Relaxed));
        spawn_process(id, site, self.fetch.clone()).await
    }
}

struct RendererViolation;

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

async fn spawn_process(
    id: RendererId,
    site: &Site,
    fetch: FetchHandle,
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
    let (tx, rx) = mpsc::channel::<ToRenderer>(COMMAND_CAPACITY);
    let (kill, kill_rx) = watch::channel(false);
    let pending = Arc::new(Mutex::new(HashMap::new()));
    let alive = Arc::new(AtomicBool::new(true));
    let subscribers = Arc::new(Mutex::new(Vec::new()));
    let (ready_tx, ready_rx) = oneshot::channel();
    let writer_task = tokio::spawn(writer_task(
        rx,
        writer,
        Arc::clone(&alive),
        Arc::clone(&pending),
        kill.clone(),
        kill_rx.clone(),
    ));
    let reader_context = ReaderContext {
        tx: tx.clone(),
        pending: Arc::clone(&pending),
        alive: Arc::clone(&alive),
        subscribers: Arc::clone(&subscribers),
        fetch: fetch.clone(),
        site: site.clone(),
        kill: kill.clone(),
    };
    let reader_task = tokio::spawn(reader_task(reader, reader_context, Some(ready_tx)));
    let stderr_task = tokio::spawn(forward_stderr(stderr));
    // Detached child task: it reaps the child when the channel closes or the
    // handle signals a kill. Dropping every kill sender also stops it.
    let _reaper = tokio::spawn(child_task(child, kill_rx));
    if tx.send(ToRenderer::Hello).await.is_err()
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
        "renderer {id:?} ready for site {site:?}"
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

/// Shared route state for one renderer's reader task.
struct ReaderContext {
    tx: mpsc::Sender<ToRenderer>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Reply>>>>,
    alive: Arc<AtomicBool>,
    subscribers: EventSubscribers,
    fetch: FetchHandle,
    site: Site,
    kill: watch::Sender<bool>,
}

async fn writer_task(
    mut rx: mpsc::Receiver<ToRenderer>,
    mut writer: Box<dyn AsyncWrite + Send + Unpin>,
    alive: Arc<AtomicBool>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Reply>>>>,
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
        if renderer::write_control_async(&mut *writer, &message)
            .await
            .is_err()
        {
            fail(&alive, &pending, &kill);
            return;
        }
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
        if route(message, &context).is_err() {
            break;
        }
    }
    if let Some(ready_tx) = ready {
        let _ = ready_tx.send(false);
    }
    fail(&context.alive, &context.pending, &context.kill);
}

/// Routes one renderer message; a failure terminates the renderer.
fn route(message: FromRenderer, context: &ReaderContext) -> Result<(), RendererViolation> {
    match message {
        // Handled by the channel handshake; never routed.
        FromRenderer::Ready => {}
        FromRenderer::Reply { id, reply } => {
            if let Some(reply_tx) = context
                .pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&id)
            {
                let _ = reply_tx.send(reply);
            }
        }
        FromRenderer::Event { frame, event } => {
            let mut saturated = false;
            context
                .subscribers
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .retain(|subscriber| match subscriber.try_send((frame, event)) {
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
        FromRenderer::ServiceCall { id, call } => match call {
            ServiceCall::Dial(request) => {
                let Some(initiator) = context.site.authorize(&request.initiator) else {
                    return Err(RendererViolation);
                };
                let worker_fetch = context.fetch.clone();
                let worker_tx = context.tx.clone();
                let worker_pending = Arc::clone(&context.pending);
                let worker_alive = Arc::clone(&context.alive);
                let worker_kill = context.kill.clone();
                tokio::spawn(async move {
                    let outcome = worker_fetch.dial_request(&request, &initiator).await;
                    if worker_tx
                        .try_send(ToRenderer::ServiceReply {
                            id,
                            reply: ServiceReply::Dial(outcome),
                        })
                        .is_err()
                    {
                        fail(&worker_alive, &worker_pending, &worker_kill);
                    }
                });
            }
            ServiceCall::CookieGet { url } => {
                let Some(url) = context.site.authorize(&url) else {
                    return Err(RendererViolation);
                };
                let reply = ServiceReply::Cookie(context.fetch.cookies_for(&url));
                send_reply(&context.tx, id, reply)?;
            }
            ServiceCall::CookieSet { value, url } => {
                let Some(url) = context.site.authorize(&url) else {
                    return Err(RendererViolation);
                };
                context.fetch.set_cookie(&value, &url);
                send_reply(&context.tx, id, ServiceReply::Unit)?;
            }
        },
    }
    Ok(())
}

fn send_reply(
    tx: &mpsc::Sender<ToRenderer>,
    id: u64,
    reply: ServiceReply,
) -> Result<(), RendererViolation> {
    tx.try_send(ToRenderer::ServiceReply { id, reply })
        .map_err(|_| RendererViolation)
}

fn fail(
    alive: &Arc<AtomicBool>,
    pending: &Arc<Mutex<HashMap<u64, oneshot::Sender<Reply>>>>,
    kill: &watch::Sender<bool>,
) {
    alive.store(false, Ordering::Relaxed);
    fail_pending(pending);
    let _ = kill.send(true);
}

fn fail_pending(pending: &Arc<Mutex<HashMap<u64, oneshot::Sender<Reply>>>>) {
    let mut pending = pending.lock().unwrap_or_else(PoisonError::into_inner);
    pending.clear();
}

/// Forwards one renderer child's stderr into this process's logger.
///
/// The child formats its own level and target; the browser only forwards the
/// lines, so renderer records land in the daemon's console and file without a
/// second file writer ([ADR 0015](../../../docs/adrs/0015-logging.md)).
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
