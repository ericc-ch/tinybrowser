//! Host side of the renderer seam: factory, handles, and routing pumps.
//!
//! [ADR 0011](../../../docs/adrs/0011-renderer-processes-per-site.md): the
//! handle is value-only; replies, events, and browser-service calls cross a
//! channel pair (local backend) or a pipe (process backend).

use std::collections::HashMap;
use std::io::{self, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::network::FetchHandle;
use crate::site::Site;
use renderer::{
    BrowserServices, Command as RendererCommand, FrameId, FromRenderer, Reply, ServiceCall,
    ServiceReply, Stop, TabError, TabEvent, ToRenderer,
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
        let _ = self.sink.send(ToRenderer::Request {
            id: 0,
            command: RendererCommand::Shutdown,
        });
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
    Local(Sender<ToRenderer>),
    Pipe(Arc<Mutex<ChildStdin>>),
}

impl Sink {
    fn send(&self, message: ToRenderer) -> io::Result<()> {
        match self {
            Self::Local(tx) => tx
                .send(message)
                .map_err(|_| io::Error::other("renderer stopped")),
            Self::Pipe(stdin) => {
                let line = renderer::encode_ipc_message(&message)?;
                let mut stdin = stdin.lock().unwrap_or_else(PoisonError::into_inner);
                stdin.write_all(&line)?;
                stdin.write_all(b"\n")?;
                stdin.flush()
            }
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
    let (to_tx, to_rx) = mpsc::channel::<ToRenderer>();
    let (from_tx, from_rx) = mpsc::channel::<FromRenderer>();
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
                terminator: RendererTerminator::Local(&pump_stop),
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
    }
}

fn spawn_process(id: RendererId, site: &Site, fetch: FetchHandle) -> io::Result<RendererHandle> {
    let mut child = Command::new(std::env::current_exe()?)
        .arg("--renderer")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
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
                terminator: RendererTerminator::Process(&pump_child),
            };
            pump_loop(
                Incoming::Pipe(BufReader::new(stdout)),
                &context,
                Some(ready_tx),
            );
        })
        .expect("renderer pump thread");
    if ready_rx.recv_timeout(HANDSHAKE_TIMEOUT) != Ok(true) {
        let mut child = child.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = child.kill();
        let _ = child.wait();
        return Err(io::Error::other("renderer handshake failed"));
    }
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
    })
}

struct PumpContext<'a> {
    sink: &'a Sink,
    pending: &'a Arc<Mutex<HashMap<u64, Sender<Reply>>>>,
    alive: &'a AtomicBool,
    subscribers: &'a EventSubscribers,
    fetch: &'a FetchHandle,
    site: &'a Site,
    terminator: RendererTerminator<'a>,
}

#[derive(Clone, Copy)]
enum RendererTerminator<'a> {
    Local(&'a Stop),
    Process(&'a Arc<Mutex<Child>>),
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
                    let submitted = self.fetch.try_submit(move || {
                        let outcome = worker_fetch.dial_request(&request, &initiator);
                        let _ = worker_sink.send(ToRenderer::ServiceReply {
                            id,
                            reply: ServiceReply::Dial(outcome),
                        });
                    });
                    if submitted.is_err() {
                        let _ = self.sink.send(ToRenderer::ServiceReply {
                            id,
                            reply: ServiceReply::Dial(None),
                        });
                    }
                }
                ServiceCall::CookieGet { url } => {
                    let Some(url) = self.site.authorize(&url) else {
                        return Err(RendererViolation);
                    };
                    let reply = ServiceReply::Cookie(self.fetch.cookies_for(&url));
                    let _ = self.sink.send(ToRenderer::ServiceReply { id, reply });
                }
                ServiceCall::CookieSet { value, url } => {
                    let Some(url) = self.site.authorize(&url) else {
                        return Err(RendererViolation);
                    };
                    self.fetch.set_cookie(&value, &url);
                    let _ = self.sink.send(ToRenderer::ServiceReply {
                        id,
                        reply: ServiceReply::Unit,
                    });
                }
            },
        }
        Ok(())
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
            while let Ok(Some(message)) = renderer::read_ipc_message(&mut reader, &mut buffer) {
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
    match context.terminator {
        RendererTerminator::Local(stop) => {
            stop.request();
            let _ = context.sink.send(ToRenderer::Request {
                id: 0,
                command: RendererCommand::Shutdown,
            });
        }
        RendererTerminator::Process(child) => {
            let mut child = child.lock().unwrap_or_else(PoisonError::into_inner);
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
