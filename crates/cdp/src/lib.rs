//! CDP adapter over [`browser::BrowserHandle`].
//!
//! [ADR 0009](../../../docs/adrs/0009-named-profile-daemon.md): honest first
//! subsets of Browser, Target, Page, and Runtime. Unsupported methods return
//! method-not-found. Flattened `sessionId` routing on the browser socket.
//! [ADR 0012](../../../docs/adrs/0012-host-protocol-and-cli-stack.md): axum
//! serves the loopback HTTP and WebSocket endpoints. The synchronous
//! [`Client`] used by the CLI and tests stays on tungstenite.
//!
//! Names: the wire object is a CDP **page target** (`"type": "page"`,
//! `Page.*`); it maps to a [`browser::TabHandle`]. CDP's experimental **tab
//! target** is the browser-UI container and is not modeled
//! ([ADR 0013](../../../docs/adrs/0013-vocabulary-and-process-names.md)).

use std::collections::{HashMap, VecDeque};
use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use browser::{BrowserHandle, RemoteValue, TabError, TabEvent, TabHandle, TabId};
use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tungstenite::client::IntoClientRequest;
use tungstenite::protocol::{Message, WebSocket as ClientSocket};

const PRODUCT: &str = "tinybrowser/0.1.0";
/// One default browser context; Playwright requires `browserContextId` on
/// attached targets.
const DEFAULT_BROWSER_CONTEXT_ID: &str = "tinybrowser-default";

/// Serves CDP HTTP discovery and WebSocket endpoints on `listener`.
///
/// Returns when [`browser::BrowserHandle::close`] runs through `Browser.close`.
///
/// # Errors
///
/// Returns when the listener cannot be converted or serving fails.
pub async fn serve(listener: &TcpListener, browser: &BrowserHandle) -> io::Result<()> {
    let bound = listener.local_addr()?;
    let std_listener = listener.try_clone()?;
    std_listener.set_nonblocking(true)?;
    let browser = browser.clone();
    let listener = tokio::net::TcpListener::from_std(std_listener)?;
    let (stop, mut stopping) = watch::channel(false);
    let state = AppState {
        browser,
        bound,
        stop: stop.clone(),
    };
    let version = get(|State(state): State<AppState>| async move { Json(version_json(&state)) });
    let discovery = get(discovery);
    let browser_socket = get(
        |ws: WebSocketUpgrade, State(state): State<AppState>| async move {
            ws.on_upgrade(move |socket| run_socket(socket, state, None))
        },
    );
    let page_socket = get(
        |Path(raw): Path<String>, ws: WebSocketUpgrade, State(state): State<AppState>| async move {
            let Ok(id) = raw.parse::<u64>() else {
                return (StatusCode::BAD_REQUEST, "invalid page target").into_response();
            };
            match state.browser.tab(TabId::new(id)).await {
                Ok(tab) => ws.on_upgrade(move |socket| run_socket(socket, state, Some(tab))),
                Err(_) => (StatusCode::NOT_FOUND, "unknown page target").into_response(),
            }
        },
    );
    // Legacy CDP clients (Playwright included) append a trailing slash to
    // discovery URLs; register both spellings rather than relying on a
    // middleware layer, which axum does not apply to fallbacks.
    let app = Router::new()
        .route("/json/version", version.clone())
        .route("/json/version/", version)
        .route("/json", discovery.clone())
        .route("/json/", discovery.clone())
        .route("/json/list", discovery.clone())
        .route("/json/list/", discovery)
        .route("/devtools/browser", browser_socket.clone())
        .route("/devtools/browser/", browser_socket)
        .route("/devtools/page/{id}", page_socket.clone())
        .route("/devtools/page/{id}/", page_socket)
        .fallback(|| async { (StatusCode::NOT_FOUND, "not found") })
        .with_state(state);
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _result = stopping.wait_for(|stopping| *stopping).await;
        })
        .await
}

#[derive(Clone)]
struct AppState {
    browser: BrowserHandle,
    bound: SocketAddr,
    stop: watch::Sender<bool>,
}

async fn discovery(State(state): State<AppState>) -> Response {
    Json(list_json(&state).await).into_response()
}

fn version_json(state: &AppState) -> Value {
    json!({
        "Browser": PRODUCT,
        "Protocol-Version": "1.3",
        "webSocketDebuggerUrl": format!("ws://{}/devtools/browser", state.bound),
    })
}

async fn list_json(state: &AppState) -> Value {
    let mut targets = Vec::new();
    for id in state.browser.tabs().await.unwrap_or_default() {
        let url = match state.browser.tab(id).await {
            Ok(tab) => tab
                .document_url()
                .await
                .unwrap_or_else(|_| "about:blank".into()),
            Err(_) => "about:blank".into(),
        };
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

/// Per-WebSocket async protocol state.
struct Conn {
    browser: BrowserHandle,
    sessions: HashMap<String, TabHandle>,
    next_session: u64,
    tab: Option<TabHandle>,
    stop: watch::Sender<bool>,
    subscriptions: Vec<TabSubscription>,
    tab_events_tx: mpsc::Sender<ConnEvent>,
    tab_events_rx: mpsc::Receiver<ConnEvent>,
    clock_origin: Instant,
    auto_attach: bool,
    events: Vec<Value>,
    loader_ids: HashMap<TabId, String>,
    isolated_worlds: HashMap<TabId, Vec<String>>,
    next_loader: u64,
    next_context: u64,
    next_handle: u64,
}

struct TabSubscription {
    tab_id: TabId,
    session: Option<String>,
    forward: JoinHandle<()>,
}

struct ConnEvent {
    tab: TabHandle,
    session: Option<String>,
    event: TabEvent,
}

/// One text reply plus whether the socket closes after it.
struct Outcome {
    reply: Value,
    close: bool,
}

impl Conn {
    fn take_queued_events(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.events)
    }

    async fn event_messages(&mut self, incoming: ConnEvent) -> Vec<Value> {
        let mut messages = Vec::new();
        let timestamp = self.clock_origin.elapsed().as_secs_f64();
        let tab = incoming.tab;
        let session = incoming.session;
        match incoming.event {
            TabEvent::Navigated => {
                self.push_navigated(&mut messages, &tab, session.as_deref())
                    .await;
            }
            TabEvent::Load => {
                let frame_id = tab.id().to_string();
                let loader_id = self.loader_ids.get(&tab.id()).cloned().unwrap_or_default();
                let mut lifecycle = json!({
                    "method": "Page.lifecycleEvent",
                    "params": {
                        "frameId": frame_id,
                        "loaderId": loader_id,
                        "name": "load",
                        "timestamp": timestamp,
                    },
                });
                attach_session(&mut lifecycle, session.as_deref());
                messages.push(lifecycle);
                let mut load = json!({
                    "method": "Page.loadEventFired",
                    "params": {"timestamp": timestamp},
                });
                attach_session(&mut load, session.as_deref());
                messages.push(load);
            }
            _ => {}
        }
        messages
    }

    /// Emits the commit event set: frame commit plus the new document's
    /// execution contexts (default and every known isolated world).
    async fn push_navigated(
        &mut self,
        messages: &mut Vec<Value>,
        tab: &TabHandle,
        session: Option<&str>,
    ) {
        let tab_id = tab.id();
        let frame_id = tab_id.to_string();
        let loader_id = self.loader_ids.get(&tab_id).cloned().unwrap_or_default();
        // Final URL after redirects, not the requested one.
        let url = tab.document_url().await.unwrap_or_default();
        let mut navigated = json!({
            "method": "Page.frameNavigated",
            "params": {"frame": {
                "id": frame_id,
                "loaderId": loader_id,
                "url": url,
                "mimeType": "text/html",
            }},
        });
        attach_session(&mut navigated, session);
        messages.push(navigated);
        // The new document is a new realm: drop announced contexts so clients
        // (Playwright) re-create their utility world.
        let mut cleared = json!({
            "method": "Runtime.executionContextsCleared",
            "params": {},
        });
        attach_session(&mut cleared, session);
        messages.push(cleared);
        let mut worlds = vec![String::new()];
        if let Some(isolated) = self.isolated_worlds.get(&tab_id) {
            worlds.extend(isolated.iter().cloned());
        }
        for world in worlds {
            let context_id = self.next_context;
            self.next_context = self.next_context.saturating_add(1);
            let default = world.is_empty();
            let mut created = json!({
                "method": "Runtime.executionContextCreated",
                "params": {"context": {
                    "id": context_id,
                    "origin": "://",
                    "name": world,
                    "uniqueId": format!("ctx-{context_id}"),
                    "auxData": {
                        "isDefault": default,
                        "type": if default { "default" } else { "isolated" },
                        "frameId": frame_id,
                    },
                }},
            });
            attach_session(&mut created, session);
            messages.push(created);
        }
    }

    /// Resolves the requested target (or the first live one) for `Target.getTargetInfo`.
    async fn target_info_for(&mut self, params: &Value) -> Result<Value, DispatchError> {
        let id = match params.get("targetId").and_then(Value::as_str) {
            Some(raw) => raw
                .parse::<u64>()
                .map(TabId::new)
                .map_err(|_| DispatchError::Failed("invalid targetId".into()))?,
            None => self
                .browser
                .tabs()
                .await
                .map_err(|error| DispatchError::Failed(error.to_string()))?
                .first()
                .copied()
                .ok_or_else(|| DispatchError::Failed("no target".into()))?,
        };
        Ok(json!({"targetInfo": target_info(&self.browser, id).await}))
    }

    /// Enables or disables auto-attach and announces existing targets.
    async fn set_auto_attach(&mut self, params: &Value) -> Value {
        let auto_attach = params
            .get("autoAttach")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        self.auto_attach = auto_attach;
        if auto_attach {
            for id in self.browser.tabs().await.unwrap_or_default() {
                if let Ok(tab) = self.browser.tab(id).await {
                    let session_id = self.mint_session(&tab);
                    self.push_attached(&session_id, &tab).await;
                }
            }
        }
        json!({})
    }

    /// Registers a new flattened session for `tab`.
    fn mint_session(&mut self, tab: &TabHandle) -> String {
        let session_id = format!("s{}", self.next_session);
        self.next_session = self.next_session.saturating_add(1);
        self.sessions.insert(session_id.clone(), tab.clone());
        session_id
    }

    /// Queues `Target.attachedToTarget` for auto-attach clients.
    async fn push_attached(&mut self, session_id: &str, tab: &TabHandle) {
        let mut info = target_info(&self.browser, tab.id()).await;
        if let Some(object) = info.as_object_mut() {
            object.insert("attached".into(), json!(true));
        }
        self.events.push(json!({
            "method": "Target.attachedToTarget",
            "params": {
                "sessionId": session_id,
                "targetInfo": info,
                "waitingForDebugger": false,
            },
        }));
    }

    async fn dispatch_text(&mut self, text: &str) -> Outcome {
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
        match self.dispatch(method, params, session).await {
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

    async fn dispatch(
        &mut self,
        method: &str,
        params: &Value,
        session: Option<&str>,
    ) -> Result<Value, DispatchError> {
        if method != "Browser.close" && !self.browser.is_live().await.unwrap_or(false) {
            return Err(DispatchError::Failed("browser closed".into()));
        }
        if let Some(session) = session {
            let tab = self
                .sessions
                .get(session)
                .cloned()
                .ok_or_else(|| DispatchError::Failed("unknown session".into()))?;
            return self
                .dispatch_tab_method(method, params, &tab, Some(session))
                .await;
        }
        if let Some(tab) = self.tab.clone() {
            return self.dispatch_tab_method(method, params, &tab, None).await;
        }
        self.dispatch_browser(method, params).await
    }

    async fn dispatch_browser(
        &mut self,
        method: &str,
        params: &Value,
    ) -> Result<Value, DispatchError> {
        match method {
            "Browser.getVersion" => Ok(json!({
                "protocolVersion": "1.3",
                "product": PRODUCT,
                "revision": "0",
                "userAgent": PRODUCT,
                "jsVersion": "QuickJS",
            })),
            "Browser.setDownloadBehavior" => Ok(json!({})),
            "Browser.getWindowForTarget" => Ok(json!({
                "windowId": 1,
                "bounds": {"left": 0, "top": 0, "width": 1280, "height": 720, "windowState": "normal"},
            })),
            "Browser.close" => {
                self.browser
                    .close()
                    .await
                    .map_err(|error| DispatchError::Failed(error.to_string()))?;
                self.sessions.clear();
                self.stop_subscriptions();
                self.tab = None;
                let _result = self.stop.send(true);
                Ok(json!({}))
            }
            _ => self.dispatch_target(method, params).await,
        }
    }

    async fn dispatch_target(
        &mut self,
        method: &str,
        params: &Value,
    ) -> Result<Value, DispatchError> {
        match method {
            "Target.setAutoAttach" => Ok(self.set_auto_attach(params).await),
            "Target.getTargetInfo" => self.target_info_for(params).await,
            "Target.getTargets" => {
                let mut target_infos = Vec::new();
                for id in self
                    .browser
                    .tabs()
                    .await
                    .map_err(|error| DispatchError::Failed(error.to_string()))?
                {
                    target_infos.push(target_info(&self.browser, id).await);
                }
                Ok(json!({ "targetInfos": target_infos }))
            }
            "Target.createTarget" => {
                let url = params
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or("about:blank");
                let tab = self
                    .browser
                    .create_tab()
                    .await
                    .map_err(|err| DispatchError::Failed(err.to_string()))?;
                if let Err(error) = open_url(&tab, url).await {
                    let _result = self.browser.close_tab(tab.id()).await;
                    return Err(error);
                }
                if self.auto_attach {
                    let session_id = self.mint_session(&tab);
                    self.push_attached(&session_id, &tab).await;
                }
                Ok(json!({ "targetId": tab.id().to_string() }))
            }
            "Target.closeTarget" => {
                let id = target_id(params.get("targetId"))?;
                if self.auto_attach {
                    let detached: Vec<String> = self
                        .sessions
                        .iter()
                        .filter(|(_, tab)| tab.id() == id)
                        .map(|(session, _)| session.clone())
                        .collect();
                    for session_id in detached {
                        self.events.push(json!({
                            "method": "Target.detachedFromTarget",
                            "params": {
                                "sessionId": session_id,
                                "targetId": id.to_string(),
                            },
                        }));
                    }
                }
                self.browser
                    .close_tab(id)
                    .await
                    .map_err(|err| DispatchError::Failed(err.to_string()))?;
                self.sessions.retain(|_, tab| tab.id() != id);
                self.unsubscribe_tab_id(id);
                self.loader_ids.remove(&id);
                self.isolated_worlds.remove(&id);
                if self.tab.as_ref().is_some_and(|tab| tab.id() == id) {
                    self.tab = None;
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
                let tab = self
                    .browser
                    .tab(id)
                    .await
                    .map_err(|err| DispatchError::Failed(err.to_string()))?;
                let session_id = format!("s{}", self.next_session);
                self.next_session = self.next_session.saturating_add(1);
                self.sessions.insert(session_id.clone(), tab);
                Ok(json!({ "sessionId": session_id }))
            }
            "Target.detachFromTarget" => {
                if let Some(session) = params.get("sessionId").and_then(Value::as_str) {
                    self.sessions.remove(session);
                    self.unsubscribe_session(session);
                }
                Ok(json!({}))
            }
            _ => Err(DispatchError::MethodNotFound),
        }
    }

    async fn dispatch_tab_method(
        &mut self,
        method: &str,
        params: &Value,
        tab: &TabHandle,
        session: Option<&str>,
    ) -> Result<Value, DispatchError> {
        match method {
            "Page.enable" => {
                self.subscribe_tab(tab, session).await?;
                Ok(json!({}))
            }
            "Page.disable" => {
                self.unsubscribe_tab(tab.id(), session);
                Ok(json!({}))
            }
            "Page.navigate" => self.navigate_tab(params, tab).await,
            "Runtime.enable" => {
                self.push_session_event(
                    session,
                    "Runtime.executionContextCreated",
                    &json!({"context": {
                        "id": tab.id().get(),
                        "origin": "://",
                        "name": "",
                        "uniqueId": format!("ctx-{}", tab.id()),
                        "auxData": {
                            "isDefault": true,
                            "type": "default",
                            "frameId": tab.id().to_string(),
                        },
                    }}),
                );
                Ok(json!({}))
            }
            "Page.createIsolatedWorld" => {
                let context_id = self.next_context;
                self.next_context = self.next_context.saturating_add(1);
                let frame_id = params
                    .get("frameId")
                    .and_then(Value::as_str)
                    .map_or_else(|| tab.id().to_string(), str::to_owned);
                let name = params
                    .get("worldName")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if !name.is_empty() {
                    let worlds = self.isolated_worlds.entry(tab.id()).or_default();
                    if !worlds.iter().any(|existing| existing == name) {
                        worlds.push(name.to_owned());
                    }
                }
                self.push_session_event(
                    session,
                    "Runtime.executionContextCreated",
                    &json!({"context": {
                        "id": context_id,
                        "origin": "://",
                        "name": name,
                        "uniqueId": format!("ctx-{context_id}"),
                        "auxData": {
                            "isDefault": false,
                            "type": "isolated",
                            "frameId": frame_id,
                        },
                    }}),
                );
                Ok(json!({"executionContextId": context_id}))
            }
            "Target.getTargetInfo" => {
                let mut info = target_info(&self.browser, tab.id()).await;
                if let Some(object) = info.as_object_mut() {
                    object.insert("attached".into(), json!(true));
                }
                Ok(json!({"targetInfo": info}))
            }
            "Runtime.evaluate" | "Runtime.callFunctionOn" => {
                self.dispatch_runtime(method, params, tab).await
            }
            _ => session_method(method, tab).await,
        }
    }

    async fn navigate_tab(
        &mut self,
        params: &Value,
        tab: &TabHandle,
    ) -> Result<Value, DispatchError> {
        let url = params
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| DispatchError::Failed("missing url".into()))?;
        let loader_id = format!("{}", self.next_loader);
        self.next_loader = self.next_loader.saturating_add(1);
        self.loader_ids.insert(tab.id(), loader_id.clone());
        let frame_id = tab.id().to_string();
        if url.is_empty() || url == "about:blank" {
            open_url(tab, url).await?;
            return Ok(json!({"frameId": frame_id, "loaderId": loader_id}));
        }
        let events = tab
            .subscribe()
            .await
            .map_err(|error| DispatchError::Failed(error.to_string()))?;
        open_url(tab, url).await?;
        match wait_for_navigation(events, Duration::from_secs(30)).await {
            Ok(()) => Ok(json!({"frameId": frame_id, "loaderId": loader_id})),
            Err(error_text) => Ok(json!({
                "frameId": frame_id,
                "loaderId": loader_id,
                "errorText": error_text,
            })),
        }
    }

    /// Queues an event for one session (or the direct page socket).
    fn push_session_event(&mut self, session: Option<&str>, method: &str, params: &Value) {
        let mut message = json!({"method": method, "params": params});
        attach_session(&mut message, session);
        self.events.push(message);
    }

    /// `Runtime.evaluate` / `Runtime.callFunctionOn` with value or handle
    /// semantics. Handles live in the page as `globalThis.__tb_handles`, so the
    /// adapter needs no JS value storage of its own.
    async fn dispatch_runtime(
        &mut self,
        method: &str,
        params: &Value,
        tab: &TabHandle,
    ) -> Result<Value, DispatchError> {
        let return_by_value = params
            .get("returnByValue")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let source = if method == "Runtime.evaluate" {
            let expression = params
                .get("expression")
                .and_then(Value::as_str)
                .ok_or_else(|| DispatchError::Failed("missing expression".into()))?;
            // `expression` is a script (statements allowed); indirect eval keeps
            // its completion value without parenthesizing a trailing `;`.
            format!("(0, eval)({})", json_string(expression))
        } else {
            let declaration = params
                .get("functionDeclaration")
                .and_then(Value::as_str)
                .ok_or_else(|| DispatchError::Failed("missing functionDeclaration".into()))?;
            let receiver = match params.get("objectId").and_then(Value::as_str) {
                Some(id) => format!("globalThis.__tb_handles[{}]", json_string(id)),
                None => "undefined".to_owned(),
            };
            let arguments = arguments_expression(params);
            format!("({declaration}).apply({receiver}, {arguments})")
        };
        if return_by_value {
            Ok(Self::runtime_value(tab, &source).await)
        } else {
            Ok(self.runtime_handle(tab, &source).await)
        }
    }

    /// Stores the result in a page-side handle and returns its `objectId`;
    /// primitives are serializable and returned inline.
    async fn runtime_handle(&mut self, tab: &TabHandle, source: &str) -> Value {
        let handle = self.next_handle;
        self.next_handle = self.next_handle.saturating_add(1);
        let script = RUNTIME_HANDLE
            .replace("__ID__", &json_string(&handle.to_string()))
            .replace("__SOURCE__", source);
        let value = match tab.execute_script(&script).await {
            Ok(value) => value,
            Err(error) => return exception_reply(&error),
        };
        if let RemoteValue::String(text) = value
            && let Ok(parsed) = serde_json::from_str::<Value>(&text)
        {
            return json!({"result": parsed});
        }
        exception_text_reply("unexpected script result")
    }

    /// Resolves the value (awaiting a thenable via the waiter) and serializes
    /// it to a CDP `RemoteObject`.
    async fn runtime_value(tab: &TabHandle, source: &str) -> Value {
        let schedule = RUNTIME_SCHEDULE.replace("__SOURCE__", source);
        if let Err(error) = tab.execute_script(&schedule).await {
            return exception_reply(&error);
        }
        match tab
            .run_until_js_true(
                "Boolean(globalThis.__tb_async && globalThis.__tb_async.done)",
                Duration::from_secs(2),
            )
            .await
        {
            Ok(true) => {}
            Ok(false) => return exception_text_reply("awaitPromise timed out"),
            Err(error) => return exception_reply(&error),
        }
        let value = match tab.execute_script(RUNTIME_READ).await {
            Ok(value) => value,
            Err(error) => return exception_reply(&error),
        };
        if let RemoteValue::String(text) = value
            && let Ok(parsed) = serde_json::from_str::<Value>(&text)
        {
            if let Some(error) = parsed.get("error").and_then(Value::as_str) {
                return exception_text_reply(error);
            }
            return json!({"result": parsed});
        }
        exception_text_reply("unexpected script result")
    }

    async fn subscribe_tab(
        &mut self,
        tab: &TabHandle,
        session: Option<&str>,
    ) -> Result<(), DispatchError> {
        if self
            .subscriptions
            .iter()
            .any(|item| item.tab_id == tab.id() && item.session.as_deref() == session)
        {
            return Ok(());
        }
        let events = tab
            .subscribe()
            .await
            .map_err(|error| DispatchError::Failed(error.to_string()))?;
        let tab_for_events = tab.clone();
        let session_for_events = session.map(str::to_owned);
        let tx = self.tab_events_tx.clone();
        let mut events = events;
        let forward = tokio::spawn(async move {
            while let Some(event) = events.recv().await {
                let message = ConnEvent {
                    tab: tab_for_events.clone(),
                    session: session_for_events.clone(),
                    event,
                };
                if tx.send(message).await.is_err() {
                    break;
                }
            }
        });
        self.subscriptions.push(TabSubscription {
            tab_id: tab.id(),
            session: session.map(str::to_owned),
            forward,
        });
        Ok(())
    }

    fn unsubscribe_tab(&mut self, tab_id: TabId, session: Option<&str>) {
        let mut retained = Vec::new();
        for subscription in self.subscriptions.drain(..) {
            if subscription.tab_id == tab_id && subscription.session.as_deref() == session {
                subscription.forward.abort();
            } else {
                retained.push(subscription);
            }
        }
        self.subscriptions = retained;
    }

    fn stop_subscriptions(&mut self) {
        for subscription in self.subscriptions.drain(..) {
            subscription.forward.abort();
        }
    }

    fn unsubscribe_tab_id(&mut self, tab_id: TabId) {
        let mut retained = Vec::new();
        for subscription in self.subscriptions.drain(..) {
            if subscription.tab_id == tab_id {
                subscription.forward.abort();
            } else {
                retained.push(subscription);
            }
        }
        self.subscriptions = retained;
    }

    fn unsubscribe_session(&mut self, session: &str) {
        let mut retained = Vec::new();
        for subscription in self.subscriptions.drain(..) {
            if subscription.session.as_deref() == Some(session) {
                subscription.forward.abort();
            } else {
                retained.push(subscription);
            }
        }
        self.subscriptions = retained;
    }
}

async fn run_socket(mut socket: WebSocket, state: AppState, tab: Option<TabHandle>) {
    let (tab_events_tx, tab_events_rx) = mpsc::channel(256);
    let mut conn = Conn {
        browser: state.browser,
        sessions: HashMap::new(),
        next_session: 1,
        tab,
        stop: state.stop,
        subscriptions: Vec::new(),
        tab_events_tx,
        tab_events_rx,
        clock_origin: Instant::now(),
        auto_attach: false,
        events: Vec::new(),
        loader_ids: HashMap::new(),
        isolated_worlds: HashMap::new(),
        next_loader: 1,
        next_context: 1000,
        next_handle: 1,
    };
    loop {
        let wake = tokio::select! {
            event = conn.tab_events_rx.recv() => SocketWake::Event(event),
            incoming = socket.recv() => SocketWake::Incoming(incoming),
        };
        match wake {
            SocketWake::Event(Some(event)) => {
                let messages = conn.event_messages(event).await;
                if send_messages(&mut socket, messages).await.is_err() {
                    break;
                }
            }
            SocketWake::Event(None)
            | SocketWake::Incoming(None | Some(Ok(WsMessage::Close(_)) | Err(_))) => break,
            SocketWake::Incoming(Some(Ok(WsMessage::Text(text)))) => {
                let outcome = conn.dispatch_text(text.as_str()).await;
                let queued = conn.take_queued_events();
                if send_messages(&mut socket, queued).await.is_err() {
                    break;
                }
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
            SocketWake::Incoming(Some(Ok(WsMessage::Ping(payload)))) => {
                let _result = socket.send(WsMessage::Pong(payload)).await;
            }
            SocketWake::Incoming(Some(Ok(_))) => {}
        }
    }
    conn.stop_subscriptions();
    let _result = socket.send(WsMessage::Close(None)).await;
}

enum SocketWake {
    Event(Option<ConnEvent>),
    Incoming(Option<Result<WsMessage, axum::Error>>),
}

async fn send_messages(socket: &mut WebSocket, messages: Vec<Value>) -> Result<(), axum::Error> {
    for message in messages {
        socket.send(WsMessage::text(message.to_string())).await?;
    }
    Ok(())
}

async fn session_method(method: &str, tab: &TabHandle) -> Result<Value, DispatchError> {
    match method {
        "Page.getFrameTree" => {
            let url = tab
                .document_url()
                .await
                .map_err(|error| DispatchError::Failed(error.to_string()))?;
            Ok(json!({"frameTree": {"frame": {
                "id": tab.id().to_string(),
                "loaderId": "",
                "url": url,
                "mimeType": "text/html",
            }}}))
        }
        "Page.addScriptToEvaluateOnNewDocument" => Ok(json!({"identifier": "1"})),
        "Runtime.disable"
        | "Target.setAutoAttach"
        | "Runtime.runIfWaitingForDebugger"
        | "Log.enable"
        | "Page.setLifecycleEventsEnabled"
        | "Network.enable"
        | "Emulation.setFocusEmulationEnabled"
        | "Emulation.setDeviceMetricsOverride"
        | "Emulation.setTouchEmulationEnabled"
        | "Emulation.setEmulatedMedia"
        | "Emulation.setScriptExecutionDisabled"
        | "Runtime.addBinding"
        | "Security.setIgnoreCertificateErrors"
        | "Page.setBypassCSP" => Ok(json!({})),
        _ => Err(DispatchError::MethodNotFound),
    }
}

/// Waits for the next navigation outcome on a temporary subscription.
async fn wait_for_navigation(
    mut events: mpsc::Receiver<TabEvent>,
    duration: Duration,
) -> Result<(), &'static str> {
    let result = tokio::time::timeout(duration, async {
        loop {
            match events.recv().await {
                Some(TabEvent::Navigated) => return Ok(()),
                Some(TabEvent::NavigationFailed | TabEvent::FetchFailed) => {
                    return Err("net::ERR_FAILED");
                }
                Some(_) => {}
                None => return Err("net::ERR_ABORTED"),
            }
        }
    })
    .await;
    match result {
        Ok(outcome) => outcome,
        Err(_) => Err("net::ERR_TIMED_OUT"),
    }
}

async fn open_url(tab: &TabHandle, url: &str) -> Result<(), DispatchError> {
    if url.is_empty() || url == "about:blank" {
        tab.load_html("<!doctype html><title></title>")
            .await
            .map_err(|err| DispatchError::Failed(err.to_string()))?;
        return Ok(());
    }
    tab.goto(url)
        .await
        .map_err(|err| DispatchError::Failed(err.to_string()))?;
    Ok(())
}

/// Builds the JS argument array for `Runtime.callFunctionOn`.
fn arguments_expression(params: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(arguments) = params.get("arguments").and_then(Value::as_array) {
        for argument in arguments {
            if let Some(id) = argument.get("objectId").and_then(Value::as_str) {
                parts.push(format!("globalThis.__tb_handles[{}]", json_string(id)));
            } else if let Some(value) = argument.get("value") {
                parts.push(serde_json::to_string(value).unwrap_or_else(|_| "undefined".to_owned()));
            } else {
                parts.push("undefined".to_owned());
            }
        }
    }
    format!("[{}]", parts.join(", "))
}

/// JS string literal for embedding in generated source.
fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_owned())
}

/// A failed runtime call, shaped like a CDP exception.
fn exception_reply(error: &TabError) -> Value {
    json!({
        "result": {"type": "undefined"},
        "exceptionDetails": {"text": error.to_string()},
    })
}

/// A failed runtime call with a literal message.
fn exception_text_reply(text: &str) -> Value {
    json!({
        "result": {"type": "undefined"},
        "exceptionDetails": {"text": text},
    })
}

/// Schedules `__SOURCE__` and captures its settled value.
const RUNTIME_SCHEDULE: &str = r#"(() => {
  globalThis.__tb_async = { done: false };
  Promise.resolve((__SOURCE__)).then(
    (v) => { globalThis.__tb_async.done = true; globalThis.__tb_async.value = v; },
    (e) => { globalThis.__tb_async.done = true; globalThis.__tb_async.error = String((e && e.message) || e); }
  );
  return "scheduled";
})()"#;

/// Evaluates `__SOURCE__`, stores a non-primitive in `__ID__`, and returns the
/// CDP `RemoteObject` JSON (primitives inline, per protocol).
const RUNTIME_HANDLE: &str = r#"(() => {
  globalThis.__tb_handles = globalThis.__tb_handles || {};
  const v = (__SOURCE__);
  if (v === null) return JSON.stringify({ type: "object", subtype: "null", value: null });
  const t = typeof v;
  if (t === "undefined") return JSON.stringify({ type: "undefined" });
  if (t === "number") {
    if (Number.isFinite(v)) return JSON.stringify({ type: "number", value: v });
    return JSON.stringify({ type: "number", unserializableValue: Number.isNaN(v) ? "NaN" : (v > 0 ? "Infinity" : "-Infinity") });
  }
  if (t === "string" || t === "boolean") return JSON.stringify({ type: t, value: v });
  globalThis.__tb_handles[__ID__] = v;
  return JSON.stringify({ type: t === "function" ? "function" : "object", objectId: __ID__ });
})()"#;

/// Reads the captured async result and serializes it as a CDP `RemoteObject`.
const RUNTIME_READ: &str = r#"(() => {
  const s = globalThis.__tb_async;
  if (!s || !s.done) return JSON.stringify({ pending: true });
  if (s.error !== undefined) return JSON.stringify({ error: s.error });
  const v = s.value;
  if (v === null) return JSON.stringify({ type: "object", subtype: "null", value: null });
  const t = typeof v;
  if (t === "undefined") return JSON.stringify({ type: "undefined" });
  if (t === "number") {
    if (Number.isFinite(v)) return JSON.stringify({ type: "number", value: v });
    return JSON.stringify({ type: "number", unserializableValue: Number.isNaN(v) ? "NaN" : (v > 0 ? "Infinity" : "-Infinity") });
  }
  if (t === "string" || t === "boolean") return JSON.stringify({ type: t, value: v });
  try { return JSON.stringify({ type: "object", value: v }); }
  catch (e) { return JSON.stringify({ type: "object" }); }
})()"#;

async fn target_info(browser: &BrowserHandle, id: TabId) -> Value {
    let url = match browser.tab(id).await {
        Ok(tab) => tab
            .document_url()
            .await
            .unwrap_or_else(|_| "about:blank".into()),
        Err(_) => "about:blank".into(),
    };
    json!({
        "targetId": id.to_string(),
        "type": "page",
        "title": "",
        "url": url,
        "attached": false,
        "canAccessOpener": false,
        "browserContextId": DEFAULT_BROWSER_CONTEXT_ID,
    })
}

fn target_id(value: Option<&Value>) -> Result<TabId, DispatchError> {
    let raw = value
        .and_then(Value::as_str)
        .ok_or_else(|| DispatchError::Failed("missing targetId".into()))?;
    let id = raw
        .parse::<u64>()
        .map_err(|_| DispatchError::Failed("invalid targetId".into()))?;
    Ok(TabId::new(id))
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
