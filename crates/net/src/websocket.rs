use std::time::Instant;

use futures_util::SinkExt as _;
use futures_util::StreamExt as _;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::tungstenite::protocol::frame::Utf8Bytes;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use url::Url;

use crate::InitiatorKind;
use crate::client::Agent;
use crate::error::{NetError, ProtocolError, TimeoutKind, TransportError};
use crate::protocol::{HeaderMap, Method};

/// Open WebSocket. Dropping it closes the socket.
pub struct WebSocket {
    inner: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    budget: crate::transport::CallBudget,
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
    pub async fn send(&mut self, message: WsMessage) -> Result<(), NetError> {
        let message = match message {
            WsMessage::Text(text) => Message::Text(text.into()),
            WsMessage::Binary(bytes) => Message::Binary(bytes.into()),
        };
        self.write(message).await
    }

    /// Sends a close frame with the given code and reason.
    ///
    /// # Errors
    ///
    /// [`NetError::Transport`] on I/O failure. [`NetError::Protocol`] when the
    /// close frame cannot be written.
    pub async fn close(&mut self, code: u16, reason: &str) -> Result<(), NetError> {
        let frame = CloseFrame {
            code: CloseCode::from(code),
            reason: Utf8Bytes::from(reason),
        };
        self.write(Message::Close(Some(frame))).await?;
        self.inner.close(None).await.map_err(ws_err)
    }

    /// Next data or close event. Ping frames are answered with pong and skipped.
    ///
    /// # Errors
    ///
    /// [`NetError::Transport`] on I/O failure. [`NetError::Protocol`] when the
    /// frame stream is invalid.
    pub async fn take_next_message(&mut self) -> Result<WsEvent, NetError> {
        loop {
            let next = match self.budget.deadline() {
                Some(deadline) => match tokio::time::timeout_at(
                    tokio::time::Instant::from_std(deadline),
                    self.inner.next(),
                )
                .await
                {
                    Ok(next) => next,
                    Err(_) => {
                        return Err(NetError::Transport(TransportError::Timeout(
                            self.budget.timeout_kind(TimeoutKind::RecvBody),
                        )));
                    }
                },
                None => self.inner.next().await,
            };
            match next {
                Some(Ok(Message::Text(text))) => {
                    return Ok(WsEvent::Message(WsMessage::Text(text.to_string())));
                }
                Some(Ok(Message::Binary(bytes))) => {
                    return Ok(WsEvent::Message(WsMessage::Binary(bytes.to_vec())));
                }
                Some(Ok(Message::Ping(payload))) => {
                    self.write(Message::Pong(payload)).await?;
                }
                Some(Ok(Message::Pong(_) | Message::Frame(_))) => {}
                Some(Ok(Message::Close(frame))) => {
                    let (code, reason) = match frame {
                        Some(frame) => (u16::from(frame.code), frame.reason.to_string()),
                        None => (1005, String::new()),
                    };
                    let _result = self.inner.close(None).await;
                    return Ok(WsEvent::Close { code, reason });
                }
                Some(Err(error)) => return Err(ws_err(error)),
                None => {
                    return Ok(WsEvent::Close {
                        code: 1006,
                        reason: String::new(),
                    });
                }
            }
        }
    }

    async fn write(&mut self, message: Message) -> Result<(), NetError> {
        match self.budget.deadline() {
            Some(deadline) => match tokio::time::timeout_at(
                tokio::time::Instant::from_std(deadline),
                self.inner.send(message),
            )
            .await
            {
                Ok(result) => result.map_err(ws_err),
                Err(_) => Err(NetError::Transport(TransportError::Timeout(
                    self.budget.timeout_kind(TimeoutKind::SendBody),
                ))),
            },
            None => self.inner.send(message).await.map_err(ws_err),
        }
    }
}

/// Polls one stream item without pulling in `futures-util`.
pub(crate) async fn connect(
    agent: &Agent,
    url: &Url,
    headers: &HeaderMap,
    initiator_kind: InitiatorKind,
    method: &Method,
    initiator: Option<&Url>,
) -> Result<WebSocket, NetError> {
    let started = Instant::now();
    let budget = agent.engine.budget_at(started);
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|error| NetError::Protocol(ProtocolError::Other(error.to_string().into())))?;
    for (name, value) in headers.iter() {
        if is_websocket_reserved(name) {
            continue;
        }
        let header_name =
            tokio_tungstenite::tungstenite::http::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| NetError::Protocol(ProtocolError::RejectedRequest))?;
        let header_value = tokio_tungstenite::tungstenite::http::HeaderValue::from_bytes(value)
            .map_err(|_| NetError::Protocol(ProtocolError::RejectedRequest))?;
        request.headers_mut().insert(header_name, header_value);
    }
    let handshake = tokio_tungstenite::connect_async(request);
    let (ws, response) = match budget.deadline() {
        Some(deadline) => {
            match tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), handshake).await
            {
                Ok(result) => result.map_err(ws_err)?,
                Err(_) => {
                    return Err(NetError::Transport(TransportError::Timeout(
                        budget.timeout_kind(TimeoutKind::Connect),
                    )));
                }
            }
        }
        None => handshake.await.map_err(ws_err)?,
    };
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
            .filter_map(|value| value.to_str().ok()),
    );
    Ok(WebSocket { inner: ws, budget })
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

fn ws_err(error: tokio_tungstenite::tungstenite::Error) -> NetError {
    use tokio_tungstenite::tungstenite::Error as Ws;
    match error {
        Ws::Io(error) => {
            let tls = error
                .get_ref()
                .is_some_and(|inner| inner.downcast_ref::<native_tls::Error>().is_some());
            if tls || error.kind() == std::io::ErrorKind::InvalidData {
                NetError::Transport(TransportError::Tls(error.to_string().into()))
            } else {
                NetError::Transport(TransportError::Io(error))
            }
        }
        other => NetError::Protocol(ProtocolError::Other(other.to_string().into())),
    }
}
