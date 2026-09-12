//! Host side of the renderer seam: factory, handles, and routing pumps.
//!
//! [ADR 0011](../../../docs/adrs/0011-renderer-processes-per-site.md): the
//! handle is value-only; replies, events, and browser-service calls cross a
//! channel pair (local backend) or a pipe (process backend).

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::network::FetchHandle;
use crate::site::Site;
use renderer::{
    BrowserServices, Command as RendererCommand, FrameId, FromRenderer, RENDERER_INBOX_CAPACITY,
    RENDERER_OUTBOX_CAPACITY, Reply, ServiceCall, ServiceReply, Stop, TabError, TabEvent,
    ToRenderer,
};

/// Upper bound on one request to a renderer. The renderer budget is seconds;
/// this is a last-resort wake-up if its reply path dies silently.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// How long a `--renderer` child has to say [`FromRenderer::Ready`].
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// Bounded renderer-pump handoff to its owning tab actor. Saturation is a
/// renderer protocol violation: dropping lifecycle events would corrupt tab
/// state, while blocking the pump could strand a reply behind those events.
const EVENT_SUBSCRIBER_CAPACITY: usize = 4096;

/// Which backend hosts renderer work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Renderers {
    /// In-process renderer threads. Fast tests; no memory isolation.
    Local,
    /// One OS process per site instance, spawned as `--renderer`.
    Process,
}

/// Identity of one live renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct RendererId(u64);

/// Live subscribers to one renderer's frame-tagged document events.
type EventSubscribers = Arc<Mutex<Vec<SyncSender<(FrameId, TabEvent)>>>>;

/// Value-only handle to one renderer.
pub(crate) struct RendererHandle {
    sink: Sink,
    pending: Arc<Mutex<HashMap<u64, Sender<Reply>>>>,
    alive: Arc<AtomicBool>,
    subscribers: EventSubscribers,
    next_request: Arc<AtomicU64>,
    stop: Option<Arc<Stop>>,
    child: Option<Arc<Mutex<Child>>>,
    renderer_join: Option<JoinHandle<()>>,
    pump_join: Option<JoinHandle<()>>,
    stderr_join: Option<JoinHandle<()>>,
}

impl RendererHandle {
    /// Sends one command and waits for its reply.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`] when the renderer is gone.
    pub(crate) fn request(&self, command: RendererCommand) -> Result<Reply, TabError> {
        let id = self.next_request.fetch_add(1, Ordering::Relaxed);
        let (reply_tx, reply_rx) = mpsc::channel();
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        if !self.alive.load(Ordering::Relaxed) {
            return Err(TabError::ActorStopped);
        }
        pending.insert(id, reply_tx);
        drop(pending);
        if self.sink.send(ToRenderer::Request { id, command }).is_err() {
            self.pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&id);
            return Err(TabError::ActorStopped);
        }
        if let Ok(reply) = reply_rx.recv_timeout(REQUEST_TIMEOUT) {
            return Ok(reply);
        }
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
        Err(TabError::ActorStopped)
    }

    /// Subscribes to renderer document events after this call.
    #[must_use]
    pub(crate) fn subscribe(&self) -> Receiver<(FrameId, TabEvent)> {
        let (tx, rx) = mpsc::sync_channel(EVENT_SUBSCRIBER_CAPACITY);
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(tx);
        rx
    }

    /// Interrupts a blocked script now: the local backend flips the shared
    /// stop flag; the process backend kills the child.
    pub(crate) fn interrupt(&self) {
        if let Some(stop) = &self.stop {
            stop.request();
        }
        if let Some(child) = &self.child {
            let _ = child.lock().unwrap_or_else(PoisonError::into_inner).kill();
        }
    }

    /// Asks the renderer loop to stop without joining it.
    pub(crate) fn request_shutdown(&self) {
        if let Some(stop) = &self.stop {
            stop.request();
        }
        let _ = self.sink.shutdown();
    }

    fn shutdown(&mut self) {
        self.request_shutdown();
        self.interrupt();
        if let Some(join) = self.pump_join.take() {
            let _ = join.join();
        }
        if let Some(child) = self.child.take() {
            let mut child = child.lock().unwrap_or_else(PoisonError::into_inner);
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(join) = self.stderr_join.take() {
            let _ = join.join();
        }
        if let Some(join) = self.renderer_join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for RendererHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Backend-selecting renderer factory.
pub(crate) struct RendererFactory {
    backend: Renderers,
    fetch: FetchHandle,
    next: AtomicU64,
}

impl RendererFactory {
    pub(crate) fn new(backend: Renderers, fetch: FetchHandle) -> Self {
        Self {
            backend,
            fetch,
            next: AtomicU64::new(1),
        }
    }

    /// Creates a renderer locked to `site`.
    ///
    /// # Errors
    ///
    /// Process spawn failure.
    pub(crate) fn acquire(&self, site: &Site) -> io::Result<Arc<RendererHandle>> {
        let id = RendererId(self.next.fetch_add(1, Ordering::Relaxed));
        match self.backend {
            Renderers::Local => {
                let services = Arc::new(crate::services::FetchServices::new(
                    self.fetch.clone(),
                    site.clone(),
                ));
                Ok(Arc::new(spawn_local(
                    id,
                    site,
                    services,
                    self.fetch.clone(),
                )))
            }
            Renderers::Process => spawn_process(id, site, self.fetch.clone()).map(Arc::new),
        }
    }
}

#[derive(Clone)]
enum Sink {
    Local(SyncSender<ToRenderer>),
    Pipe(Arc<Mutex<ChildStdin>>),
}

impl Sink {
    fn send(&self, message: ToRenderer) -> io::Result<()> {
        match self {
            Self::Local(tx) => match tx.try_send(message) {
                Ok(()) => Ok(()),
                Err(TrySendError::Full(_)) => Err(io::Error::other("renderer inbox is full")),
                Err(TrySendError::Disconnected(_)) => Err(io::Error::other("renderer stopped")),
            },
            Self::Pipe(stdin) => {
                let line = renderer::encode_ipc_message(&message)?;
                let mut stdin = stdin.lock().unwrap_or_else(PoisonError::into_inner);
                stdin.write_all(&line)?;
                stdin.write_all(b"\n")?;
                stdin.flush()
            }
        }
    }

    fn shutdown(&self) -> io::Result<()> {
        let message = ToRenderer::Request {
            id: 0,
            command: RendererCommand::Shutdown,
        };
        match self {
            Self::Local(tx) => tx
                .send(message)
                .map_err(|_| io::Error::other("renderer stopped")),
            Self::Pipe(_) => self.send(message),
        }
    }
}

enum Incoming {
    Local(Receiver<FromRenderer>),
    Pipe(BufReader<std::process::ChildStdout>),
}

struct RendererViolation;

fn spawn_local(
    id: RendererId,
    site: &Site,
    services: Arc<dyn BrowserServices>,
    fetch: FetchHandle,
) -> RendererHandle {
    let (to_tx, to_rx) = mpsc::sync_channel::<ToRenderer>(RENDERER_INBOX_CAPACITY);
    let (from_tx, from_rx) = mpsc::sync_channel::<FromRenderer>(RENDERER_OUTBOX_CAPACITY);
    let stop = Arc::new(Stop::new());
    let renderer_stop = Arc::clone(&stop);
    let renderer_name = format!("renderer-{id:?}");
    let renderer_join = thread::Builder::new()
        .name(renderer_name)
        .spawn(move || renderer::run_with_stop(&to_rx, &from_tx, services, &renderer_stop))
        .expect("renderer thread");
    let sink = Sink::Local(to_tx);
    let pending = Arc::new(Mutex::new(HashMap::new()));
    let alive = Arc::new(AtomicBool::new(true));
    let subscribers = Arc::new(Mutex::new(Vec::new()));
    let pump_sink = sink.clone();
    let pump_pending = Arc::clone(&pending);
    let pump_alive = Arc::clone(&alive);
    let pump_subscribers = Arc::clone(&subscribers);
    let pump_stop = Arc::clone(&stop);
    let pump_site = site.clone();
    let name = format!("renderer-{id:?}-pump");
    let pump_join = thread::Builder::new()
        .name(name)
        .spawn(move || {
            let context = PumpContext {
                sink: &pump_sink,
                pending: &pump_pending,
                alive: &pump_alive,
                subscribers: &pump_subscribers,
                fetch: &fetch,
                site: &pump_site,
                terminator: RendererTerminator::Local(pump_stop),
            };
            pump_loop(Incoming::Local(from_rx), &context, None);
        })
        .expect("renderer pump thread");
    RendererHandle {
        sink,
        pending,
        alive,
        subscribers,
        next_request: Arc::new(AtomicU64::new(1)),
        stop: Some(stop),
        child: None,
        renderer_join: Some(renderer_join),
        pump_join: Some(pump_join),
        stderr_join: None,
    }
}

fn spawn_process(id: RendererId, site: &Site, fetch: FetchHandle) -> io::Result<RendererHandle> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("--renderer")
        .env("TINYBROWSER_LOG", logging::level().as_str())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let Some(stdin) = child.stdin.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(io::Error::other("renderer stdin missing"));
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(io::Error::other("renderer stdout missing"));
    };
    let Some(stderr) = child.stderr.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(io::Error::other("renderer stderr missing"));
    };
    let child = Arc::new(Mutex::new(child));
    let sink = Sink::Pipe(Arc::new(Mutex::new(stdin)));
    let pending = Arc::new(Mutex::new(HashMap::new()));
    let alive = Arc::new(AtomicBool::new(true));
    let subscribers = Arc::new(Mutex::new(Vec::new()));
    let pump_sink = sink.clone();
    let pump_pending = Arc::clone(&pending);
    let pump_alive = Arc::clone(&alive);
    let pump_subscribers = Arc::clone(&subscribers);
    let pump_child = Arc::clone(&child);
    let pump_site = site.clone();
    let (ready_tx, ready_rx) = mpsc::channel();
    let name = format!("renderer-{id:?}-pump");
    let pump_join = thread::Builder::new()
        .name(name)
        .spawn(move || {
            let context = PumpContext {
                sink: &pump_sink,
                pending: &pump_pending,
                alive: &pump_alive,
                subscribers: &pump_subscribers,
                fetch: &fetch,
                site: &pump_site,
                terminator: RendererTerminator::Process(pump_child),
            };
            pump_loop(
                Incoming::Pipe(BufReader::new(stdout)),
                &context,
                Some(ready_tx),
            );
        })
        .expect("renderer pump thread");
    let stderr_join = thread::Builder::new()
        .name(format!("renderer-{id:?}-stderr"))
        .spawn(move || forward_stderr(stderr))
        .expect("renderer stderr thread");
    if ready_rx.recv_timeout(HANDSHAKE_TIMEOUT) != Ok(true) {
        let mut child = child.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = child.kill();
        let _ = child.wait();
        return Err(io::Error::other("renderer handshake failed"));
    }
    logging::debug!(
        target: "browser::link",
        "renderer {id:?} ready for site {site:?}"
    );
    Ok(RendererHandle {
        sink,
        pending,
        alive,
        subscribers,
        next_request: Arc::new(AtomicU64::new(1)),
        stop: None,
        child: Some(child),
        renderer_join: None,
        pump_join: Some(pump_join),
        stderr_join: Some(stderr_join),
    })
}

struct PumpContext<'a> {
    sink: &'a Sink,
    pending: &'a Arc<Mutex<HashMap<u64, Sender<Reply>>>>,
    alive: &'a Arc<AtomicBool>,
    subscribers: &'a EventSubscribers,
    fetch: &'a FetchHandle,
    site: &'a Site,
    terminator: RendererTerminator,
}

#[derive(Clone)]
enum RendererTerminator {
    Local(Arc<Stop>),
    Process(Arc<Mutex<Child>>),
}

impl RendererTerminator {
    fn terminate(&self, sink: &Sink) {
        match self {
            Self::Local(stop) => {
                stop.request();
                if matches!(sink, Sink::Local(_)) {
                    let _ = sink.shutdown();
                }
            }
            Self::Process(child) => {
                let mut child = child.lock().unwrap_or_else(PoisonError::into_inner);
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

impl PumpContext<'_> {
    fn route(&self, message: FromRenderer) -> Result<(), RendererViolation> {
        match message {
            // Handled by the pipe handshake; never routed.
            FromRenderer::Ready => {}
            FromRenderer::Reply { id, reply } => {
                if let Some(reply_tx) = self
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
                self.subscribers
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .retain(|subscriber| match subscriber.try_send((frame, event)) {
                        Ok(()) => true,
                        Err(TrySendError::Disconnected(_)) => false,
                        Err(TrySendError::Full(_)) => {
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
                    let Some(initiator) = self.site.authorize(&request.initiator) else {
                        return Err(RendererViolation);
                    };
                    let worker_fetch = self.fetch.clone();
                    let worker_sink = self.sink.clone();
                    let worker_terminator = self.terminator.clone();
                    let worker_alive = Arc::clone(self.alive);
                    let submitted = self.fetch.try_submit(move || {
                        let outcome = worker_fetch.dial_request(&request, &initiator);
                        if worker_sink
                            .send(ToRenderer::ServiceReply {
                                id,
                                reply: ServiceReply::Dial(outcome),
                            })
                            .is_err()
                        {
                            worker_alive.store(false, Ordering::Relaxed);
                            worker_terminator.terminate(&worker_sink);
                        }
                    });
                    if submitted.is_err() {
                        self.sink
                            .send(ToRenderer::ServiceReply {
                                id,
                                reply: ServiceReply::Dial(None),
                            })
                            .map_err(|_| RendererViolation)?;
                    }
                }
                ServiceCall::CookieGet { url } => {
                    let Some(url) = self.site.authorize(&url) else {
                        return Err(RendererViolation);
                    };
                    let reply = ServiceReply::Cookie(self.fetch.cookies_for(&url));
                    self.sink
                        .send(ToRenderer::ServiceReply { id, reply })
                        .map_err(|_| RendererViolation)?;
                }
                ServiceCall::CookieSet { value, url } => {
                    let Some(url) = self.site.authorize(&url) else {
                        return Err(RendererViolation);
                    };
                    self.fetch.set_cookie(&value, &url);
                    self.sink
                        .send(ToRenderer::ServiceReply {
                            id,
                            reply: ServiceReply::Unit,
                        })
                        .map_err(|_| RendererViolation)?;
                }
            },
        }
        Ok(())
    }
}

/// Pumps one renderer child's stderr into this process's logger.
///
/// The child formats its own level and target; the browser only forwards the
/// lines, so renderer records land in the daemon's console and file without a
/// second file writer ([ADR 0015](../../../docs/adrs/0015-logging.md)).
fn forward_stderr(stderr: std::process::ChildStderr) {
    let mut reader = BufReader::new(stderr);
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        match reader.read_until(b'\n', &mut bytes) {
            Ok(0) | Err(_) => break,
            Ok(_) => logging::log_forwarded(&String::from_utf8_lossy(&bytes)),
        }
    }
}

fn pump_loop(incoming: Incoming, context: &PumpContext<'_>, ready: Option<Sender<bool>>) {
    let mut ready = ready;
    match incoming {
        Incoming::Local(receiver) => {
            while let Ok(message) = receiver.recv() {
                if context.route(message).is_err() {
                    break;
                }
            }
        }
        Incoming::Pipe(mut reader) => {
            let mut buffer = Vec::new();
            loop {
                let message = match renderer::read_ipc_message(&mut reader, &mut buffer) {
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
                if context.route(message).is_err() {
                    break;
                }
            }
        }
    }
    if let Some(ready_tx) = ready {
        let _ = ready_tx.send(false);
    }
    // A dead renderer must not strand callers blocked in `request`.
    let mut pending = context
        .pending
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    context.alive.store(false, Ordering::Relaxed);
    pending.clear();
    drop(pending);
    context.terminator.terminate(context.sink);
}
