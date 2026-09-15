use axum::Router;
use axum::extract::ws::{Message, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Json;
use axum::routing::get;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::Arc;

struct App {
    port: u16,
}

async fn version(State(app): State<Arc<App>>) -> Json<Value> {
    Json(json!({"Browser": "probe/0.1", "webSocketDebuggerUrl": format!("ws://127.0.0.1:{}/devtools", app.port)}))
}

async fn socket(ws: WebSocketUpgrade) -> impl axum::response::IntoResponse {
    ws.on_upgrade(async |mut socket| {
        while let Some(Ok(message)) = socket.recv().await {
            if let Message::Text(text) = message {
                let _ = socket.send(Message::Text(text)).await;
            }
        }
    })
}

#[tokio::main]
async fn main() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let app = Arc::new(App { port });
    let router = Router::new()
        .route("/json/version", get(version))
        .route("/devtools", get(socket))
        .with_state(app);
    axum::serve(listener, router).await.expect("serve");
    let _ = SocketAddr::from(([127, 0, 0, 1], 0));
}
