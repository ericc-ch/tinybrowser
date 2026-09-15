use base64::Engine;
use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::header::{CONNECTION, SEC_WEBSOCKET_ACCEPT, SEC_WEBSOCKET_KEY, UPGRADE};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::json;
use sha1::{Digest, Sha1};
use std::convert::Infallible;
use tokio_tungstenite::tungstenite::protocol::Role;

/// RFC 6455 section 4.2.2: SHA-1 of the client key plus the fixed GUID,
/// base64-encoded.
fn accept_key(key: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(key.as_bytes());
    hasher.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    base64::engine::general_purpose::STANDARD.encode(hasher.finalize())
}

async fn handle(request: Request<Incoming>) -> Result<Response<Full<Bytes>>, Infallible> {
    let upgrade_requested = request.headers().contains_key(UPGRADE)
        && request.uri().path() == "/devtools";
    if upgrade_requested {
        let Some(key) = request
            .headers()
            .get(SEC_WEBSOCKET_KEY)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
        else {
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Full::new(Bytes::new()))
                .expect("bad request"));
        };
        tokio::spawn(async move {
            let Ok(upgraded) = hyper::upgrade::on(request).await else {
                return;
            };
            let mut socket = tokio_tungstenite::WebSocketStream::from_raw_socket(
                TokioIo::new(upgraded),
                Role::Server,
                None,
            )
            .await;
            use futures_util::{SinkExt, StreamExt};
            while let Some(Ok(message)) = socket.next().await {
                if message.is_text() || message.is_binary() {
                    let _ = socket.send(message).await;
                }
            }
        });
        return Ok(Response::builder()
            .status(StatusCode::SWITCHING_PROTOCOLS)
            .header(CONNECTION, "upgrade")
            .header(UPGRADE, "websocket")
            .header(SEC_WEBSOCKET_ACCEPT, accept_key(&key))
            .body(Full::new(Bytes::new()))
            .expect("upgrade response"));
    }
    let body = match request.uri().path() {
        "/json/version" => json!({"Browser": "probe/0.1"}).to_string(),
        _ => String::new(),
    };
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(body)))
        .expect("response"))
}

#[tokio::main]
async fn main() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    loop {
        let (stream, _) = listener.accept().await.expect("accept");
        let io = TokioIo::new(stream);
        tokio::spawn(async move {
            let _ = http1::Builder::new()
                .serve_connection(io, service_fn(handle))
                .with_upgrades()
                .await;
        });
    }
}
