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

/// Bounded outbox control lane: replies, reverse calls, cancellation.
const OUTBOX_CONTROL_CAPACITY: usize = 256;

/// Bounded outbox data lane: events and screenshot chunks.
const OUTBOX_DATA_CAPACITY: usize = 4096;

use std::io::{self, Read, Write};
use std::sync::Arc;
use std::thread;

use renderer::{
    BrowserServices, DialCompletion, DialRequest, FrameId, Stop, StorageChange, StorageError,
    StorageSeed,
};
use tokio::sync::Notify;
use url::Url;

use crate::exchange::{self, BlockingClient, Frame, RequestId};
use crate::wire::channel::{FrameKind, decode_control, read_frame, write_frame};
use crate::wire::{
    BrowserCall, FromRenderer, HostNotice, RendererAssignmentId, RendererNotice, RendererReply,
    ServiceCall, ServiceReply, ToRenderer,
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
    let (command_tx, command_rx) = exchange::pair(INBOX_CAPACITY, INBOX_CAPACITY);
    let (out_tx, out_rx) = exchange::blocking_pair(OUTBOX_CONTROL_CAPACITY, OUTBOX_DATA_CAPACITY);
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
            let shutdown = Frame::Notify(HostNotice::Shutdown);
            let _ = writer_commands.try_send(shutdown);
        }
        result
    });
    let services = Arc::new(ChannelServices::new(out_tx.clone()));
    let _ready = out_tx.try_send(Frame::Notify(RendererNotice::Ready));
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
    command_tx: &exchange::Sender<ToRenderer>,
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
        let message = match frame.kind {
            FrameKind::Control => match decode_control::<ToRenderer>(&buffer) {
                Ok(message) => message,
                Err(error) => {
                    logging::error!(target: "renderer::ipc", "bad host message: {error}");
                    break;
                }
            },
            FrameKind::RequestChunk => Frame::RequestChunk {
                id: RequestId::new(frame.request),
                bytes: buffer.clone(),
            },
            FrameKind::ResponseChunk => Frame::ResponseChunk {
                id: RequestId::new(frame.request),
                bytes: buffer.clone(),
            },
        };
        if !greeted {
            match message {
                Frame::Notify(HostNotice::Hello) => {
                    greeted = true;
                    continue;
                }
                Frame::Reply { id, body } => {
                    services.deliver(id, body);
                    continue;
                }
                _ => {
                    logging::error!(target: "renderer::ipc", "message before handshake");
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
    services.close();
    let _result = command_tx.try_send(Frame::Notify(HostNotice::Shutdown));
}

/// Routes one post-handshake host message; `false` stops the read loop.
fn route_message(
    message: ToRenderer,
    command_tx: &exchange::Sender<ToRenderer>,
    services: &ChannelServices,
) -> bool {
    match message {
        Frame::Notify(HostNotice::Hello) => {
            logging::error!(target: "renderer::ipc", "duplicate handshake");
            false
        }
        Frame::Notify(HostNotice::Shutdown) => {
            let _result = command_tx.blocking_send(message);
            false
        }
        Frame::Reply { id, body } => {
            services.deliver(id, body);
            true
        }
        _ => command_tx.blocking_send(message).is_ok(),
    }
}

fn write_messages(
    rx: &exchange::BlockingReceiver<FromRenderer>,
    mut output: Box<dyn Write + Send>,
) -> io::Result<()> {
    while let Some(message) = rx.recv() {
        match message {
            Frame::RequestChunk { id, bytes } => {
                write_frame(&mut output, FrameKind::RequestChunk, id.get(), &bytes)?;
            }
            Frame::ResponseChunk { id, bytes } => {
                write_frame(&mut output, FrameKind::ResponseChunk, id.get(), &bytes)?;
            }
            _ => crate::wire::channel::write_control(&mut output, &message)?,
        }
    }
    Ok(())
}

/// [`BrowserServices`] proxy that asks the browser process over the channel.
pub(crate) struct ChannelServices {
    client: BlockingClient<BrowserCall, RendererReply, RendererNotice, ServiceReply>,
}

impl ChannelServices {
    fn new(out: exchange::BlockingSender<FromRenderer>) -> Self {
        Self {
            client: BlockingClient::new(out),
        }
    }

    fn call(&self, assignment: RendererAssignmentId, call: ServiceCall) -> Option<ServiceReply> {
        self.client.call(BrowserCall { assignment, call }).ok()
    }

    fn start_dial(
        &self,
        assignment: RendererAssignmentId,
        request: DialRequest,
        completion: DialCompletion,
    ) {
        self.client.call_with(
            BrowserCall {
                assignment,
                call: ServiceCall::Dial(request),
            },
            move |reply| match reply {
                Ok(ServiceReply::Dial(outcome)) => completion(outcome),
                Ok(
                    ServiceReply::Cookie(_)
                    | ServiceReply::StorageValue(_)
                    | ServiceReply::StorageKeys(_)
                    | ServiceReply::StorageChanged(_)
                    | ServiceReply::Window(_)
                    | ServiceReply::Unit,
                )
                | Err(_) => completion(Err(renderer::DialFailure::Connect)),
            },
        );
    }

    fn deliver(&self, id: RequestId, reply: ServiceReply) {
        self.client.deliver(id, reply);
    }

    fn close(&self) {
        self.client.close();
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
        self.channel
            .start_dial(self.assignment, request, completion);
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
