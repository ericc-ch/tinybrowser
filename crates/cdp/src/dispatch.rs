//! CDP dispatch helpers: session methods, runtime script shapes, and replies.
//!
//! Split out of [`crate`] so the socket state machine and the method surface
//! have separate reasons to change.

use std::io;
use std::time::Duration;

use browser::{BrowserHandle, TabError, TabEvent, TabHandle, TabId};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::DEFAULT_BROWSER_CONTEXT_ID;

pub(crate) async fn session_method(method: &str, tab: &TabHandle) -> Result<Value, DispatchError> {
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
pub(crate) async fn wait_for_navigation(
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

pub(crate) async fn open_url(tab: &TabHandle, url: &str) -> Result<(), DispatchError> {
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
pub(crate) fn arguments_expression(params: &Value) -> String {
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
pub(crate) fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_owned())
}

/// A failed runtime call, shaped like a CDP exception.
pub(crate) fn exception_reply(error: &TabError) -> Value {
    json!({
        "result": {"type": "undefined"},
        "exceptionDetails": {"text": error.to_string()},
    })
}

/// A failed runtime call with a literal message.
pub(crate) fn exception_text_reply(text: &str) -> Value {
    json!({
        "result": {"type": "undefined"},
        "exceptionDetails": {"text": text},
    })
}

/// Schedules `__SOURCE__` and captures its settled value.
pub(crate) const RUNTIME_SCHEDULE: &str = r#"(() => {
  globalThis.__tb_async = { done: false };
  Promise.resolve((__SOURCE__)).then(
    (v) => { globalThis.__tb_async.done = true; globalThis.__tb_async.value = v; },
    (e) => { globalThis.__tb_async.done = true; globalThis.__tb_async.error = String((e && e.message) || e); }
  );
  return "scheduled";
})()"#;

/// Evaluates `__SOURCE__`, stores a non-primitive in `__ID__`, and returns the
/// CDP `RemoteObject` JSON (primitives inline, per protocol).
pub(crate) const RUNTIME_HANDLE: &str = r#"(() => {
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
pub(crate) const RUNTIME_READ: &str = r#"(() => {
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

pub(crate) async fn target_info(browser: &BrowserHandle, id: TabId) -> Value {
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

pub(crate) fn target_id(value: Option<&Value>) -> Result<TabId, DispatchError> {
    let raw = value
        .and_then(Value::as_str)
        .ok_or_else(|| DispatchError::Failed("missing targetId".into()))?;
    let id = raw
        .parse::<u64>()
        .map_err(|_| DispatchError::Failed("invalid targetId".into()))?;
    Ok(TabId::new(id))
}

pub(crate) enum DispatchError {
    MethodNotFound,
    Failed(String),
}

pub(crate) fn attach_session(reply: &mut Value, session: Option<&str>) {
    if let Some(session) = session
        && let Some(object) = reply.as_object_mut()
    {
        object.insert("sessionId".into(), json!(session));
    }
}

pub(crate) fn ws_io(err: tungstenite::Error) -> io::Error {
    io::Error::other(err)
}

pub(crate) fn json_io(err: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err)
}
