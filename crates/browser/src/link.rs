//! Browser side of the renderer seam: factory, handles, and routing pumps.
//!
//! [ADR 0011](../../../docs/adrs/0011-renderer-processes-per-site.md): the
//! handle is value-only; replies, events, and browser-service calls cross a
//! pipe.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::network::FetchHandle;
use crate::site::Site;
use renderer::{
    Command as RendererCommand, FrameId, FromRenderer, Reply, ServiceCall, ServiceReply, TabError,
    TabEvent, ToRenderer,
};
use tokio::sync::mpsc as async_mpsc;

/// Upper bound on one request to a renderer. The renderer budget is seconds;
/// this is a last-resort wake-up if its reply path dies silently.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// How long a `renderer` child has to say [`FromRenderer::Ready`].
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// Bounded renderer-pump handoff to its owning tab coordinator. Saturation is a
/// renderer protocol violation: dropping lifecycle events would corrupt tab
/// state, while blocking the pump could strand a reply behind those events.
const EVENT_SUBSCRIBER_CAPACITY: usize = 4096;

/// Identity of one live renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct RendererId(u64);

/// Live subscribers to one renderer's frame-tagged document events.
type EventSubscribers = Arc<Mutex<Vec<async_mpsc::Sender<(FrameId, TabEvent)>>>>;

/// Value-only handle to one renderer.
pub(crate) struct RendererHandle {
    sink: Sink,
    pending: Arc<Mutex<HashMap<u64, Sender<Reply>>>>,
    alive: Arc<AtomicBool>,
    subscribers: EventSubscribers,
    next_request: Arc<AtomicU64>,
    child: Arc<Mutex<Child>>,
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
        if self
            .sink
            .send(&ToRenderer::Request { id, command })
            .is_err()
        {
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
    pub(crate) fn subscribe(&self) -> async_mpsc::Receiver<(FrameId, TabEvent)> {
        let (tx, rx) = async_mpsc::channel(EVENT_SUBSCRIBER_CAPACITY);
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(tx);
        rx
    }

    /// Interrupts a blocked script by killing the renderer process.
    pub(crate) fn interrupt(&self) {
        let _ = self
            .child
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .kill();
    }

    /// Asks the renderer loop to stop without joining it.
    pub(crate) fn request_shutdown(&self) {
        let _ = self.sink.shutdown();
    }

    fn shutdown(&mut self) {
        self.request_shutdown();
        self.interrupt();
        if let Some(join) = self.pump_join.take() {
            let _ = join.join();
        }
        let mut child = self.child.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = child.kill();
        let _ = child.wait();
        drop(child);
        if let Some(join) = self.stderr_join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for RendererHandle {
    fn drop(&mut self) {
        self.shutdown();
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
    /// Process spawn failure.
    pub(crate) fn acquire(&self, site: &Site) -> io::Result<Arc<RendererHandle>> {
        let id = RendererId(self.next.fetch_add(1, Ordering::Relaxed));
        spawn_process(id, site, self.fetch.clone()).map(Arc::new)
    }
}

#[derive(Clone)]
struct Sink(Arc<Mutex<ChildStdin>>);

impl Sink {
    fn send(&self, message: &ToRenderer) -> io::Result<()> {
        let line = renderer::encode_ipc_message(message)?;
        let mut stdin = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        stdin.write_all(&line)?;
        stdin.write_all(b"\n")?;
        stdin.flush()
    }

    fn shutdown(&self) -> io::Result<()> {
        let message = ToRenderer::Request {
            id: 0,
            command: RendererCommand::Shutdown,
        };
        self.send(&message)
    }
}

struct RendererViolation;

fn spawn_process(id: RendererId, site: &Site, fetch: FetchHandle) -> io::Result<RendererHandle> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("renderer")
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
    let sink = Sink(Arc::new(Mutex::new(stdin)));
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
                child: &pump_child,
            };
            pump_loop(BufReader::new(stdout), &context, Some(ready_tx));
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
        child,
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
    child: &'a Arc<Mutex<Child>>,
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
                        Err(async_mpsc::error::TrySendError::Closed(_)) => false,
                        Err(async_mpsc::error::TrySendError::Full(_)) => {
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
                    let worker_child = Arc::clone(self.child);
                    let worker_alive = Arc::clone(self.alive);
                    let submitted = self.fetch.try_submit(move || {
                        let outcome = worker_fetch.dial_request(&request, &initiator);
                        if worker_sink
                            .send(&ToRenderer::ServiceReply {
                                id,
                                reply: ServiceReply::Dial(outcome),
                            })
                            .is_err()
                        {
                            worker_alive.store(false, Ordering::Relaxed);
                            terminate(&worker_child);
                        }
                    });
                    if submitted.is_err() {
                        self.sink
                            .send(&ToRenderer::ServiceReply {
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
                        .send(&ToRenderer::ServiceReply { id, reply })
                        .map_err(|_| RendererViolation)?;
                }
                ServiceCall::CookieSet { value, url } => {
                    let Some(url) = self.site.authorize(&url) else {
                        return Err(RendererViolation);
                    };
                    self.fetch.set_cookie(&value, &url);
                    self.sink
                        .send(&ToRenderer::ServiceReply {
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

fn pump_loop(
    mut reader: BufReader<std::process::ChildStdout>,
    context: &PumpContext<'_>,
    ready: Option<Sender<bool>>,
) {
    let mut ready = ready;
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
    terminate(context.child);
}

fn terminate(child: &Arc<Mutex<Child>>) {
    let mut child = child.lock().unwrap_or_else(PoisonError::into_inner);
    let _ = child.kill();
    let _ = child.wait();
}
