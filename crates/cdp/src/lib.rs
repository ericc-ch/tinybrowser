//! CDP adapter over [`browser::BrowserHandle`].
//!
//! [ADR 0009](../../../docs/adrs/0009-named-profile-daemon.md): honest first
//! subsets of Browser, Target, Page, and Runtime. Unsupported methods return
//! method-not-found. Flattened `sessionId` routing on the browser socket.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use browser::{BrowserHandle, PageError, PageHandle, PageId, RemoteValue};
use serde_json::{Value, json};
use tungstenite::protocol::{Role, WebSocket};
use tungstenite::{Message, handshake::derive_accept_key};

const MAX_HEAD: usize = 65_536;
const MAX_BODY: usize = 8_388_608;
const PRODUCT: &str = "tinybrowser/0.1.0";

/// Serves CDP HTTP discovery and WebSocket endpoints on `listener`.
///
/// Returns when [`browser::BrowserHandle::close`] runs through `Browser.close`.
///
/// # Errors
///
/// Returns when `accept` fails.
pub fn serve(listener: &TcpListener, browser: &BrowserHandle) -> io::Result<()> {
    let bound = listener.local_addr()?;
    let stop = Arc::new(AtomicBool::new(false));
    loop {
        let (stream, _) = match listener.accept() {
            Ok(pair) => pair,
            Err(_) if stop.load(Ordering::SeqCst) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if accept_retry(&error) => {
                eprintln!("cdp: accept {error}");
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            Err(error) => return Err(error),
        };
        if stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        let browser = browser.clone();
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            if let Err(error) = handle_connection(stream, browser, bound, stop) {
                eprintln!("cdp: {error}");
            }
        });
    }
}

/// Client for the browser WebSocket.
pub struct Client {
    socket: WebSocket<TcpStream>,
    next_id: i64,
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
        }
    }
}

fn connect_ws(addr: SocketAddr, path: &str) -> io::Result<Client> {
    let mut stream = TcpStream::connect(addr)?;
    let key = client_key();
    let host = addr.to_string();
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {key}\r\n\r\n"
    );
    stream.write_all(request.as_bytes())?;
    let (start, headers, _) = read_head(&mut stream)?;
    let status = start
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(0);
    if status != 101 {
        return Err(io::Error::other(format!("ws handshake {status}")));
    }
    let accept = header(&headers, "sec-websocket-accept");
    if accept != Some(derive_accept_key(key.as_bytes()).as_str()) {
        return Err(io::Error::other("ws accept mismatch"));
    }
    Ok(Client {
        socket: WebSocket::from_raw_socket(stream, Role::Client, None),
        next_id: 0,
    })
}

fn client_key() -> String {
    let mut raw = [0_u8; 16];
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(1, |duration| duration.as_nanos())
        .to_le_bytes();
    raw.copy_from_slice(&nanos[..16]);
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, raw)
}

fn handle_connection(
    mut stream: TcpStream,
    browser: BrowserHandle,
    bound: SocketAddr,
    stop: Arc<AtomicBool>,
) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    let (start, headers, _) = read_head(&mut stream)?;
    let mut parts = start.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("/");
    if method != "GET" {
        write_http(&mut stream, 405, "text/plain", b"method not allowed")?;
        return Ok(());
    }
    if is_websocket(&headers) {
        stream.set_read_timeout(None)?;
        return serve_socket(stream, path, &headers, browser, bound, stop);
    }
    match path {
        "/json/version" | "/json/version/" => {
            let body = json!({
                "Browser": PRODUCT,
                "Protocol-Version": "1.3",
                "webSocketDebuggerUrl": format!("ws://{bound}/devtools/browser"),
            });
            write_http(
                &mut stream,
                200,
                "application/json",
                body.to_string().as_bytes(),
            )
        }
        "/json" | "/json/" | "/json/list" | "/json/list/" => {
            let body = json_list(&browser, bound);
            write_http(
                &mut stream,
                200,
                "application/json",
                body.to_string().as_bytes(),
            )
        }
        _ => write_http(&mut stream, 404, "text/plain", b"not found"),
    }
}

fn json_list(browser: &BrowserHandle, bound: SocketAddr) -> Value {
    let mut targets = Vec::new();
    for id in browser.pages() {
        let url = browser
            .page(id)
            .ok()
            .and_then(|page| page.document_url().ok())
            .unwrap_or_else(|| "about:blank".into());
        targets.push(json!({
            "id": id.to_string(),
            "type": "page",
            "url": url,
            "webSocketDebuggerUrl": format!("ws://{bound}/devtools/page/{id}"),
        }));
    }
    Value::Array(targets)
}

fn is_websocket(headers: &[(String, String)]) -> bool {
    headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("upgrade") && value.eq_ignore_ascii_case("websocket")
    })
}

fn serve_socket(
    mut stream: TcpStream,
    path: &str,
    headers: &[(String, String)],
    browser: BrowserHandle,
    bound: SocketAddr,
    stop: Arc<AtomicBool>,
) -> io::Result<()> {
    let page = match page_from_path(path, &browser) {
        Ok(page) => page,
        Err(error) => {
            let status = if error.kind() == io::ErrorKind::NotFound {
                404
            } else {
                400
            };
            write_http(
                &mut stream,
                status,
                "text/plain",
                error.to_string().as_bytes(),
            )?;
            return Ok(());
        }
    };
    let key = header(headers, "sec-websocket-key")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing websocket key"))?;
    let accept = derive_accept_key(key.as_bytes());
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    stream.write_all(response.as_bytes())?;
    let mut conn = Conn {
        browser,
        socket: WebSocket::from_raw_socket(stream, Role::Server, None),
        sessions: HashMap::new(),
        next_session: 1,
        page,
        bound,
        stop,
    };
    conn.run()
}

fn page_from_path(path: &str, browser: &BrowserHandle) -> io::Result<Option<PageHandle>> {
    let path = path.trim_end_matches('/');
    if path == "/devtools/browser" {
        return Ok(None);
    }
    let Some(rest) = path.strip_prefix("/devtools/page/") else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "not a devtools socket",
        ));
    };
    if rest.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid page target",
        ));
    }
    let raw = rest
        .parse::<u64>()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid page target"))?;
    browser
        .page(PageId::new(raw))
        .map(Some)
        .map_err(|_| io::Error::new(io::ErrorKind::NotFound, "unknown page target"))
}

struct Conn {
    browser: BrowserHandle,
    socket: WebSocket<TcpStream>,
    sessions: HashMap<String, PageHandle>,
    next_session: u64,
    page: Option<PageHandle>,
    bound: SocketAddr,
    stop: Arc<AtomicBool>,
}

impl Conn {
    fn run(&mut self) -> io::Result<()> {
        loop {
            match self.socket.read() {
                Ok(Message::Text(text)) => self.dispatch_text(&text)?,
                Ok(Message::Ping(payload)) => {
                    self.socket.send(Message::Pong(payload)).map_err(ws_io)?;
                }
                Ok(Message::Close(_)) | Err(_) => return Ok(()),
                Ok(_) => {}
            }
        }
    }

    fn dispatch_text(&mut self, text: &str) -> io::Result<()> {
        let parsed: Value = match serde_json::from_str(text) {
            Ok(value) => value,
            Err(err) => {
                return self.send_json(&json!({
                    "id": Value::Null,
                    "error": {"code": -32700, "message": err.to_string()},
                }));
            }
        };
        let id = parsed.get("id").cloned().unwrap_or(Value::Null);
        let Some(method) = parsed.get("method").and_then(Value::as_str) else {
            return self.send_json(&json!({
                "id": id,
                "error": {"code": -32600, "message": "missing method"},
            }));
        };
        let empty = json!({});
        let params = parsed.get("params").unwrap_or(&empty);
        let session = parsed.get("sessionId").and_then(Value::as_str);
        match self.dispatch(method, params, session) {
            Ok(result) => {
                let mut reply = json!({"id": id, "result": result});
                attach_session(&mut reply, session);
                self.send_json(&reply)?;
                if method == "Browser.close" {
                    return Ok(());
                }
                Ok(())
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
                self.send_json(&reply)
            }
            Err(DispatchError::Failed(message)) => {
                let mut reply = json!({
                    "id": id,
                    "error": {"code": -32000, "message": message},
                });
                attach_session(&mut reply, session);
                self.send_json(&reply)
            }
        }
    }

    fn send_json(&mut self, value: &Value) -> io::Result<()> {
        self.socket
            .send(Message::Text(value.to_string().into()))
            .map_err(ws_io)
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
            return session_method(method, params, &page);
        }
        if let Some(page) = self.page.clone() {
            return session_method(method, params, &page);
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
                self.browser.close();
                self.sessions.clear();
                self.page = None;
                self.stop.store(true, Ordering::SeqCst);
                let _ = TcpStream::connect(self.bound);
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
                }
                Ok(json!({}))
            }
            _ => Err(DispatchError::MethodNotFound),
        }
    }
}

fn session_method(method: &str, params: &Value, page: &PageHandle) -> Result<Value, DispatchError> {
    match method {
        "Page.enable" | "Runtime.enable" | "Page.disable" | "Runtime.disable" => Ok(json!({})),
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
    page.run_until_load()
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

fn accept_retry(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock
            | io::ErrorKind::TimedOut
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::OutOfMemory
            | io::ErrorKind::ResourceBusy
            | io::ErrorKind::QuotaExceeded
    )
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

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

type HttpHead = (String, Vec<(String, String)>, Vec<u8>);

fn read_head(stream: &mut TcpStream) -> io::Result<HttpHead> {
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        if head.len() >= MAX_HEAD {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request head too large",
            ));
        }
        let read = stream.read(&mut byte)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "closed during head",
            ));
        }
        head.push(byte[0]);
    }
    let text = String::from_utf8_lossy(&head);
    let mut lines = text.split("\r\n");
    let start = lines.next().unwrap_or("").to_owned();
    let mut headers = Vec::new();
    let mut content_length = 0_usize;
    for line in lines {
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            content_length = value.trim().parse().unwrap_or(0);
        }
        headers.push((name.trim().to_owned(), value.trim().to_owned()));
    }
    if content_length > MAX_BODY {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "request body too large",
        ));
    }
    let mut body = vec![0_u8; content_length];
    if content_length > 0 {
        stream.read_exact(&mut body)?;
    }
    Ok((start, headers, body))
}

fn write_http(stream: &mut TcpStream, status: u16, ctype: &str, body: &[u8]) -> io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)
}

fn ws_io(err: tungstenite::Error) -> io::Error {
    io::Error::other(err)
}

fn json_io(err: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err)
}
