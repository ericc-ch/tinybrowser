use std::sync::Mutex;
use std::time::{Duration, Instant};

use tungstenite::client::IntoClientRequest as _;
use tungstenite::handshake::HandshakeError;
use tungstenite::handshake::client::ClientHandshake;
use tungstenite::protocol::frame::Utf8Bytes;
use tungstenite::protocol::frame::coding::CloseCode;
use tungstenite::protocol::{CloseFrame, Message};
use url::Url;

use crate::InitiatorKind;
use crate::client::Agent;
use crate::error::{NetError, ProtocolError, TransportError};
use crate::protocol::{HeaderMap, Method};
use crate::transport::RawStream;

/// Open WebSocket. Dropping it sends close code 1001 (Going Away).
pub struct WebSocket {
    inner: Mutex<tungstenite::WebSocket<RawStream>>,
}

/// Incoming WebSocket event after control frames are handled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WsEvent {
    Message(WsMessage),
    /// Peer close. `code` is 1005 when the peer omitted a close frame.
    Close {
        code: u16,
        reason: String,
    },
}

/// WebSocket data frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WsMessage {
    Text(String),
    Binary(Vec<u8>),
}

impl WebSocket {
    /// Sends a data frame.
    ///
    /// # Errors
    ///
    /// [`NetError::Transport`] on I/O failure. [`NetError::Protocol`] when the
    /// frame cannot be encoded.
    pub fn send(&self, message: WsMessage) -> Result<(), NetError> {
        let msg = match message {
            WsMessage::Text(t) => Message::Text(t.into()),
            WsMessage::Binary(b) => Message::Binary(b.into()),
        };
        self.lock().send(msg).map_err(ws_err)
    }

    /// Sends a close frame with the given code and reason.
    ///
    /// # Errors
    ///
    /// [`NetError::Transport`] on I/O failure. [`NetError::Protocol`] when the
    /// close frame cannot be written.
    pub fn close(&self, code: u16, reason: &str) -> Result<(), NetError> {
        let frame = CloseFrame {
            code: CloseCode::from(code),
            reason: Utf8Bytes::from(reason),
        };
        self.lock().close(Some(frame)).map_err(ws_err)
    }

    /// Next data or close event. Ping frames are answered with pong and skipped.
    ///
    /// # Errors
    ///
    /// [`NetError::Transport`] on I/O failure. [`NetError::Protocol`] when the
    /// frame stream is invalid.
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
            let raw = inner.get_mut();
            let _ = raw.set_write_timeout(Some(Duration::from_secs(1)));
            let _ = inner.close(Some(CloseFrame {
                code: CloseCode::Away,
                reason: Utf8Bytes::from(""),
            }));
        }
    }
}

pub(crate) fn connect(
    agent: &Agent,
    url: &Url,
    headers: &HeaderMap,
    initiator_kind: InitiatorKind,
    method: &Method,
    initiator: Option<&Url>,
) -> Result<WebSocket, NetError> {
    let started = Instant::now();
    let budget = agent.engine.budget_at(started);
    let _guard = crate::transport::enter_budget(budget);
    let stream = crate::transport::open(
        url,
        agent.engine.proxy.as_deref(),
        budget.deadline(),
        &agent.engine.host_map,
    )?;
    if let Some(limit) = budget.remaining() {
        stream
            .set_read_timeout(Some(limit))
            .map_err(|err| NetError::Transport(TransportError::Io(err)))?;
        stream
            .set_write_timeout(Some(limit))
            .map_err(|err| NetError::Transport(TransportError::Io(err)))?;
    }
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|err| NetError::Protocol(ProtocolError::Other(err.to_string().into())))?;
    for (name, value) in headers.iter() {
        if is_websocket_reserved(name) {
            continue;
        }
        let header_name = tungstenite::http::HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| NetError::Protocol(ProtocolError::RejectedRequest))?;
        let header_value = tungstenite::http::HeaderValue::from_bytes(value)
            .map_err(|_| NetError::Protocol(ProtocolError::RejectedRequest))?;
        request.headers_mut().insert(header_name, header_value);
    }
    let (mut ws, response) = tungstenite::client(request, stream).map_err(handshake_err)?;
    agent.store_set_cookie_lines(
        url,
        initiator_kind,
        method,
        initiator,
        false,
        response
            .headers()
            .get_all("set-cookie")
            .into_iter()
            .filter_map(|v| v.to_str().ok()),
    );
    let raw = ws.get_mut();
    let _ = raw.set_read_timeout(None);
    let _ = raw.set_write_timeout(None);
    Ok(WebSocket {
        inner: Mutex::new(ws),
    })
}

fn is_websocket_reserved(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "upgrade"
            | "sec-websocket-key"
            | "sec-websocket-version"
            | "sec-websocket-extensions"
    )
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
