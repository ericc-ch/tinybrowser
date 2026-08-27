//! WebSocket handle: handshake through [`crate::dial::open`], then the
//! caller owns the socket. [`WebSocket::send`] writes; [`WebSocket::take_next_message`]
//! reads (and answers pings).

use std::sync::Mutex;

use tungstenite::client::IntoClientRequest as _;
use tungstenite::handshake::HandshakeError;
use tungstenite::handshake::client::ClientHandshake;
use tungstenite::protocol::frame::Utf8Bytes;
use tungstenite::protocol::frame::coding::CloseCode;
use tungstenite::protocol::{CloseFrame, Message};
use url::Url;

use crate::Context;
use crate::agent::Agent;
use crate::cookie::{CookieOp, RetrievalKind};
use crate::dial::RawStream;
use crate::error::{NetError, ProtocolError, TransportError};
use crate::method::Method;

/// A live WebSocket after a successful upgrade. The TCP (or TLS) stream
/// lives here; there is no background thread.
pub struct WebSocket {
    inner: Mutex<tungstenite::WebSocket<RawStream>>,
}

/// One event from a read: a data message or the close handshake.
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
    /// Write one application message.
    ///
    /// # Errors
    ///
    /// [`NetError::Transport`] if the socket is already dead.
    pub fn send(&self, message: WsMessage) -> Result<(), NetError> {
        let msg = match message {
            WsMessage::Text(t) => Message::Text(t.into()),
            WsMessage::Binary(b) => Message::Binary(b.into()),
        };
        self.lock().send(msg).map_err(ws_err)
    }

    /// Initiate a close handshake (RFC 6455 §7.4).
    ///
    /// # Errors
    ///
    /// [`NetError::Transport`] if the socket is already dead.
    pub fn close(&self, code: u16, reason: &str) -> Result<(), NetError> {
        let frame = CloseFrame {
            code: CloseCode::from(code),
            reason: Utf8Bytes::from(reason),
        };
        self.lock().close(Some(frame)).map_err(ws_err)
    }

    /// Block until the next application message or close event.
    ///
    /// Control frames are handled here: a `Ping` is answered with `Pong`
    /// and the read continues. Idle connections wait; there is no 32-slot
    /// queue in front of the socket.
    ///
    /// # Errors
    ///
    /// [`NetError::Transport`] on a socket failure with no close frame.
    pub fn take_next_message(&self) -> Result<WsEvent, NetError> {
        let mut inner = self.lock();
        loop {
            match inner.read() {
                Ok(Message::Text(t)) => {
                    return Ok(WsEvent::Message(WsMessage::Text(t.to_string())));
                }
                Ok(Message::Binary(b)) => {
                    return Ok(WsEvent::Message(WsMessage::Binary(b.to_vec())));
                }
                Ok(Message::Ping(p)) => {
                    inner.send(Message::Pong(p)).map_err(ws_err)?;
                }
                Ok(Message::Pong(_) | Message::Frame(_)) => {}
                Ok(Message::Close(frame)) => {
                    let (code, reason) = match frame {
                        Some(f) => (u16::from(f.code), f.reason.to_string()),
                        None => (1005, String::new()),
                    };
                    let _ = inner.close(None);
                    return Ok(WsEvent::Close { code, reason });
                }
                Err(err) => return Err(ws_err(err)),
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, tungstenite::WebSocket<RawStream>> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Drop for WebSocket {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.inner.lock() {
            let _ = inner.close(Some(CloseFrame {
                code: CloseCode::Away,
                reason: Utf8Bytes::from(""),
            }));
        }
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
            stream
                .set_read_timeout(Some(limit))
                .map_err(|err| NetError::Transport(TransportError::Io(err)))?;
            stream
                .set_write_timeout(Some(limit))
                .map_err(|err| NetError::Transport(TransportError::Io(err)))?;
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
                cookie
                    .parse()
                    .map_err(|_| NetError::Protocol(ProtocolError::RejectedRequest))?,
            );
        }
        let (mut ws, response) = tungstenite::client(request, stream).map_err(handshake_err)?;
        harvest_ws_cookies(self, url, initiator, &response);
        // Handshake used the agent timeout. Live reads wait for a frame.
        let raw = ws.get_mut();
        let _ = raw.set_read_timeout(None);
        let _ = raw.set_write_timeout(None);
        Ok(WebSocket {
            inner: Mutex::new(ws),
        })
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
