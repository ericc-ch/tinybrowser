//! Classic `WebDriver` HTTP server for wptrunner.
//!
//! In-process: one binary, no sidecar. Speaks the W3C HTTP protocol
//! ([WebDriver](https://w3c.github.io/webdriver/)) enough for testharness:
//! session, navigate, execute script, windows.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use browser::{Page, PageError, ScriptValue};
use serde_json::{Value, json};

pub use browser::AgentBuilder;

const ELEMENT_KEY: &str = "element-6066-11e4-a52e-4f735466cecf";
const MAX_HEAD: usize = 65_536;
const MAX_BODY: usize = 8_388_608;
const DEFAULT_SCRIPT_TIMEOUT: Duration = Duration::from_millis(30_000);
const DEFAULT_PAGE_LOAD_TIMEOUT: Duration = Duration::from_millis(300_000);

/// Serves classic `WebDriver` on `listener` until the process exits.
///
/// Each session builds a [`Page`] from `builder` (separate cookie jar).
///
/// # Errors
///
/// Returns when `accept` fails.
pub fn serve(listener: &TcpListener, builder: AgentBuilder) -> std::io::Result<()> {
    let mut sessions = Sessions {
        builder,
        next_session: 0,
        next_window: 0,
        open: HashMap::new(),
    };
    loop {
        let (stream, _) = listener.accept()?;
        if let Err(error) = handle_connection(stream, &mut sessions) {
            eprintln!("webdriver: {error}");
        }
    }
}

struct Sessions {
    builder: AgentBuilder,
    next_session: u32,
    next_window: u32,
    open: HashMap<String, Session>,
}

struct Session {
    current: String,
    windows: HashMap<String, Window>,
    script_timeout: Duration,
    page_load_timeout: Duration,
}

struct Window {
    page: Page,
    elements: HashMap<String, browser::NodeId>,
    next_element: u32,
}

impl Sessions {
    fn blank_window(&self) -> Window {
        let mut page = Page::from_builder(self.builder.clone());
        page.load_html("<!doctype html><title></title>");
        Window {
            page,
            elements: HashMap::new(),
            next_element: 0,
        }
    }

    fn create(&mut self) -> String {
        self.next_session += 1;
        let id = format!("s{}", self.next_session);
        self.next_window += 1;
        let handle = format!("w{}", self.next_window);
        self.open.insert(
            id.clone(),
            Session {
                current: handle.clone(),
                windows: HashMap::from([(handle, self.blank_window())]),
                script_timeout: DEFAULT_SCRIPT_TIMEOUT,
                page_load_timeout: DEFAULT_PAGE_LOAD_TIMEOUT,
            },
        );
        id
    }
}

fn handle_connection(mut stream: TcpStream, sessions: &mut Sessions) -> std::io::Result<()> {
    let (method, path, body) = read_request(&mut stream)?;
    let (status, payload) = dispatch(&method, &path, &body, sessions);
    write_response(&mut stream, status, &payload)
}

fn dispatch(method: &str, path: &str, body: &str, sessions: &mut Sessions) -> (u16, Value) {
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    match (method, segments.as_slice()) {
        ("GET", ["status"]) => ok(json!({"ready": true, "message": "ready"})),
        ("POST", ["session"]) => {
            let id = sessions.create();
            ok(json!({"sessionId": id, "capabilities": {"browserName": "tinybrowser"}}))
        }
        ("DELETE", ["session", session]) => {
            sessions.open.remove(*session);
            ok(Value::Null)
        }
        ("POST", ["session", session, "url"]) => navigate(sessions, session, body),
        ("GET", ["session", session, "url"]) => current_url(sessions, session),
        ("POST", ["session", session, "execute", "sync"]) => {
            execute(sessions, session, body, false)
        }
        ("POST", ["session", session, "execute", "async"]) => {
            execute(sessions, session, body, true)
        }
        ("GET", ["session", session, "window"]) => current_window(sessions, session),
        ("GET", ["session", session, "window", "handles"]) => window_handles(sessions, session),
        ("POST", ["session", session, "window"]) => switch_window(sessions, session, body),
        ("POST", ["session", session, "window", "new"]) => new_window(sessions, session),
        ("DELETE", ["session", session, "window"]) => close_window(sessions, session),
        ("GET", ["session", _session, "window", "rect"]) => {
            ok(json!({"x":0,"y":0,"width":800,"height":600}))
        }
        ("POST", ["session", _session, "window", "rect"]) => {
            ok(json!({"x":0,"y":0,"width":800,"height":600}))
        }
        ("POST", ["session", session, "timeouts"]) => set_timeouts(sessions, session, body),
        ("GET", ["session", session, "timeouts"]) => get_timeouts(sessions, session),
        ("POST", ["session", _session, "element", _, "click"]) => ok(Value::Null),
        ("POST", ["session", _session, "actions"]) => ok(Value::Null),
        ("DELETE", ["session", _session, "actions"]) => ok(Value::Null),
        _ => error(404, "unknown command", path),
    }
}

fn navigate(sessions: &mut Sessions, session: &str, body: &str) -> (u16, Value) {
    let Some(url) = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| value.get("url").and_then(Value::as_str).map(str::to_owned))
    else {
        return error(400, "invalid argument", "missing url");
    };
    let page_load_timeout = match sessions.open.get(session) {
        Some(found) => found.page_load_timeout,
        None => return error(404, "invalid session id", session),
    };
    let Some(window) = current_mut(sessions, session) else {
        return error(404, "invalid session id", session);
    };
    match window.page.goto(&url) {
        Ok(()) => {
            if !window.page.run_until_load_timeout(page_load_timeout) {
                return error(500, "timeout", "navigation timed out");
            }
            if window.page.last_navigation_failed() {
                return error(500, "unknown error", "navigation failed");
            }
            ok(Value::Null)
        }
        Err(PageError::InvalidUrl { spec }) => error(400, "invalid argument", &spec),
        Err(err) => error(500, "unknown error", &err.to_string()),
    }
}

fn current_url(sessions: &Sessions, session: &str) -> (u16, Value) {
    match current(sessions, session) {
        Some(window) => ok(json!(window.page.document_url())),
        None => error(404, "invalid session id", session),
    }
}

fn execute(sessions: &mut Sessions, session: &str, body: &str, asynchronous: bool) -> (u16, Value) {
    let parsed: Value = match serde_json::from_str(body) {
        Ok(value) => value,
        Err(err) => return error(400, "invalid argument", &err.to_string()),
    };
    let script = parsed
        .get("script")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let args = parsed.get("args").cloned().unwrap_or(json!([]));
    let script_timeout = match sessions.open.get(session) {
        Some(found) => found.script_timeout,
        None => return error(404, "invalid session id", session),
    };
    let Some(window) = current_mut(sessions, session) else {
        return error(404, "invalid session id", session);
    };
    let wrapped = wrap_script(script, &args, asynchronous);
    if asynchronous {
        if let Err(err) = window.page.execute_script(&wrapped) {
            return error(500, "javascript error", &err.to_string());
        }
        if !window.page.run_until_timeout(script_timeout, |page| {
            matches!(
                page.execute_script("globalThis.__wd_done === true"),
                Ok(ScriptValue::Bool(true))
            )
        }) {
            return error(500, "script timeout", "script timeout");
        }
        match window.page.execute_script("globalThis.__wd_async") {
            Ok(value) => ok(encode(window, value)),
            Err(err) => error(500, "javascript error", &err.to_string()),
        }
    } else {
        match window.page.execute_script(&wrapped) {
            Ok(value) => ok(encode(window, value)),
            Err(err) => error(500, "javascript error", &err.to_string()),
        }
    }
}

fn wrap_script(script: &str, args: &Value, asynchronous: bool) -> String {
    let args_json = args.to_string();
    if asynchronous {
        format!(
            "globalThis.__wd_async = undefined;\n\
             globalThis.__wd_done = false;\n\
             (function() {{ {script} }}).apply(null, {args_json}.concat([function(v) {{ \
               globalThis.__wd_async = v === undefined ? null : v; \
               globalThis.__wd_done = true; \
             }}]));"
        )
    } else {
        format!("(function() {{ {script} }}).apply(null, {args_json})")
    }
}

fn encode(window: &mut Window, value: ScriptValue) -> Value {
    match value {
        ScriptValue::Undefined | ScriptValue::Null => Value::Null,
        ScriptValue::Bool(flag) => json!(flag),
        ScriptValue::Number(number) => {
            if number.is_finite() {
                json!(number)
            } else {
                Value::Null
            }
        }
        ScriptValue::String(text) => json!(text),
        ScriptValue::List(items) => {
            Value::Array(items.into_iter().map(|item| encode(window, item)).collect())
        }
        ScriptValue::Map(entries) => {
            let mut map = serde_json::Map::new();
            for (key, item) in entries {
                map.insert(key, encode(window, item));
            }
            Value::Object(map)
        }
        ScriptValue::Node(id) => {
            window.next_element += 1;
            let element = format!("e{}", window.next_element);
            window.elements.insert(element.clone(), id);
            json!({ ELEMENT_KEY: element })
        }
    }
}

fn get_timeouts(sessions: &Sessions, session: &str) -> (u16, Value) {
    match sessions.open.get(session) {
        Some(found) => ok(json!({
            "implicit": 0,
            "pageLoad": found.page_load_timeout.as_millis() as u64,
            "script": found.script_timeout.as_millis() as u64,
        })),
        None => error(404, "invalid session id", session),
    }
}

fn set_timeouts(sessions: &mut Sessions, session: &str, body: &str) -> (u16, Value) {
    let parsed: Value = match serde_json::from_str(body) {
        Ok(value) => value,
        Err(err) => return error(400, "invalid argument", &err.to_string()),
    };
    let Some(found) = sessions.open.get_mut(session) else {
        return error(404, "invalid session id", session);
    };
    if let Some(ms) = parsed.get("script").and_then(json_millis) {
        found.script_timeout = Duration::from_millis(ms);
    }
    if let Some(ms) = parsed.get("pageLoad").and_then(json_millis) {
        found.page_load_timeout = Duration::from_millis(ms);
    }
    ok(Value::Null)
}

fn json_millis(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_f64().and_then(|ms| (ms >= 0.0).then_some(ms as u64)))
}

fn current_window(sessions: &Sessions, session: &str) -> (u16, Value) {
    match sessions.open.get(session) {
        Some(found) => ok(json!(found.current)),
        None => error(404, "invalid session id", session),
    }
}

fn window_handles(sessions: &Sessions, session: &str) -> (u16, Value) {
    match sessions.open.get(session) {
        Some(found) => ok(json!(found.windows.keys().cloned().collect::<Vec<_>>())),
        None => error(404, "invalid session id", session),
    }
}

fn switch_window(sessions: &mut Sessions, session: &str, body: &str) -> (u16, Value) {
    let handle = serde_json::from_str::<Value>(body).ok().and_then(|value| {
        value
            .get("handle")
            .and_then(Value::as_str)
            .map(str::to_owned)
    });
    let Some(handle) = handle else {
        return error(400, "invalid argument", "missing handle");
    };
    let Some(found) = sessions.open.get_mut(session) else {
        return error(404, "invalid session id", session);
    };
    if !found.windows.contains_key(&handle) {
        return error(404, "no such window", &handle);
    }
    found.current = handle;
    ok(Value::Null)
}

fn new_window(sessions: &mut Sessions, session: &str) -> (u16, Value) {
    if !sessions.open.contains_key(session) {
        return error(404, "invalid session id", session);
    }
    sessions.next_window += 1;
    let handle = format!("w{}", sessions.next_window);
    let window = sessions.blank_window();
    let Some(found) = sessions.open.get_mut(session) else {
        return error(404, "invalid session id", session);
    };
    found.windows.insert(handle.clone(), window);
    ok(json!({"handle": handle, "type": "window"}))
}

fn close_window(sessions: &mut Sessions, session: &str) -> (u16, Value) {
    let Some(found) = sessions.open.get_mut(session) else {
        return error(404, "invalid session id", session);
    };
    found.windows.remove(&found.current);
    let remaining: Vec<String> = found.windows.keys().cloned().collect();
    if let Some(next) = remaining.first() {
        found.current.clone_from(next);
    }
    ok(json!(remaining))
}

fn current<'a>(sessions: &'a Sessions, session: &str) -> Option<&'a Window> {
    sessions
        .open
        .get(session)
        .and_then(|found| found.windows.get(&found.current))
}

fn current_mut<'a>(sessions: &'a mut Sessions, session: &str) -> Option<&'a mut Window> {
    sessions
        .open
        .get_mut(session)
        .and_then(|found| found.windows.get_mut(&found.current))
}

fn ok(value: Value) -> (u16, Value) {
    (
        200,
        Value::Object(serde_json::Map::from_iter([("value".into(), value)])),
    )
}

fn error(status: u16, err: &str, message: &str) -> (u16, Value) {
    (
        status,
        json!({"value": {"error": err, "message": message, "stacktrace": ""}}),
    )
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<(String, String, String)> {
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        if head.len() >= MAX_HEAD {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "request head too large",
            ));
        }
        let read = stream.read(&mut byte)?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "closed",
            ));
        }
        head.push(byte[0]);
    }
    let text = String::from_utf8_lossy(&head);
    let mut lines = text.split("\r\n");
    let request = lines.next().unwrap_or("");
    let mut parts = request.split_whitespace();
    let method = parts.next().unwrap_or("GET").to_owned();
    let path = parts.next().unwrap_or("/").to_owned();
    let mut content_length = 0_usize;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length")
            && let Ok(length) = value.trim().parse()
        {
            content_length = length;
        }
    }
    if content_length > MAX_BODY {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "request body too large",
        ));
    }
    let mut body = vec![0_u8; content_length];
    if content_length > 0 {
        stream.read_exact(&mut body)?;
    }
    Ok((method, path, String::from_utf8_lossy(&body).into_owned()))
}

fn write_response(stream: &mut TcpStream, status: u16, payload: &Value) -> std::io::Result<()> {
    let body = payload.to_string();
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())
}
