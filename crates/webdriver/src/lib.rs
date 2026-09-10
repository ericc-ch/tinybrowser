//! Classic `WebDriver` HTTP server for wptrunner.
//!
//! Speaks the W3C HTTP protocol
//! ([WebDriver](https://w3c.github.io/webdriver/)) enough for testharness:
//! session, navigate, execute script, windows. Adapter over
//! [`BrowserHandle`](browser::BrowserHandle); it does not own Browser, pages,
//! or the cookie jar.

use std::collections::HashMap;
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{Method, StatusCode, Uri};
use axum::response::{IntoResponse, Json, Response};
use browser::{BrowserHandle, PageError, PageHandle, RemoteValue, ScriptFailure};
use serde_json::{Value, json};

pub use browser::AgentBuilder;

const ELEMENT_KEY: &str = "element-6066-11e4-a52e-4f735466cecf";
const DEFAULT_SCRIPT_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_PAGE_LOAD_TIMEOUT: Duration = Duration::from_secs(300);

/// Serves classic `WebDriver` on `listener` until the process exits.
///
/// One active HTTP session. Pages live on `browser`.
///
/// # Errors
///
/// Returns when the listener cannot be converted or serving fails.
pub fn serve(listener: &TcpListener, browser: BrowserHandle) -> std::io::Result<()> {
    let std_listener = listener.try_clone()?;
    std_listener.set_nonblocking(true)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let sessions = Arc::new(Mutex::new(Sessions {
        browser,
        next_session: 0,
        next_window: 0,
        open: HashMap::new(),
    }));
    let result: std::io::Result<()> = runtime.block_on(async move {
        let listener = tokio::net::TcpListener::from_std(std_listener)?;
        let app = Router::new()
            .fallback(dispatch_request)
            .with_state(AppState { sessions });
        axum::serve(listener, app).await
    });
    result
}

#[derive(Clone)]
struct AppState {
    sessions: Arc<Mutex<Sessions>>,
}

async fn dispatch_request(
    State(state): State<AppState>,
    method: Method,
    uri: Uri,
    body: Bytes,
) -> Response {
    let method = method.as_str().to_owned();
    let path = uri.path().to_owned();
    let body = String::from_utf8_lossy(&body).into_owned();
    let sessions = Arc::clone(&state.sessions);
    let result = tokio::task::spawn_blocking(move || {
        let mut sessions = sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        dispatch(&method, &path, &body, &mut sessions)
    })
    .await;
    let Ok((status, payload)) = result else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "value": {
                    "error": "unknown error",
                    "message": "dispatch panicked",
                    "stacktrace": ""
                }
            })),
        )
            .into_response();
    };
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut response = (status, Json(payload)).into_response();
    // W3C WebDriver JSON is UTF-8; keep the charset the old adapter wrote.
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/json; charset=utf-8"),
    );
    response
}

struct Sessions {
    browser: BrowserHandle,
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
    page: PageHandle,
}

impl Sessions {
    fn blank_window(&self) -> Result<Window, String> {
        let page = self.browser.create_page().map_err(|err| err.to_string())?;
        if let Err(error) = page.load_html("<!doctype html><title></title>") {
            let _ = self.browser.close_page(page.id());
            return Err(error.to_string());
        }
        Ok(Window { page })
    }

    fn create(&mut self) -> Result<String, String> {
        self.next_session += 1;
        let id = format!("s{}", self.next_session);
        self.next_window += 1;
        let handle = format!("w{}", self.next_window);
        let window = self.blank_window()?;
        self.open.insert(
            id.clone(),
            Session {
                current: handle.clone(),
                windows: HashMap::from([(handle, window)]),
                script_timeout: DEFAULT_SCRIPT_TIMEOUT,
                page_load_timeout: DEFAULT_PAGE_LOAD_TIMEOUT,
            },
        );
        Ok(id)
    }
}

fn dispatch(method: &str, path: &str, body: &str, sessions: &mut Sessions) -> (u16, Value) {
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    match (method, segments.as_slice()) {
        ("GET", ["status"]) => ok(json!({"ready": true, "message": "ready"})),
        ("POST", ["session"]) => {
            if !sessions.open.is_empty() {
                return error(500, "session not created", "already have a session");
            }
            match sessions.create() {
                Ok(id) => {
                    ok(json!({"sessionId": id, "capabilities": {"browserName": "tinybrowser"}}))
                }
                Err(err) => error(500, "session not created", &err),
            }
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
        ("POST", ["session", _session, "element", _, "click"]) => {
            error(500, "unsupported operation", "element click")
        }
        ("POST", ["session", _session, "actions"]) => {
            error(500, "unsupported operation", "actions")
        }
        ("DELETE", ["session", _session, "actions"]) => {
            error(500, "unsupported operation", "release actions")
        }
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
    let Some(window) = current(sessions, session) else {
        return error(404, "invalid session id", session);
    };
    match window.page.goto(&url) {
        Ok(()) => {
            match window.page.run_until_load_timeout(page_load_timeout) {
                Ok(false) => return error(500, "timeout", "navigation timed out"),
                Ok(true) => {}
                Err(err) => return error(500, "unknown error", &err.to_string()),
            }
            match window.page.last_navigation_failed() {
                Ok(true) => error(500, "unknown error", "navigation failed"),
                Ok(false) => ok(Value::Null),
                Err(err) => error(500, "unknown error", &err.to_string()),
            }
        }
        Err(PageError::InvalidUrl { spec }) => error(400, "invalid argument", &spec),
        Err(err) => error(500, "unknown error", &err.to_string()),
    }
}

fn current_url(sessions: &Sessions, session: &str) -> (u16, Value) {
    match current(sessions, session) {
        Some(window) => match window.page.document_url() {
            Ok(url) => ok(json!(url)),
            Err(err) => error(500, "unknown error", &err.to_string()),
        },
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
    let Some(window) = current(sessions, session) else {
        return error(404, "invalid session id", session);
    };
    let started = Instant::now();
    let wrapped = wrap_script(script, &args, asynchronous);
    match window
        .page
        .execute_script_timeout(&wrapped, Some(script_timeout))
    {
        Err(err) => script_error(&err),
        Ok(_) if asynchronous => wait_for_async(window, remaining(started, script_timeout)),
        Ok(value) => match window.page.execute_script("globalThis.__wd_wait === true") {
            Ok(RemoteValue::Bool(true)) => {
                wait_for_async(window, remaining(started, script_timeout))
            }
            Ok(_) => ok(encode(&value)),
            Err(err) => script_error(&err),
        },
    }
}

fn remaining(started: Instant, budget: Duration) -> Duration {
    budget.saturating_sub(started.elapsed())
}

fn wait_for_async(window: &Window, script_timeout: Duration) -> (u16, Value) {
    match window
        .page
        .run_until_js_true("globalThis.__wd_done === true", script_timeout)
    {
        Ok(false) => return error(500, "script timeout", "script timeout"),
        Ok(true) => {}
        Err(err) => return error(500, "unknown error", &err.to_string()),
    }
    match window
        .page
        .execute_script("globalThis.__wd_failed === true")
    {
        Ok(RemoteValue::Bool(true)) => {
            let message = match window.page.execute_script("String(globalThis.__wd_err)") {
                Ok(RemoteValue::String(text)) => text,
                Ok(_) => "javascript error".to_owned(),
                Err(err) => return script_error(&err),
            };
            return error(500, "javascript error", &message);
        }
        Ok(_) => {}
        Err(err) => return script_error(&err),
    }
    match window.page.execute_script("globalThis.__wd_async") {
        Ok(value) => ok(encode(&value)),
        Err(err) => script_error(&err),
    }
}

fn script_error(err: &PageError) -> (u16, Value) {
    match err {
        PageError::Script(ScriptFailure::Interrupted) => {
            error(500, "script timeout", "script timeout")
        }
        _ => error(500, "javascript error", &err.to_string()),
    }
}

fn wrap_script(script: &str, args: &Value, asynchronous: bool) -> String {
    let args_json = args.to_string();
    if asynchronous {
        format!(
            "globalThis.__wd_async = undefined;\n\
             globalThis.__wd_err = undefined;\n\
             globalThis.__wd_failed = false;\n\
             globalThis.__wd_done = false;\n\
             globalThis.__wd_wait = false;\n\
             (function() {{ {script} }}).apply(null, {args_json}.concat([function(v) {{ \
               globalThis.__wd_async = v === undefined ? null : v; \
               globalThis.__wd_done = true; \
             }}]));"
        )
    } else {
        format!(
            "globalThis.__wd_async = undefined;\n\
             globalThis.__wd_err = undefined;\n\
             globalThis.__wd_failed = false;\n\
             globalThis.__wd_done = false;\n\
             globalThis.__wd_wait = false;\n\
             (function() {{\n\
               var result = (function() {{ {script} }}).apply(null, {args_json});\n\
               if (result && typeof result.then === 'function') {{\n\
                 result.then(function(v) {{\n\
                   globalThis.__wd_async = v === undefined ? null : v;\n\
                   globalThis.__wd_done = true;\n\
                 }}, function(e) {{\n\
                   globalThis.__wd_failed = true;\n\
                   globalThis.__wd_err = e == null ? 'undefined' : (e && e.message ? String(e.message) : String(e));\n\
                   globalThis.__wd_done = true;\n\
                 }});\n\
                 globalThis.__wd_wait = true;\n\
                 return null;\n\
               }}\n\
               return result;\n\
             }})()"
        )
    }
}

fn encode(value: &RemoteValue) -> Value {
    match value {
        RemoteValue::Undefined | RemoteValue::Null => Value::Null,
        RemoteValue::Bool(flag) => json!(flag),
        RemoteValue::Number(number) => {
            if number.is_finite() {
                json!(number)
            } else {
                Value::Null
            }
        }
        RemoteValue::String(text) => json!(text),
        RemoteValue::List(items) => Value::Array(items.iter().map(encode).collect()),
        RemoteValue::Map(entries) => {
            let mut map = serde_json::Map::new();
            for (key, item) in entries {
                map.insert(key.clone(), encode(item));
            }
            Value::Object(map)
        }
        RemoteValue::Node(id) => json!({ ELEMENT_KEY: id.to_string() }),
    }
}

fn get_timeouts(sessions: &Sessions, session: &str) -> (u16, Value) {
    match sessions.open.get(session) {
        Some(found) => ok(json!({
            "implicit": 0,
            "pageLoad": duration_millis(found.page_load_timeout),
            "script": duration_millis(found.script_timeout),
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

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn json_millis(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => {
            if let Some(ms) = number.as_u64() {
                Some(ms)
            } else if let Some(ms) = number.as_i64() {
                u64::try_from(ms).ok()
            } else {
                let ms = number.as_f64()?;
                if !ms.is_finite() || ms < 0.0 {
                    return None;
                }
                let duration = Duration::try_from_secs_f64(ms / 1000.0).ok()?;
                u64::try_from(duration.as_millis()).ok()
            }
        }
        _ => None,
    }
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
    let window = match sessions.blank_window() {
        Ok(window) => window,
        Err(err) => return error(500, "unknown error", &err),
    };
    let Some(found) = sessions.open.get_mut(session) else {
        return error(404, "invalid session id", session);
    };
    found.windows.insert(handle.clone(), window);
    ok(json!({"handle": handle, "type": "window"}))
}

fn close_window(sessions: &mut Sessions, session: &str) -> (u16, Value) {
    let (page_id, remaining) = {
        let Some(found) = sessions.open.get_mut(session) else {
            return error(404, "invalid session id", session);
        };
        let Some(window) = found.windows.remove(&found.current) else {
            return error(404, "no such window", &found.current);
        };
        let page_id = window.page.id();
        let remaining: Vec<String> = found.windows.keys().cloned().collect();
        if let Some(next) = remaining.first() {
            found.current.clone_from(next);
        }
        (page_id, remaining)
    };
    let _ = sessions.browser.close_page(page_id);
    if remaining.is_empty() {
        sessions.open.remove(session);
    }
    ok(json!(remaining))
}

fn current<'a>(sessions: &'a Sessions, session: &str) -> Option<&'a Window> {
    sessions
        .open
        .get(session)
        .and_then(|found| found.windows.get(&found.current))
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
