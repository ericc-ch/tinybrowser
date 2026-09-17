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

use renderer::{BrowserServices, DialCompletion, DialRequest, Stop};
use tokio::sync::{Notify, mpsc};
use url::Url;

use crate::wire::channel::{FrameKind, decode_control, read_frame};
use crate::wire::{
    Command, FromRenderer, RendererAssignmentId, ServiceCall, ServiceReply, ToRenderer,
};

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
    let (out_tx, out_rx) = std_mpsc::sync_channel::<FromRenderer>(OUTBOX_CAPACITY);
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
    let _ready = out_tx.try_send(FromRenderer::Ready);
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
            }
        }
        match message {
            ToRenderer::Hello => {
                logging::error!(target: "renderer::ipc", "duplicate handshake");
                break;
            }
            ToRenderer::Assign { .. } | ToRenderer::Release { .. } | ToRenderer::Request { .. } => {
                let shutdown = matches!(
                    &message,
                    ToRenderer::Request {
                        command: Command::Shutdown,
                        ..
                    }
                );
                if command_tx
                    .blocking_send(RendererInput::Control(message))
                    .is_err()
                    || shutdown
                {
                    return;
                }
            }
            ToRenderer::ResponseStart { .. }
            | ToRenderer::ResponseEnd { .. }
            | ToRenderer::ResponseError { .. } => {
                if command_tx
                    .blocking_send(RendererInput::Control(message))
                    .is_err()
                {
                    return;
                }
            }
            ToRenderer::ServiceReply { id, reply } => services.deliver(id, reply),
        }
    }
    // The host is gone (EOF or a broken pipe). A renderer must not outlive its
    // browser: interrupt in-flight work and stop the loop.
    stop.request();
    wake.notify_one();
    let _ = command_tx.try_send(RendererInput::Control(ToRenderer::shutdown_request()));
}

pub(crate) enum RendererInput {
    Control(ToRenderer),
    Body { request: u64, bytes: Vec<u8> },
}

fn write_messages(
    rx: &std_mpsc::Receiver<FromRenderer>,
    mut output: Box<dyn Write + Send>,
) -> io::Result<()> {
    for message in rx {
        crate::wire::channel::write_control(&mut output, &message)?;
    }
    Ok(())
}

/// [`BrowserServices`] proxy that asks the browser process over the channel.
pub(crate) struct ChannelServices {
    out: SyncSender<FromRenderer>,
    pending: Mutex<HashMap<u64, PendingService>>,
    next: AtomicU64,
}

enum PendingService {
    Blocking(Sender<ServiceReply>),
    Dial(DialCompletion),
}

impl ChannelServices {
    fn new(out: SyncSender<FromRenderer>) -> Self {
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
            .try_send(FromRenderer::ServiceCall {
                assignment,
                id,
                call,
            })
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
                ServiceReply::Cookie(_) | ServiceReply::Unit => {
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
}
