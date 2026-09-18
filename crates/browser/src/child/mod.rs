//! The renderer *as a child process*: this crate's second role.
//!
//! The child is the same executable re-executed with the `renderer` argument.
//! On Unix the browser hands it one end of an unnamed socket pair as file
//! descriptor 0; other platforms keep the stdin/stdout pipes. stderr stays for
//! diagnostics, which `crate::link` forwards into the daemon's log.
//!
//! This module is child-side only. It must not reach for the `link`,
//! `manager`, `actor`, or `store` modules: a renderer serves one process's
//! pages and never acts as a browser. `serve` takes no handles for exactly that
//! reason — everything it needs comes from fd 0 and the engine.
//!
//! The loop runs as a future on one current-thread Tokio runtime and owns every
//! wait: commands, dial completions, timer deadlines, and shutdown. The reader
//! and writer stay blocking threads because synchronous browser-service calls
//! (`document.cookie`) must make progress while the page engine runs.

mod session;

/// Bounded command channel: one renderer's inbound messages.
const INBOX_CAPACITY: usize = 256;

/// Bounded outbox: messages waiting to be written to the host.
const OUTBOX_CAPACITY: usize = 4096;

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self as std_mpsc, Sender, SyncSender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use renderer::{
    BrowserServices, DialCompletion, DialRequest, FrameId, Stop, StorageChange, StorageError,
    StorageSeed,
};
use tokio::sync::{Notify, mpsc};
use url::Url;

use crate::wire::channel::{FrameKind, decode_control, read_frame, write_frame};
use crate::wire::{
    Command, FromRenderer, RendererAssignmentId, ServiceCall, ServiceReply, ToRenderer,
};

/// One message the renderer writes to the browser: control JSON or a raw body
/// chunk of a streamed reply.
pub(crate) enum Outgoing {
    /// A control message.
    Message(FromRenderer),
    /// Raw bytes for one request id.
    Body { request: u64, payload: Vec<u8> },
}

/// Runs the renderer child until `Shutdown` or the channel closes.
///
/// # Errors
///
/// Runtime creation, channel bootstrap, or writer failure.
pub fn serve() -> io::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()?;
    let writer = runtime.block_on(serve_async())?;
    writer
        .join()
        .map_err(|_| io::Error::other("writer panicked"))?
}

async fn serve_async() -> io::Result<thread::JoinHandle<io::Result<()>>> {
    let (input, output) = endpoint()?;
    let (command_tx, command_rx) = mpsc::channel::<RendererInput>(INBOX_CAPACITY);
    let (out_tx, out_rx) = std_mpsc::sync_channel::<Outgoing>(OUTBOX_CAPACITY);
    let stop = Arc::new(Stop::new());
    let wake = Arc::new(Notify::new());
    let writer_stop = Arc::clone(&stop);
    let writer_wake = Arc::clone(&wake);
    let writer_commands = command_tx.clone();
    let writer = thread::spawn(move || {
        let result = write_messages(&out_rx, output);
        if result.is_err() {
            writer_stop.request();
            writer_wake.notify_one();
            let shutdown = RendererInput::Control(ToRenderer::shutdown_request());
            let _ = writer_commands.try_send(shutdown);
        }
        result
    });
    let services = Arc::new(ChannelServices::new(out_tx.clone()));
    let _ready = out_tx.try_send(Outgoing::Message(FromRenderer::Ready));
    logging::info!(target: "renderer", "ready");
    let reader_services = Arc::clone(&services);
    let reader_stop = Arc::clone(&stop);
    let reader_wake = Arc::clone(&wake);
    thread::spawn(move || {
        read_messages(
            input,
            &command_tx,
            &reader_services,
            &reader_stop,
            &reader_wake,
        );
    });
    session::run(command_rx, &out_tx, services, &stop, Arc::clone(&wake)).await;
    // The reader returns on `Shutdown`, dropping its `ChannelServices` clone, so
    // the writer channel closes and the child can exit.
    drop(out_tx);
    Ok(writer)
}

/// One child endpoint. Unix duplicates the inherited descriptor; other
/// platforms split stdin and stdout.
#[cfg(unix)]
fn endpoint() -> io::Result<(Box<dyn Read + Send>, Box<dyn Write + Send>)> {
    use std::os::fd::AsFd;
    use std::os::unix::net::UnixStream;

    let descriptor = std::io::stdin().as_fd().try_clone_to_owned()?;
    let stream = UnixStream::from(descriptor);
    let input = stream.try_clone()?;
    Ok((Box::new(input), Box::new(stream)))
}

#[cfg(not(unix))]
fn endpoint() -> io::Result<(Box<dyn Read + Send>, Box<dyn Write + Send>)> {
    Ok((Box::new(std::io::stdin()), Box::new(std::io::stdout())))
}

fn read_messages(
    mut input: Box<dyn Read + Send>,
    command_tx: &mpsc::Sender<RendererInput>,
    services: &ChannelServices,
    stop: &Arc<Stop>,
    wake: &Arc<Notify>,
) {
    let mut buffer = Vec::new();
    let mut greeted = false;
    loop {
        let frame = match read_frame(&mut input, &mut buffer) {
            Ok(Some(frame)) => frame,
            Ok(None) => break,
            Err(error) => {
                logging::error!(target: "renderer::ipc", "bad host message: {error}");
                break;
            }
        };
        if frame.kind == FrameKind::Body {
            if command_tx
                .blocking_send(RendererInput::Body {
                    request: frame.request,
                    bytes: buffer.clone(),
                })
                .is_err()
            {
                break;
            }
            continue;
        }
        let message = match decode_control::<ToRenderer>(&buffer) {
            Ok(message) => message,
            Err(error) => {
                logging::error!(target: "renderer::ipc", "bad host message: {error}");
                break;
            }
        };
        if !greeted {
            match message {
                ToRenderer::Hello => {
                    greeted = true;
                    continue;
                }
                ToRenderer::ServiceReply { id, reply } => {
                    services.deliver(id, reply);
                    continue;
                }
                ToRenderer::Assign { .. }
                | ToRenderer::Release { .. }
                | ToRenderer::Request { .. } => {
                    logging::error!(target: "renderer::ipc", "request before handshake");
                    break;
                }
                ToRenderer::ResponseStart { .. }
                | ToRenderer::ResponseEnd { .. }
                | ToRenderer::ResponseError { .. } => {
                    logging::error!(target: "renderer::ipc", "response before handshake");
                    break;
                }
                ToRenderer::StorageEvent { .. } => {
                    logging::error!(target: "renderer::ipc", "storage event before handshake");
                    break;
                }
                ToRenderer::BroadcastMessage { .. } => {
                    logging::error!(target: "renderer::ipc", "broadcast before handshake");
                    break;
                }
            }
        }
        if !route_message(message, command_tx, services) {
            return;
        }
    }
    // The host is gone (EOF or a broken pipe). A renderer must not outlive its
    // browser: interrupt in-flight work and stop the loop.
    stop.request();
    wake.notify_one();
    let _ = command_tx.try_send(RendererInput::Control(ToRenderer::shutdown_request()));
}

/// Routes one post-handshake host message; `false` stops the read loop.
fn route_message(
    message: ToRenderer,
    command_tx: &mpsc::Sender<RendererInput>,
    services: &ChannelServices,
) -> bool {
    match message {
        ToRenderer::Hello => {
            logging::error!(target: "renderer::ipc", "duplicate handshake");
            false
        }
        ToRenderer::Assign { .. } | ToRenderer::Release { .. } | ToRenderer::Request { .. } => {
            let shutdown = matches!(
                &message,
                ToRenderer::Request {
                    command: Command::Shutdown,
                    ..
                }
            );
            command_tx
                .blocking_send(RendererInput::Control(message))
                .is_ok()
                && !shutdown
        }
        ToRenderer::ResponseStart { .. }
        | ToRenderer::ResponseEnd { .. }
        | ToRenderer::ResponseError { .. } => command_tx
            .blocking_send(RendererInput::Control(message))
            .is_ok(),
        ToRenderer::StorageEvent { .. } => {
            // Storage events are best-effort, like the spec's task queue. The
            // engine may be blocked in a synchronous service call, so blocking
            // here would stall the service reply it waits on.
            match command_tx.try_send(RendererInput::Control(message)) {
                Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => true,
                Err(mpsc::error::TrySendError::Closed(_)) => false,
            }
        }
        ToRenderer::BroadcastMessage { .. } => {
            // Same best-effort rule as storage events.
            match command_tx.try_send(RendererInput::Control(message)) {
                Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => true,
                Err(mpsc::error::TrySendError::Closed(_)) => false,
            }
        }
        ToRenderer::ServiceReply { id, reply } => {
            services.deliver(id, reply);
            true
        }
    }
}

pub(crate) enum RendererInput {
    Control(ToRenderer),
    Body { request: u64, bytes: Vec<u8> },
}

fn write_messages(
    rx: &std_mpsc::Receiver<Outgoing>,
    mut output: Box<dyn Write + Send>,
) -> io::Result<()> {
    for message in rx {
        match message {
            Outgoing::Message(message) => {
                crate::wire::channel::write_control(&mut output, &message)?;
            }
            Outgoing::Body { request, payload } => {
                write_frame(&mut output, FrameKind::Body, request, &payload)?;
            }
        }
    }
    Ok(())
}

/// [`BrowserServices`] proxy that asks the browser process over the channel.
pub(crate) struct ChannelServices {
    out: SyncSender<Outgoing>,
    pending: Mutex<HashMap<u64, PendingService>>,
    next: AtomicU64,
}

enum PendingService {
    Blocking(Sender<ServiceReply>),
    Dial(DialCompletion),
}

impl ChannelServices {
    fn new(out: SyncSender<Outgoing>) -> Self {
        Self {
            out,
            pending: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
        }
    }

    /// Registers `pending` under a fresh id and queues one service call. On
    /// queue failure the registration is rolled back and returned so the
    /// caller can take its own failure path.
    fn begin_service(
        &self,
        assignment: RendererAssignmentId,
        call: ServiceCall,
        pending: PendingService,
    ) -> Result<(), PendingService> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, pending);
        if self
            .out
            .try_send(Outgoing::Message(FromRenderer::ServiceCall {
                assignment,
                id,
                call,
            }))
            .is_ok()
        {
            return Ok(());
        }
        match self
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id)
        {
            Some(pending) => Err(pending),
            // `deliver` is the only other remover, and it cannot see an id the
            // host never received, so this branch is unreachable.
            None => Ok(()),
        }
    }

    fn call(&self, assignment: RendererAssignmentId, call: ServiceCall) -> Option<ServiceReply> {
        let (reply_tx, reply_rx) = std_mpsc::channel();
        self.begin_service(assignment, call, PendingService::Blocking(reply_tx))
            .ok()?;
        reply_rx.recv().ok()
    }

    fn deliver(&self, id: u64, reply: ServiceReply) {
        let pending = self
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
        match pending {
            Some(PendingService::Blocking(reply_tx)) => {
                let _ = reply_tx.send(reply);
            }
            Some(PendingService::Dial(completion)) => match reply {
                ServiceReply::Dial(outcome) => completion(outcome),
                ServiceReply::Cookie(_)
                | ServiceReply::StorageValue(_)
                | ServiceReply::StorageKeys(_)
                | ServiceReply::StorageChanged(_)
                | ServiceReply::Window(_)
                | ServiceReply::Unit => {
                    completion(Err(renderer::DialFailure::Connect));
                }
            },
            None => {}
        }
    }
}

pub(crate) struct AssignmentServices {
    assignment: RendererAssignmentId,
    channel: Arc<ChannelServices>,
}

impl AssignmentServices {
    pub(crate) fn new(assignment: RendererAssignmentId, channel: Arc<ChannelServices>) -> Self {
        Self {
            assignment,
            channel,
        }
    }
}

impl BrowserServices for AssignmentServices {
    fn start_dial(&self, request: DialRequest, completion: DialCompletion) {
        let pending = self.channel.begin_service(
            self.assignment,
            ServiceCall::Dial(request),
            PendingService::Dial(completion),
        );
        if let Err(PendingService::Dial(completion)) = pending {
            completion(Err(renderer::DialFailure::Connect));
        }
    }

    fn cookies_for(&self, url: &Url) -> String {
        match self.channel.call(
            self.assignment,
            ServiceCall::CookieGet {
                url: url.to_string(),
            },
        ) {
            Some(ServiceReply::Cookie(value)) => value,
            _ => String::new(),
        }
    }

    fn set_cookie(&self, value: &str, url: &Url) {
        let _result = self.channel.call(
            self.assignment,
            ServiceCall::CookieSet {
                value: value.to_owned(),
                url: url.to_string(),
            },
        );
    }

    fn storage_get(&self, origin: &str, key: &str) -> Option<String> {
        match self.channel.call(
            self.assignment,
            ServiceCall::StorageGet {
                origin: origin.to_owned(),
                key: key.to_owned(),
            },
        ) {
            Some(ServiceReply::StorageValue(value)) => value,
            _ => None,
        }
    }

    fn storage_keys(&self, origin: &str) -> Vec<String> {
        match self.channel.call(
            self.assignment,
            ServiceCall::StorageKeys {
                origin: origin.to_owned(),
            },
        ) {
            Some(ServiceReply::StorageKeys(keys)) => keys,
            _ => Vec::new(),
        }
    }

    fn storage_set(
        &self,
        origin: &str,
        url: &str,
        key: &str,
        value: &str,
        source: FrameId,
    ) -> Result<Option<StorageChange>, StorageError> {
        match self.channel.call(
            self.assignment,
            ServiceCall::StorageSet {
                origin: origin.to_owned(),
                url: url.to_owned(),
                key: key.to_owned(),
                value: value.to_owned(),
                source,
            },
        ) {
            Some(ServiceReply::StorageChanged(change)) => change,
            _ => Ok(None),
        }
    }

    fn storage_remove(
        &self,
        origin: &str,
        url: &str,
        key: &str,
        source: FrameId,
    ) -> Option<StorageChange> {
        match self.channel.call(
            self.assignment,
            ServiceCall::StorageRemove {
                origin: origin.to_owned(),
                url: url.to_owned(),
                key: key.to_owned(),
                source,
            },
        ) {
            Some(ServiceReply::StorageChanged(Ok(change))) => change,
            _ => None,
        }
    }

    fn storage_clear(&self, origin: &str, url: &str, source: FrameId) -> Option<StorageChange> {
        match self.channel.call(
            self.assignment,
            ServiceCall::StorageClear {
                origin: origin.to_owned(),
                url: url.to_owned(),
                source,
            },
        ) {
            Some(ServiceReply::StorageChanged(Ok(change))) => change,
            _ => None,
        }
    }

    fn window_open(
        &self,
        url: &str,
        name: &str,
        features: &str,
        seed: Option<&StorageSeed>,
    ) -> Option<u64> {
        match self.channel.call(
            self.assignment,
            ServiceCall::WindowOpen {
                url: url.to_owned(),
                name: name.to_owned(),
                features: features.to_owned(),
                seed: seed.cloned(),
            },
        ) {
            Some(ServiceReply::Window(tab)) => tab,
            _ => None,
        }
    }

    fn window_close(&self, tab: u64) {
        let _result = self
            .channel
            .call(self.assignment, ServiceCall::WindowClose { tab });
    }

    fn window_opener(&self) -> Option<u64> {
        match self.channel.call(self.assignment, ServiceCall::Opener) {
            Some(ServiceReply::Window(tab)) => tab,
            _ => None,
        }
    }

    fn window_post_message(&self, tab: u64, payload: &str) {
        let _result = self.channel.call(
            self.assignment,
            ServiceCall::WindowMessage {
                tab,
                payload: payload.to_owned(),
            },
        );
    }

    fn remote_session_get(&self, tab: u64, origin: &str, key: &str) -> Option<String> {
        match self.channel.call(
            self.assignment,
            ServiceCall::RemoteSessionGet {
                tab,
                origin: origin.to_owned(),
                key: key.to_owned(),
            },
        ) {
            Some(ServiceReply::StorageValue(value)) => value,
            _ => None,
        }
    }

    fn broadcast_post(&self, origin: &str, name: &str, payload: &str, channel: u64) {
        let _result = self.channel.call(
            self.assignment,
            ServiceCall::BroadcastPost {
                origin: origin.to_owned(),
                name: name.to_owned(),
                payload: payload.to_owned(),
                channel,
            },
        );
    }
}
