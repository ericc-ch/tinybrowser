//! WebSocket handle: handshake through [`crate::dial::open`], pump thread
//! after the 101.

use std::sync::mpsc::{self, RecvError, SyncSender};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use tungstenite::client::IntoClientRequest as _;
use tungstenite::handshake::client::ClientHandshake;
use tungstenite::handshake::HandshakeError;
use tungstenite::protocol::frame::coding::CloseCode;
use tungstenite::protocol::{CloseFrame, Message};
use url::Url;

use crate::agent::Agent;
use crate::cookie::{CookieOp, RetrievalKind};
use crate::dial::RawStream;
use crate::error::{NetError, ProtocolError, TransportError};
use crate::method::Method;
use crate::Context;

const EVENT_QUEUE: usize = 32;
const CMD_QUEUE: usize = 32;

/// A live WebSocket after a successful upgrade.
pub struct WebSocket {
    cmds: SyncSender<Cmd>,
    events: Mutex<mpsc::Receiver<WsEvent>>,
}

enum Cmd {
    Send(Message),
    Close { code: u16, reason: String },
}

/// One event from the pump: a data message or the close handshake.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WsEvent {
    /// Reassembled text or binary payload.
    Message(WsMessage),
    /// RFC 6455 §7.4 close code and reason.
    Close { code: u16, reason: String },
}

/// Application data on the WebSocket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WsMessage {
    Text(String),
    Binary(Vec<u8>),
}

impl WebSocket {
    /// Send an application message. Queued for the pump thread.
    ///
    /// # Errors
    ///
    /// [`NetError::Transport`] if the pump has already exited.
    pub fn send(&self, message: WsMessage) -> Result<(), NetError> {
        let msg = match message {
            WsMessage::Text(t) => Message::Text(t.into()),
            WsMessage::Binary(b) => Message::Binary(b.into()),
        };
        self.cmds
            .send(Cmd::Send(msg))
            .map_err(|_| NetError::Transport(TransportError::Io(std::io::Error::other("ws closed"))))
    }

    /// Initiate a close handshake (RFC 6455 §7.4).
    ///
    /// # Errors
    ///
    /// [`NetError::Transport`] if the pump has already exited.
    pub fn close(&self, code: u16, reason: &str) -> Result<(), NetError> {
        self.cmds
            .send(Cmd::Close {
                code,
                reason: reason.to_owned(),
            })
            .map_err(|_| NetError::Transport(TransportError::Io(std::io::Error::other("ws closed"))))
    }

    /// Block until the next application message or close event.
    ///
    /// # Errors
    ///
    /// [`NetError::Transport`] if the pump thread ended without a close event.
    pub fn take_next_message(&self) -> Result<WsEvent, NetError> {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .recv()
            .map_err(|RecvError { .. }| {
                NetError::Transport(TransportError::Io(std::io::Error::other("ws pump ended")))
            })
    }
}

impl Drop for WebSocket {
    fn drop(&mut self) {
        let _ = self.cmds.try_send(Cmd::Close {
            code: 1001,
            reason: String::new(),
        });
    }
}

impl Agent {
    /// Open a `ws://` or `wss://` connection through the shared dial path.
    ///
    /// Handshake uses [`Context::WsHandshake`]. Same-site is first-party
    /// unless [`Agent::websocket_from`] supplies a document URL.
    ///
    /// # Errors
    ///
    /// [`NetError::Protocol`] for a non-ws URL; transport/TLS failures as usual.
    pub fn websocket(&self, url: &Url) -> Result<WebSocket, NetError> {
        self.open_websocket(url, None)
    }

    /// Open a WebSocket using `initiator` as the document URL for `SameSite`.
    ///
    /// # Errors
    ///
    /// Same as [`Agent::websocket`].
    pub fn websocket_from(&self, url: &Url, initiator: &Url) -> Result<WebSocket, NetError> {
        self.open_websocket(url, Some(initiator))
    }

    fn open_websocket(&self, url: &Url, initiator: Option<&Url>) -> Result<WebSocket, NetError> {
        if !matches!(url.scheme(), "ws" | "wss") {
            return Err(NetError::Protocol(ProtocolError::RejectedRequest));
        }
        let stream = crate::dial::open(url, self.proxy.as_deref(), self.timeout)?;
        if let Some(limit) = self.timeout {
            stream.set_read_timeout(Some(limit)).map_err(|err| {
                NetError::Transport(TransportError::Io(err))
            })?;
            stream.set_write_timeout(Some(limit)).map_err(|err| {
                NetError::Transport(TransportError::Io(err))
            })?;
        }
        let cookie = self
            .jar
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .cookie_string(CookieOp {
                url,
                now: std::time::SystemTime::now(),
                kind: RetrievalKind::Http,
                context: Context::WsHandshake,
                method: &Method::GET,
                initiator,
            });
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|err| NetError::Protocol(ProtocolError::Other(err.to_string().into())))?;
        if !cookie.is_empty() {
            request.headers_mut().insert(
                tungstenite::http::header::COOKIE,
                cookie.parse().map_err(|_| {
                    NetError::Protocol(ProtocolError::RejectedRequest)
                })?,
            );
        }
        let (ws, response) = tungstenite::client(request, stream).map_err(handshake_err)?;
        harvest_ws_cookies(self, url, initiator, &response);
        Ok(start_pump(ws))
    }
}

fn harvest_ws_cookies(
    agent: &Agent,
    url: &Url,
    initiator: Option<&Url>,
    response: &tungstenite::http::Response<Option<Vec<u8>>>,
) {
    let now = std::time::SystemTime::now();
    let mut jar = agent
        .jar
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for value in response.headers().get_all("set-cookie") {
        if let Ok(text) = value.to_str() {
            jar.store(
                text,
                CookieOp {
                    url,
                    now,
                    kind: RetrievalKind::Http,
                    context: Context::WsHandshake,
                    method: &Method::GET,
                    initiator,
                },
            );
        }
    }
}

struct PumpIo {
    ws: tungstenite::WebSocket<RawStream>,
    cmds: mpsc::Receiver<Cmd>,
    events: SyncSender<WsEvent>,
}

fn start_pump(ws: tungstenite::WebSocket<RawStream>) -> WebSocket {
    let (cmd_tx, cmd_rx) = mpsc::sync_channel(CMD_QUEUE);
    let (ev_tx, ev_rx) = mpsc::sync_channel(EVENT_QUEUE);
    thread::spawn(move || {
        PumpIo {
            ws,
            cmds: cmd_rx,
            events: ev_tx,
        }
        .run();
    });
    WebSocket {
        cmds: cmd_tx,
        events: Mutex::new(ev_rx),
    }
}

impl PumpIo {
    fn run(mut self) {
        let _ = self
            .ws
            .get_mut()
            .set_read_timeout(Some(Duration::from_millis(25)));
        let mut sent_close = false;
        loop {
            match self.cmds.try_recv() {
                Ok(Cmd::Send(msg)) => {
                    if self.ws.send(msg).is_err() {
                        emit_abort(&self.events, &mut sent_close);
                        break;
                    }
                }
                Ok(Cmd::Close { code, reason }) => {
                    let frame = CloseFrame {
                        code: CloseCode::from(code),
                        reason: reason.into(),
                    };
                    if self.ws.close(Some(frame)).is_err() {
                        emit_abort(&self.events, &mut sent_close);
                        break;
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    emit_abort(&self.events, &mut sent_close);
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
            match self.ws.read() {
                Ok(Message::Text(t)) => {
                    if self
                        .events
                        .send(WsEvent::Message(WsMessage::Text(t.to_string())))
                        .is_err()
                    {
                        break;
                    }
                }
                Ok(Message::Binary(b)) => {
                    if self
                        .events
                        .send(WsEvent::Message(WsMessage::Binary(b.to_vec())))
                        .is_err()
                    {
                        break;
                    }
                }
                Ok(Message::Ping(p)) => {
                    let _ = self.ws.send(Message::Pong(p));
                }
                Ok(Message::Pong(_) | Message::Frame(_)) => {}
                Ok(Message::Close(frame)) => {
                    let (code, reason) = match frame {
                        Some(f) => (u16::from(f.code), f.reason.to_string()),
                        None => (1005, String::new()),
                    };
                    let _ = self.ws.close(None);
                    if !sent_close {
                        let _ = self.events.send(WsEvent::Close { code, reason });
                    }
                    break;
                }
                Err(tungstenite::Error::Io(err))
                    if err.kind() == std::io::ErrorKind::WouldBlock
                        || err.kind() == std::io::ErrorKind::TimedOut => {}
                Err(_) => {
                    emit_abort(&self.events, &mut sent_close);
                    break;
                }
            }
        }
    }
}

fn emit_abort(events: &SyncSender<WsEvent>, sent_close: &mut bool) {
    if *sent_close {
        return;
    }
    *sent_close = true;
    let _ = events.send(WsEvent::Close {
        code: 1006,
        reason: String::new(),
    });
}

fn handshake_err(err: HandshakeError<ClientHandshake<RawStream>>) -> NetError {
    match err {
        HandshakeError::Failure(err) => ws_err(err),
        HandshakeError::Interrupted(_) => {
            NetError::Protocol(ProtocolError::Other("ws handshake interrupted".into()))
        }
    }
}

fn ws_err(err: tungstenite::Error) -> NetError {
    match err {
        tungstenite::Error::Io(e) => NetError::Transport(TransportError::Io(e)),
        other => NetError::Protocol(ProtocolError::Other(other.to_string().into())),
    }
}
