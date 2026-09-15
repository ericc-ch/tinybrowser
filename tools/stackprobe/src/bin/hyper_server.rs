use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::header::{CONNECTION, UPGRADE};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::json;
use std::convert::Infallible;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::protocol::Role;

async fn handle(request: Request<Incoming>) -> Result<Response<Full<Bytes>>, Infallible> {
    if request.uri().path() == "/devtools" && request.headers().contains_key(UPGRADE) {
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
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
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
