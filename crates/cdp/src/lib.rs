//! CDP adapter over [`browser::BrowserHandle`].
//!
//! [ADR 0009](../../../docs/adrs/0009-named-profile-daemon.md): honest first
//! subsets of Browser, Target, Page, and Runtime. Unsupported methods return
//! method-not-found. Flattened `sessionId` routing on the browser socket.
//! [ADR 0012](../../../docs/adrs/0012-host-protocol-and-cli-stack.md): axum
//! serves the loopback HTTP and WebSocket endpoints. The synchronous
//! [`Client`] used by the CLI and tests stays on tungstenite.

use std::collections::{HashMap, VecDeque};
use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use browser::{BrowserHandle, PageError, PageEvent, PageHandle, PageId, RemoteValue};
use serde_json::{Value, json};
use tokio::sync::watch;
use tungstenite::client::IntoClientRequest;
use tungstenite::protocol::{Message, WebSocket as ClientSocket};

const PRODUCT: &str = "tinybrowser/0.1.0";
const EVENT_POLL: Duration = Duration::from_millis(20);

/// Serves CDP HTTP discovery and WebSocket endpoints on `listener`.
///
/// Returns when [`browser::BrowserHandle::close`] runs through `Browser.close`.
///
/// # Errors
///
/// Returns when the listener cannot be converted or serving fails.
pub fn serve(listener: &TcpListener, browser: &BrowserHandle) -> io::Result<()> {
    let bound = listener.local_addr()?;
    let std_listener = listener.try_clone()?;
    std_listener.set_nonblocking(true)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let browser = browser.clone();
    let result: io::Result<()> = runtime.block_on(async move {
        let listener = tokio::net::TcpListener::from_std(std_listener)?;
        let (stop, mut stopping) = watch::channel(false);
        let state = AppState {
            browser,
            bound,
            stop: stop.clone(),
        };
        let app = Router::new()
            .route(
                "/json/version",
                get(|State(state): State<AppState>| async move { Json(version_json(&state)) }),
            )
            .route("/json", get(discovery))
            .route("/json/list", get(discovery))
            .route(
                "/devtools/browser",
                get(
                    |ws: WebSocketUpgrade, State(state): State<AppState>| async move {
                        ws.on_upgrade(move |socket| run_socket(socket, state, None))
                    },
                ),
            )
            .route(
                "/devtools/page/{id}",
                get(
                    |Path(raw): Path<String>,
                     ws: WebSocketUpgrade,
                     State(state): State<AppState>| async move {
                        let Ok(id) = raw.parse::<u64>() else {
                            return (StatusCode::BAD_REQUEST, "invalid page target")
                                .into_response();
                        };
                        match state.browser.page(PageId::new(id)) {
                            Ok(page) => {
                                ws.on_upgrade(move |socket| run_socket(socket, state, Some(page)))
                            }
                            Err(_) => {
                                (StatusCode::NOT_FOUND, "unknown page target").into_response()
                            }
                        }
                    },
                ),
            )
            .layer(middleware::from_fn(strip_trailing_slash))
            .fallback(|| async { (StatusCode::NOT_FOUND, "not found") })
            .with_state(state);
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _result = stopping.wait_for(|stopping| *stopping).await;
            })
            .await?;
        Ok(())
    });
    result
}

#[derive(Clone)]
struct AppState {
    browser: BrowserHandle,
    bound: SocketAddr,
    stop: watch::Sender<bool>,
}

/// Discovery walks page actors; keep it off the async workers ([ADR 0012]).
///
/// [ADR 0012]: ../../../docs/adrs/0012-host-protocol-and-cli-stack.md
async fn discovery(State(state): State<AppState>) -> Response {
    match tokio::task::spawn_blocking(move || list_json(&state)).await {
        Ok(value) => Json(value).into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "discovery failed").into_response(),
    }
}

/// Legacy CDP clients append a trailing slash; normalize before routing.
async fn strip_trailing_slash(mut request: Request, next: Next) -> Response {
    let uri = request.uri().clone();
    let path = uri.path();
    if path.len() > 1 && path.ends_with('/') {
        let normalized = match uri.query() {
            Some(query) => format!("{}?{query}", path.trim_end_matches('/')),
            None => path.trim_end_matches('/').to_owned(),
        };
        if let Ok(parsed) = normalized.parse() {
            *request.uri_mut() = parsed;
        }
    }
    next.run(request).await
}

fn version_json(state: &AppState) -> Value {
    json!({
        "Browser": PRODUCT,
        "Protocol-Version": "1.3",
        "webSocketDebuggerUrl": format!("ws://{}/devtools/browser", state.bound),
    })
}

fn list_json(state: &AppState) -> Value {
    let mut targets = Vec::new();
    for id in state.browser.pages() {
        let url = state
            .browser
            .page(id)
            .ok()
            .and_then(|page| page.document_url().ok())
            .unwrap_or_else(|| "about:blank".into());
        targets.push(json!({
            "id": id.to_string(),
            "type": "page",
            "url": url,
            "webSocketDebuggerUrl": format!("ws://{}/devtools/page/{id}", state.bound),
        }));
    }
    Value::Array(targets)
}

/// Client for the browser WebSocket.
pub struct Client {
    socket: ClientSocket<TcpStream>,
    next_id: i64,
    events: VecDeque<Value>,
}

impl Client {
    /// Connects to `ws://host:port/devtools/browser`.
    ///
    /// # Errors
    ///
    /// Transport or handshake failure.
    pub fn connect(addr: SocketAddr) -> io::Result<Self> {
        connect_ws(addr, "/devtools/browser")
    }

    /// Connects to `ws://host:port{path}`.
    ///
    /// # Errors
    ///
    /// Transport or handshake failure.
    pub fn connect_path(addr: SocketAddr, path: &str) -> io::Result<Self> {
        connect_ws(addr, path)
    }

    /// Sends `method` and waits for the matching id.
    ///
    /// # Errors
    ///
    /// Transport failure or a CDP error object.
    pub fn call(
        &mut self,
        method: &str,
        params: &Value,
        session_id: Option<&str>,
    ) -> io::Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let mut message = json!({
            "id": id,
            "method": method,
            "params": params,
        });
        if let Some(session) = session_id
            && let Some(object) = message.as_object_mut()
        {
            object.insert("sessionId".into(), json!(session));
        }
        self.socket
            .send(Message::Text(message.to_string().into()))
            .map_err(ws_io)?;
        loop {
            let incoming = self.socket.read().map_err(ws_io)?;
            let Message::Text(text) = incoming else {
                continue;
            };
            let parsed: Value = serde_json::from_str(&text).map_err(json_io)?;
            if parsed.get("id") == Some(&json!(id)) {
                if let Some(error) = parsed.get("error") {
                    return Err(io::Error::other(error.to_string()));
                }
                return Ok(parsed.get("result").cloned().unwrap_or(Value::Null));
            }
            if parsed.get("method").is_some() {
                self.events.push_back(parsed);
            }
        }
    }

    /// Reads the next protocol event, returning `None` when `timeout` elapses.
    ///
    /// # Errors
    ///
    /// Transport or JSON decoding failure.
    pub fn read_event(&mut self, timeout: Duration) -> io::Result<Option<Value>> {
        if let Some(event) = self.events.pop_front() {
            return Ok(Some(event));
        }
        self.socket.get_mut().set_read_timeout(Some(timeout))?;
        let result = loop {
            match self.socket.read() {
                Ok(Message::Text(text)) => {
                    let parsed: Value = match serde_json::from_str(&text).map_err(json_io) {
                        Ok(parsed) => parsed,
                        Err(error) => break Err(error),
                    };
                    if parsed.get("method").is_some() {
                        break Ok(Some(parsed));
                    }
                }
                Ok(Message::Ping(payload)) => {
                    if let Err(error) = self.socket.send(Message::Pong(payload)).map_err(ws_io) {
                        break Err(error);
                    }
                }
                Ok(Message::Close(_)) => break Ok(None),
                Err(tungstenite::Error::Io(error))
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    break Ok(None);
                }
                Err(error) => break Err(ws_io(error)),
                Ok(_) => {}
            }
        };
        let reset = self
            .socket
            .get_mut()
            .set_read_timeout(Some(Duration::from_secs(30)));
        match result {
            Err(error) => Err(error),
            Ok(event) => {
                reset?;
                Ok(event)
            }
        }
    }
}

fn connect_ws(addr: SocketAddr, path: &str) -> io::Result<Client> {
    let stream = TcpStream::connect(addr)?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    let url = format!("ws://{addr}{path}");
    let request = url
        .as_str()
        .into_client_request()
        .map_err(|error| io::Error::other(error.to_string()))?;
    let (socket, _response) = tungstenite::client::client(request, stream)
        .map_err(|error| io::Error::other(error.to_string()))?;
    Ok(Client {
        socket,
        next_id: 0,
        events: VecDeque::new(),
    })
}

/// Per-WebSocket protocol state. Commands dispatch synchronously against
/// [`PageHandle`]; the socket loop is async.
struct Conn {
    browser: BrowserHandle,
    sessions: HashMap<String, PageHandle>,
    next_session: u64,
    page: Option<PageHandle>,
    stop: watch::Sender<bool>,
    subscriptions: Vec<PageSubscription>,
    clock_origin: Instant,
}

struct PageSubscription {
    page_id: PageId,
    session: Option<String>,
    events: Receiver<PageEvent>,
}

/// One text reply plus whether the socket closes after it.
struct Outcome {
    reply: Value,
    close: bool,
}

impl Conn {
    /// Page events accumulated since the last flush, each ready to send.
    fn take_event_messages(&self) -> Vec<Value> {
        let mut messages = Vec::new();
        for subscription in &self.subscriptions {
            for event in subscription.events.try_iter() {
                if event == PageEvent::Load {
                    let mut message = json!({
                        "method": "Page.loadEventFired",
                        "params": {"timestamp": self.clock_origin.elapsed().as_secs_f64()},
                    });
                    attach_session(&mut message, subscription.session.as_deref());
                    messages.push(message);
                }
            }
        }
        messages
    }

    fn dispatch_text(&mut self, text: &str) -> Outcome {
        let parsed: Value = match serde_json::from_str(text) {
            Ok(value) => value,
            Err(err) => {
                return Outcome {
                    reply: json!({
                        "id": Value::Null,
                        "error": {"code": -32700, "message": err.to_string()},
                    }),
                    close: false,
                };
            }
        };
        let id = parsed.get("id").cloned().unwrap_or(Value::Null);
        let Some(method) = parsed.get("method").and_then(Value::as_str) else {
            return Outcome {
                reply: json!({
                    "id": id,
                    "error": {"code": -32600, "message": "missing method"},
                }),
                close: false,
            };
        };
        let empty = json!({});
        let params = parsed.get("params").unwrap_or(&empty);
        let session = parsed.get("sessionId").and_then(Value::as_str);
        match self.dispatch(method, params, session) {
            Ok(result) => {
                let mut reply = json!({"id": id, "result": result});
                attach_session(&mut reply, session);
                Outcome {
                    reply,
                    close: method == "Browser.close",
                }
            }
            Err(DispatchError::MethodNotFound) => {
                let mut reply = json!({
                    "id": id,
                    "error": {
                        "code": -32601,
                        "message": format!("'{method}' wasn't found")
                    },
                });
                attach_session(&mut reply, session);
                Outcome {
                    reply,
                    close: false,
                }
            }
            Err(DispatchError::Failed(message)) => {
                let mut reply = json!({
                    "id": id,
                    "error": {"code": -32000, "message": message},
                });
                attach_session(&mut reply, session);
                Outcome {
                    reply,
                    close: false,
                }
            }
        }
    }

    fn dispatch(
        &mut self,
        method: &str,
        params: &Value,
        session: Option<&str>,
    ) -> Result<Value, DispatchError> {
        if method != "Browser.close" && !self.browser.is_live() {
            return Err(DispatchError::Failed("browser closed".into()));
        }
        if let Some(session) = session {
            let page = self
                .sessions
                .get(session)
                .cloned()
                .ok_or_else(|| DispatchError::Failed("unknown session".into()))?;
            return self.dispatch_page_method(method, params, &page, Some(session));
        }
        if let Some(page) = self.page.clone() {
            return self.dispatch_page_method(method, params, &page, None);
        }
        match method {
            "Browser.getVersion" => Ok(json!({
                "protocolVersion": "1.3",
                "product": PRODUCT,
                "revision": "0",
                "userAgent": PRODUCT,
                "jsVersion": "QuickJS",
            })),
            "Browser.close" => {
                self.browser
                    .close()
                    .map_err(|error| DispatchError::Failed(error.to_string()))?;
                self.sessions.clear();
                self.subscriptions.clear();
                self.page = None;
                let _result = self.stop.send(true);
                Ok(json!({}))
            }
            "Target.getTargets" => {
                let target_infos: Vec<Value> = self
                    .browser
                    .pages()
                    .into_iter()
                    .map(|id| target_info(&self.browser, id))
                    .collect();
                Ok(json!({ "targetInfos": target_infos }))
            }
            "Target.createTarget" => {
                let url = params
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or("about:blank");
                let page = self
                    .browser
                    .create_page()
                    .map_err(|err| DispatchError::Failed(err.to_string()))?;
                if let Err(error) = open_url(&page, url) {
                    let _ = self.browser.close_page(page.id());
                    return Err(error);
                }
                Ok(json!({ "targetId": page.id().to_string() }))
            }
            "Target.closeTarget" => {
                let id = target_id(params.get("targetId"))?;
                self.browser
                    .close_page(id)
                    .map_err(|err| DispatchError::Failed(err.to_string()))?;
                self.sessions.retain(|_, page| page.id() != id);
                self.subscriptions.retain(|item| item.page_id != id);
                if self.page.as_ref().is_some_and(|page| page.id() == id) {
                    self.page = None;
                }
                Ok(json!({ "success": true }))
            }
            "Target.attachToTarget" => {
                let id = target_id(params.get("targetId"))?;
                let flatten = params
                    .get("flatten")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if !flatten {
                    return Err(DispatchError::Failed(
                        "attachToTarget requires flatten".into(),
                    ));
                }
                let page = self
                    .browser
                    .page(id)
                    .map_err(|err| DispatchError::Failed(err.to_string()))?;
                let session_id = format!("s{}", self.next_session);
                self.next_session = self.next_session.saturating_add(1);
                self.sessions.insert(session_id.clone(), page);
                Ok(json!({ "sessionId": session_id }))
            }
            "Target.detachFromTarget" => {
                if let Some(session) = params.get("sessionId").and_then(Value::as_str) {
                    self.sessions.remove(session);
                    self.subscriptions
                        .retain(|item| item.session.as_deref() != Some(session));
                }
                Ok(json!({}))
            }
            _ => Err(DispatchError::MethodNotFound),
        }
    }

    fn dispatch_page_method(
        &mut self,
        method: &str,
        params: &Value,
        page: &PageHandle,
        session: Option<&str>,
    ) -> Result<Value, DispatchError> {
        match method {
            "Page.enable" => {
                self.subscribe_page(page, session)?;
                Ok(json!({}))
            }
            "Page.disable" => {
                self.unsubscribe_page(page.id(), session);
                Ok(json!({}))
            }
            _ => session_method(method, params, page),
        }
    }

    fn subscribe_page(
        &mut self,
        page: &PageHandle,
        session: Option<&str>,
    ) -> Result<(), DispatchError> {
        if self
            .subscriptions
            .iter()
            .any(|item| item.page_id == page.id() && item.session.as_deref() == session)
        {
            return Ok(());
        }
        let events = page
            .subscribe()
            .map_err(|error| DispatchError::Failed(error.to_string()))?;
        self.subscriptions.push(PageSubscription {
            page_id: page.id(),
            session: session.map(str::to_owned),
            events,
        });
        Ok(())
    }

    fn unsubscribe_page(&mut self, page_id: PageId, session: Option<&str>) {
        self.subscriptions
            .retain(|item| item.page_id != page_id || item.session.as_deref() != session);
    }
}

async fn run_socket(mut socket: WebSocket, state: AppState, page: Option<PageHandle>) {
    let mut conn = Conn {
        browser: state.browser,
        sessions: HashMap::new(),
        next_session: 1,
        page,
        stop: state.stop,
        subscriptions: Vec::new(),
        clock_origin: Instant::now(),
    };
    let mut poll = tokio::time::interval(EVENT_POLL);
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        let outcome = tokio::select! {
            _ = poll.tick() => None,
            incoming = socket.recv() => match incoming {
                Some(Ok(WsMessage::Text(text))) => {
                    let text = text.as_str().to_owned();
                    Some(tokio::task::block_in_place(|| conn.dispatch_text(&text)))
                }
                Some(Ok(WsMessage::Ping(payload))) => {
                    let _ = socket.send(WsMessage::Pong(payload)).await;
                    None
                }
                None | Some(Ok(WsMessage::Close(_)) | Err(_)) => break,
                Some(Ok(_)) => None,            },
        };
        let mut failed = false;
        for message in conn.take_event_messages() {
            if socket
                .send(WsMessage::text(message.to_string()))
                .await
                .is_err()
            {
                failed = true;
                break;
            }
        }
        if failed {
            break;
        }
        let Some(outcome) = outcome else {
            continue;
        };
        let close = outcome.close;
        if socket
            .send(WsMessage::text(outcome.reply.to_string()))
            .await
            .is_err()
        {
            break;
        }
        if close {
            break;
        }
    }
    let _ = socket.send(WsMessage::Close(None)).await;
}

fn session_method(method: &str, params: &Value, page: &PageHandle) -> Result<Value, DispatchError> {
    match method {
        "Runtime.enable" | "Runtime.disable" => Ok(json!({})),
        "Page.navigate" => {
            let url = params
                .get("url")
                .and_then(Value::as_str)
                .ok_or_else(|| DispatchError::Failed("missing url".into()))?;
            open_url(page, url)?;
            Ok(json!({ "frameId": page.id().to_string() }))
        }
        "Runtime.evaluate" => {
            let expression = params
                .get("expression")
                .and_then(Value::as_str)
                .ok_or_else(|| DispatchError::Failed("missing expression".into()))?;
            match page.execute_script(expression) {
                Ok(value) => Ok(json!({ "result": remote_preview(&value) })),
                Err(PageError::ActorStopped) => {
                    Err(DispatchError::Failed("page actor stopped".into()))
                }
                Err(err) => Ok(json!({
                    "result": {"type": "undefined"},
                    "exceptionDetails": {"text": err.to_string()},
                })),
            }
        }
        _ => Err(DispatchError::MethodNotFound),
    }
}

fn open_url(page: &PageHandle, url: &str) -> Result<(), DispatchError> {
    if url.is_empty() || url == "about:blank" {
        page.load_html("<!doctype html><title></title>")
            .map_err(|err| DispatchError::Failed(err.to_string()))?;
        return Ok(());
    }
    page.goto(url)
        .map_err(|err| DispatchError::Failed(err.to_string()))?;
    Ok(())
}

fn target_info(browser: &BrowserHandle, id: PageId) -> Value {
    let url = browser
        .page(id)
        .ok()
        .and_then(|page| page.document_url().ok())
        .unwrap_or_else(|| "about:blank".into());
    json!({
        "targetId": id.to_string(),
        "type": "page",
        "title": "",
        "url": url,
        "attached": false,
        "canAccessOpener": false,
    })
}

fn target_id(value: Option<&Value>) -> Result<PageId, DispatchError> {
    let raw = value
        .and_then(Value::as_str)
        .ok_or_else(|| DispatchError::Failed("missing targetId".into()))?;
    let id = raw
        .parse::<u64>()
        .map_err(|_| DispatchError::Failed("invalid targetId".into()))?;
    Ok(PageId::new(id))
}

fn remote_preview(value: &RemoteValue) -> Value {
    match value {
        RemoteValue::Undefined => json!({"type": "undefined"}),
        RemoteValue::Null => json!({"type": "object", "subtype": "null", "value": null}),
        RemoteValue::Bool(flag) => json!({"type": "boolean", "value": flag}),
        RemoteValue::Number(number) => json!({"type": "number", "value": number}),
        RemoteValue::String(text) => json!({"type": "string", "value": text}),
        RemoteValue::List(_) => json!({"type": "object", "subtype": "array"}),
        RemoteValue::Map(_) => json!({"type": "object"}),
        RemoteValue::Node(_) => json!({"type": "object", "subtype": "node"}),
    }
}

enum DispatchError {
    MethodNotFound,
    Failed(String),
}

fn attach_session(reply: &mut Value, session: Option<&str>) {
    if let Some(session) = session
        && let Some(object) = reply.as_object_mut()
    {
        object.insert("sessionId".into(), json!(session));
    }
}

fn ws_io(err: tungstenite::Error) -> io::Error {
    io::Error::other(err)
}

fn json_io(err: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err)
}
