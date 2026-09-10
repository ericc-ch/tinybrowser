//! Host side of the renderer seam: registry, handles, and routing pumps.
//!
//! [ADR 0011](../../../docs/adrs/0011-renderer-processes-per-site.md): the
//! handle is value-only; replies, events, and browser-service calls cross a
//! channel pair (local backend) or a pipe (process backend).

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use renderer::{
    BrowserServices, Command as RendererCommand, FromRenderer, Reply, ServiceCall, ServiceReply,
    Stop, TabError, TabEvent, ToRenderer,
};
use url::Url;

use crate::network::FetchHandle;
use crate::site::Site;

/// Upper bound on one request to a renderer. The renderer budget is seconds;
/// this is a last-resort wake-up if its reply path dies silently.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// How long a `--renderer` child has to say [`FromRenderer::Ready`].
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

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
pub struct RendererId(u64);

/// Value-only handle to one renderer.
pub struct RendererHandle {
    id: RendererId,
    site: Site,
    sink: Sink,
    pending: Arc<Mutex<HashMap<u64, Sender<Reply>>>>,
    subscribers: Arc<Mutex<Vec<Sender<TabEvent>>>>,
    next_request: Arc<AtomicU64>,
    stop: Option<Arc<Stop>>,
    child: Option<Arc<Mutex<Child>>>,
    renderer_join: Option<JoinHandle<()>>,
    pump_join: Option<JoinHandle<()>>,
}

impl RendererHandle {
    /// Identity of this renderer.
    #[must_use]
    pub fn id(&self) -> RendererId {
        self.id
    }

    /// Site instance this renderer is locked to.
    #[must_use]
    pub fn site(&self) -> &Site {
        &self.site
    }

    /// Sends one command and waits for its reply.
    ///
    /// # Errors
    ///
    /// [`TabError::ActorStopped`] when the renderer is gone.
    pub fn request(&self, command: RendererCommand) -> Result<Reply, TabError> {
        let id = self.next_request.fetch_add(1, Ordering::Relaxed);
        let (reply_tx, reply_rx) = mpsc::channel();
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, reply_tx);
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
    pub fn subscribe(&self) -> Receiver<TabEvent> {
        let (tx, rx) = mpsc::channel();
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(tx);
        rx
    }

    /// Interrupts a blocked script now: the local backend flips the shared
    /// stop flag; the process backend kills the child.
    pub fn interrupt(&self) {
        if let Some(stop) = &self.stop {
            stop.request();
        }
        if let Some(child) = &self.child {
            let _ = child.lock().unwrap_or_else(PoisonError::into_inner).kill();
        }
    }

    /// Asks the renderer loop to stop without joining it.
    pub fn request_shutdown(&self) {
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

/// Backend-selecting registry of live and idle renderers keyed by site.
pub struct RendererRegistry {
    backend: Renderers,
    fetch: FetchHandle,
    idle: Mutex<HashMap<Site, Vec<Arc<RendererHandle>>>>,
    next: AtomicU64,
}

impl RendererRegistry {
    pub(crate) fn new(backend: Renderers, fetch: FetchHandle) -> Self {
        Self {
            backend,
            fetch,
            idle: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
        }
    }

    /// A renderer for `site`, reusing an idle one when possible.
    ///
    /// # Errors
    ///
    /// Process spawn failure.
    pub fn acquire(&self, site: &Site) -> io::Result<Arc<RendererHandle>> {
        if let Some(handle) = self
            .idle
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_mut(site)
            .and_then(Vec::pop)
        {
            return Ok(handle);
        }
        let id = RendererId(self.next.fetch_add(1, Ordering::Relaxed));
        match self.backend {
            Renderers::Local => {
                let services = Arc::new(crate::services::FetchServices::new(self.fetch.clone()));
                Ok(Arc::new(spawn_local(
                    id,
                    site.clone(),
                    services,
                    self.fetch.clone(),
                )))
            }
            Renderers::Process => spawn_process(id, site.clone(), self.fetch.clone()).map(Arc::new),
        }
    }

    /// Returns a renderer to the idle pool, keyed by its site. Opaque
    /// per-tab instances are dropped instead: no other tab can reuse them.
    pub fn release(&self, handle: Arc<RendererHandle>) {
        if handle.site().is_opaque() {
            return;
        }
        let site = handle.site().clone();
        self.idle
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(site)
            .or_default()
            .push(handle);
    }

    /// Stops every idle renderer. Live pages own their own handles.
    pub fn shutdown(&self) {
        let mut idle = self.idle.lock().unwrap_or_else(PoisonError::into_inner);
        let handles: Vec<Arc<RendererHandle>> =
            idle.drain().flat_map(|(_, handles)| handles).collect();
        drop(idle);
        drop(handles);
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
                let line = serde_json::to_string(&message)
                    .map_err(|error| io::Error::other(error.to_string()))?;
                let mut stdin = stdin.lock().unwrap_or_else(PoisonError::into_inner);
                writeln!(stdin, "{line}")?;
                stdin.flush()
            }
        }
    }
}

enum Incoming {
    Local(Receiver<FromRenderer>),
    Pipe(BufReader<std::process::ChildStdout>),
}

fn spawn_local(
    id: RendererId,
    site: Site,
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
    let subscribers = Arc::new(Mutex::new(Vec::new()));
    let pump_sink = sink.clone();
    let pump_pending = Arc::clone(&pending);
    let pump_subscribers = Arc::clone(&subscribers);
    let name = format!("renderer-{id:?}-pump");
    let pump_join = thread::Builder::new()
        .name(name)
        .spawn(move || {
            pump_loop(
                Incoming::Local(from_rx),
                &pump_sink,
                &pump_pending,
                &pump_subscribers,
                &fetch,
                None,
                None,
            );
        })
        .expect("renderer pump thread");
    RendererHandle {
        id,
        site,
        sink,
        pending,
        subscribers,
        next_request: Arc::new(AtomicU64::new(1)),
        stop: Some(stop),
        child: None,
        renderer_join: Some(renderer_join),
        pump_join: Some(pump_join),
    }
}

fn spawn_process(id: RendererId, site: Site, fetch: FetchHandle) -> io::Result<RendererHandle> {
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
    let subscribers = Arc::new(Mutex::new(Vec::new()));
    let pump_sink = sink.clone();
    let pump_pending = Arc::clone(&pending);
    let pump_subscribers = Arc::clone(&subscribers);
    let pump_child = Arc::clone(&child);
    let (ready_tx, ready_rx) = mpsc::channel();
    let name = format!("renderer-{id:?}-pump");
    let pump_join = thread::Builder::new()
        .name(name)
        .spawn(move || {
            pump_loop(
                Incoming::Pipe(BufReader::new(stdout)),
                &pump_sink,
                &pump_pending,
                &pump_subscribers,
                &fetch,
                Some(&pump_child),
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
        id,
        site,
        sink,
        pending,
        subscribers,
        next_request: Arc::new(AtomicU64::new(1)),
        stop: None,
        child: Some(child),
        renderer_join: None,
        pump_join: Some(pump_join),
    })
}

fn pump_loop(
    incoming: Incoming,
    sink: &Sink,
    pending: &Arc<Mutex<HashMap<u64, Sender<Reply>>>>,
    subscribers: &Arc<Mutex<Vec<Sender<TabEvent>>>>,
    fetch: &FetchHandle,
    child: Option<&Arc<Mutex<Child>>>,
    ready: Option<Sender<bool>>,
) {
    let mut ready = ready;
    match incoming {
        Incoming::Local(receiver) => {
            while let Ok(message) = receiver.recv() {
                route(message, sink, pending, subscribers, fetch, false);
            }
        }
        Incoming::Pipe(mut reader) => {
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let Ok(message) = serde_json::from_str::<FromRenderer>(line.trim()) else {
                    eprintln!("renderer: bad child message: {}", line.trim());
                    break;
                };
                if let Some(ready_tx) = ready.take() {
                    let ok = matches!(message, FromRenderer::Ready);
                    let _ = ready_tx.send(ok);
                    if !ok {
                        break;
                    }
                    continue;
                }
                route(message, sink, pending, subscribers, fetch, true);
            }
        }
    }
    if let Some(ready_tx) = ready {
        let _ = ready_tx.send(false);
    }
    // A dead renderer must not strand callers blocked in `request`.
    pending
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
    if let Some(child) = child {
        let mut child = child.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn route(
    message: FromRenderer,
    sink: &Sink,
    pending: &Arc<Mutex<HashMap<u64, Sender<Reply>>>>,
    subscribers: &Arc<Mutex<Vec<Sender<TabEvent>>>>,
    fetch: &FetchHandle,
    offload_dials: bool,
) {
    match message {
        // Handled by the pipe handshake; never routed.
        FromRenderer::Ready => {}
        FromRenderer::Reply { id, reply } => {
            if let Some(reply_tx) = pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&id)
            {
                let _ = reply_tx.send(reply);
            }
        }
        FromRenderer::Event(event) => {
            subscribers
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .retain(|subscriber| subscriber.send(event).is_ok());
        }
        FromRenderer::ServiceCall { id, call } => {
            if offload_dials && let ServiceCall::Dial(request) = call {
                let worker_fetch = fetch.clone();
                let worker_sink = sink.clone();
                let submitted = fetch.try_submit(move || {
                    let outcome = worker_fetch.dial_request(&request);
                    let _ = worker_sink.send(ToRenderer::ServiceReply {
                        id,
                        reply: ServiceReply::Dial(outcome),
                    });
                });
                if submitted.is_err() {
                    let _ = sink.send(ToRenderer::ServiceReply {
                        id,
                        reply: ServiceReply::Dial(None),
                    });
                }
                return;
            }
            let reply = inline_service(fetch, call);
            let _ = sink.send(ToRenderer::ServiceReply { id, reply });
        }
    }
}

fn inline_service(fetch: &FetchHandle, call: ServiceCall) -> ServiceReply {
    match call {
        ServiceCall::Dial(request) => ServiceReply::Dial(fetch.dial_request(&request)),
        ServiceCall::CookieGet { url } => ServiceReply::Cookie(
            Url::parse(&url)
                .map(|url| fetch.cookies_for(&url))
                .unwrap_or_default(),
        ),
        ServiceCall::CookieSet { value, url } => {
            if let Ok(url) = Url::parse(&url) {
                fetch.set_cookie(&value, &url);
            }
            ServiceReply::Unit
        }
        ServiceCall::MarkDirty => {
            fetch.mark_dirty();
            ServiceReply::Unit
        }
    }
}
