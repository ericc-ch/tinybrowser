//! `renderer` child transport: length-prefixed frames over the platform channel.
//!
//! [ADR 0019](../../../docs/adrs/0019-async-browser-runtime-and-io.md): the
//! child is the same executable. On Unix the browser passes one end of an
//! unnamed socket pair as file descriptor 0 and the child reads and writes that
//! endpoint. Other platforms keep the stdin/stdout pipes until their platform
//! channel lands. stderr stays for diagnostics.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender, SyncSender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use url::Url;

use crate::channel::{read_control, write_control};
use crate::document::Stop;
use crate::protocol::{
    BrowserServices, Command, DialCompletion, DialRequest, FromRenderer, RENDERER_INBOX_CAPACITY,
    RENDERER_OUTBOX_CAPACITY, ServiceCall, ServiceReply, ToRenderer,
};

/// Runs the renderer child until `Shutdown` or the channel closes.
///
/// # Errors
///
/// I/O failure while draining the writer.
pub fn serve() -> io::Result<()> {
    let (input, output) = endpoint()?;
    let (command_tx, command_rx) = mpsc::sync_channel::<ToRenderer>(RENDERER_INBOX_CAPACITY);
    let (out_tx, out_rx) = mpsc::sync_channel::<FromRenderer>(RENDERER_OUTBOX_CAPACITY);
    let stop = Arc::new(Stop::new());
    let writer_stop = Arc::clone(&stop);
    let writer_commands = command_tx.clone();
    let writer = thread::spawn(move || {
        let result = write_messages(&out_rx, output);
        if result.is_err() {
            writer_stop.request();
            let _ = writer_commands.try_send(ToRenderer::Request {
                id: 0,
                command: Command::Shutdown,
            });
        }
        result
    });
    let services = Arc::new(ChannelServices::new(out_tx.clone()));
    let _ready = out_tx.try_send(FromRenderer::Ready);
    logging::info!(target: "renderer", "ready");
    let reader_services = Arc::clone(&services);
    let reader_stop = Arc::clone(&stop);
    thread::spawn(move || read_messages(input, &command_tx, &reader_services, &reader_stop));
    crate::run(&command_rx, &out_tx, services, &stop);
    // The reader returns on `Shutdown`, dropping its `ChannelServices` clone, so
    // the writer channel closes and the child can exit.
    drop(out_tx);
    writer
        .join()
        .map_err(|_| io::Error::other("writer panicked"))?
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
    command_tx: &SyncSender<ToRenderer>,
    services: &ChannelServices,
    stop: &Arc<Stop>,
) {
    let mut buffer = Vec::new();
    let mut greeted = false;
    loop {
        let message = match read_control::<ToRenderer>(&mut input, &mut buffer) {
            Ok(Some(message)) => message,
            Ok(None) => break,
            Err(error) => {
                logging::error!(target: "renderer::ipc", "bad host message: {error}");
                break;
            }
        };
        match message {
            ToRenderer::Hello if !greeted => greeted = true,
            ToRenderer::Hello => {
                logging::error!(target: "renderer::ipc", "duplicate handshake");
                break;
            }
            ToRenderer::Request { .. } if !greeted => {
                logging::error!(target: "renderer::ipc", "request before handshake");
                break;
            }
            ToRenderer::Request { .. } => {
                let shutdown = matches!(
                    &message,
                    ToRenderer::Request {
                        command: Command::Shutdown,
                        ..
                    }
                );
                if command_tx.send(message).is_err() || shutdown {
                    return;
                }
            }
            ToRenderer::ServiceReply { id, reply } => services.deliver(id, reply),
        }
    }
    // The host is gone (EOF or a broken pipe). A renderer must not outlive its
    // browser: interrupt in-flight work and stop the loop.
    stop.request();
    let _ = command_tx.send(ToRenderer::Request {
        id: 0,
        command: Command::Shutdown,
    });
}

fn write_messages(
    rx: &mpsc::Receiver<FromRenderer>,
    mut output: Box<dyn Write + Send>,
) -> io::Result<()> {
    for message in rx {
        write_control(&mut output, &message)?;
    }
    Ok(())
}

/// [`BrowserServices`] proxy that asks the browser process over the pipe.
struct ChannelServices {
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

    fn call(&self, call: ServiceCall) -> Option<ServiceReply> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (reply_tx, reply_rx) = mpsc::channel();
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, PendingService::Blocking(reply_tx));
        if self
            .out
            .try_send(FromRenderer::ServiceCall { id, call })
            .is_err()
        {
            self.pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&id);
            return None;
        }
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
                ServiceReply::Cookie(_) | ServiceReply::Unit => completion(None),
            },
            None => {}
        }
    }
}

impl BrowserServices for ChannelServices {
    fn start_dial(&self, request: DialRequest, completion: DialCompletion) {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, PendingService::Dial(std::sync::Arc::clone(&completion)));
        if self
            .out
            .try_send(FromRenderer::ServiceCall {
                id,
                call: ServiceCall::Dial(request),
            })
            .is_err()
        {
            self.pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&id);
            completion(None);
        }
    }

    fn cookies_for(&self, url: &Url) -> String {
        match self.call(ServiceCall::CookieGet {
            url: url.to_string(),
        }) {
            Some(ServiceReply::Cookie(value)) => value,
            _ => String::new(),
        }
    }

    fn set_cookie(&self, value: &str, url: &Url) {
        let _result = self.call(ServiceCall::CookieSet {
            value: value.to_owned(),
            url: url.to_string(),
        });
    }
}
