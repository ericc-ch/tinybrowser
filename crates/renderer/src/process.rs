//! `--renderer` child transport: one JSON object per line over stdin/stdout.
//!
//! [ADR 0011](../../../docs/adrs/0011-renderer-processes-per-site.md): the
//! child is the same executable; commands arrive on stdin, replies, events,
//! and browser-service calls leave on stdout. stderr stays for diagnostics.

use std::collections::HashMap;
use std::io::{self, BufReader, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender, SyncSender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use url::Url;

use crate::Stop;
use crate::protocol::{
    BrowserServices, Command, DialCompletion, DialRequest, FromRenderer, RENDERER_INBOX_CAPACITY,
    RENDERER_OUTBOX_CAPACITY, ServiceCall, ServiceReply, ToRenderer,
};

/// Runs the renderer child until `Shutdown` or stdin closes.
///
/// # Errors
///
/// I/O failure while draining the writer.
pub fn serve_stdio() -> io::Result<()> {
    let (command_tx, command_rx) = mpsc::sync_channel::<ToRenderer>(RENDERER_INBOX_CAPACITY);
    let (out_tx, out_rx) = mpsc::sync_channel::<FromRenderer>(RENDERER_OUTBOX_CAPACITY);
    let stop = Arc::new(Stop::new());
    let writer_stop = Arc::clone(&stop);
    let writer_commands = command_tx.clone();
    let writer = thread::spawn(move || {
        let result = write_messages(&out_rx);
        if result.is_err() {
            writer_stop.request();
            let _ = writer_commands.try_send(ToRenderer::Request {
                id: 0,
                command: Command::Shutdown,
            });
        }
        result
    });
    let services = Arc::new(PipeServices::new(out_tx.clone()));
    let _ready = out_tx.try_send(FromRenderer::Ready);
    logging::info!(target: "renderer", "ready");
    let reader_services = Arc::clone(&services);
    thread::spawn(move || read_messages(&command_tx, &reader_services));
    crate::run_with_stop(&command_rx, &out_tx, services, &stop);
    // The reader returns on `Shutdown`, dropping its `PipeServices` clone, so
    // the writer channel closes and the child can exit.
    drop(out_tx);
    writer
        .join()
        .map_err(|_| io::Error::other("writer panicked"))?
}

fn read_messages(command_tx: &SyncSender<ToRenderer>, services: &PipeServices) {
    let mut input = BufReader::new(io::stdin());
    let mut buffer = Vec::new();
    loop {
        let message = match crate::read_ipc_message::<ToRenderer>(&mut input, &mut buffer) {
            Ok(Some(message)) => message,
            Ok(None) => return,
            Err(error) => {
                logging::error!(target: "renderer::ipc", "bad host message: {error}");
                return;
            }
        };
        match message {
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
}

fn write_messages(rx: &mpsc::Receiver<FromRenderer>) -> io::Result<()> {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    for message in rx {
        let line = crate::encode_ipc_message(&message)?;
        out.write_all(&line)?;
        out.write_all(b"\n")?;
        out.flush()?;
    }
    Ok(())
}

/// [`BrowserServices`] proxy that asks the browser process over the pipe.
struct PipeServices {
    out: SyncSender<FromRenderer>,
    pending: Mutex<HashMap<u64, PendingService>>,
    next: AtomicU64,
}

enum PendingService {
    Blocking(Sender<ServiceReply>),
    Dial(DialCompletion),
}

impl PipeServices {
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

impl BrowserServices for PipeServices {
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
