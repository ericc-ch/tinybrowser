//! CDP dispatch helpers: session methods, runtime script shapes, and replies.
//!
//! Split out of [`crate`] so the socket state machine and the method surface
//! have separate reasons to change.

use std::io;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use browser::{
    BrowserHandle, DEFAULT_VIEWPORT, FrameId, RemoteValue, ScreenshotClip, ScreenshotRequest,
    TabError, TabEvent, TabHandle, TabId,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::DEFAULT_BROWSER_CONTEXT_ID;

/// Largest capture or emulated dimension the renderer accepts.
const MAX_SCREENSHOT_DIM: f64 = 4096.0;

pub(crate) async fn session_method(
    method: &str,
    params: &Value,
    tab: &TabHandle,
) -> Result<Value, DispatchError> {
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
        // Playwright dereferences `visualViewport.pageX/pageY/scale` before
        // every screenshot, so all three viewports are present, sized to the
        // tab's emulated viewport.
        "Page.getLayoutMetrics" => {
            let (width, height) = tab
                .viewport_size()
                .await
                .map_err(|error| DispatchError::Failed(error.to_string()))?;
            let (content_width, content_height) = tab
                .content_size()
                .await
                .map_err(|error| DispatchError::Failed(error.to_string()))?;
            let width = f64::from(width);
            let height = f64::from(height);
            let content_width = f64::from(content_width);
            let content_height = f64::from(content_height);
            Ok(json!({
                "layoutViewport": viewport_rect(width, height),
                "visualViewport": {
                    "offsetX": 0,
                    "offsetY": 0,
                    "pageX": 0,
                    "pageY": 0,
                    "clientWidth": width,
                    "clientHeight": height,
                    "scale": 1,
                },
                "contentSize": {"x": 0, "y": 0, "width": content_width, "height": content_height},
                "cssLayoutViewport": viewport_rect(width, height),
                "cssVisualViewport": {
                    "offsetX": 0,
                    "offsetY": 0,
                    "pageX": 0,
                    "pageY": 0,
                    "clientWidth": width,
                    "clientHeight": height,
                    "scale": 1,
                },
                "cssContentSize": {"x": 0, "y": 0, "width": content_width, "height": content_height},
            }))
        }
        "Page.addScriptToEvaluateOnNewDocument" => add_init_script(params, tab).await,
        "Page.removeScriptToEvaluateOnNewDocument" => remove_init_script(params, tab).await,
        "Emulation.setDeviceMetricsOverride" => set_device_metrics(params, tab).await,
        "Emulation.clearDeviceMetricsOverride" => clear_device_metrics(tab).await,
        "Page.reload" => {
            let url = tab
                .document_url()
                .await
                .map_err(|error| DispatchError::Failed(error.to_string()))?;
            open_url(tab, &url).await?;
            Ok(json!({}))
        }
        "Page.getResourceTree" => {
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
        "Runtime.getProperties" => runtime_get_properties(tab, params).await,
        "Page.getNavigationHistory" => navigation_history(tab).await,
        "Storage.getStorageKey" => storage_key(tab).await,
        _ => {
            if accepted_noop(method, params) {
                return Ok(json!({}));
            }
            Err(DispatchError::MethodNotFound)
        }
    }
}

/// `Page.getNavigationHistory`: the tab's committed session-history entries in
/// traversal order and the active index.
/// <https://chromedevtools.github.io/devtools-protocol/tot/Page/#method-getNavigationHistory>
async fn navigation_history(tab: &TabHandle) -> Result<Value, DispatchError> {
    let (entries, current_index) = tab
        .history()
        .await
        .map_err(|error| DispatchError::Failed(error.to_string()))?;
    let entries: Vec<Value> = entries
        .into_iter()
        .map(|entry| {
            json!({
                "id": entry.id,
                "url": entry.url,
                "userTypedURL": entry.url,
                "title": "",
                // The engine does not record how a navigation was initiated.
                // "other" is the protocol's explicit unknown value.
                "transitionType": "other",
            })
        })
        .collect();
    Ok(json!({"currentIndex": current_index, "entries": entries}))
}

/// `Runtime.getProperties`: own and inherited properties of a page-side handle,
/// plus its `[[Prototype]]` internal property. Accessors are
/// reported as `get`/`set` pairs; getters never run. `privateProperties` is
/// absent because JavaScript has no reflection API for private names.
/// <https://chromedevtools.github.io/devtools-protocol/tot/Runtime/#method-getProperties>
async fn runtime_get_properties(tab: &TabHandle, params: &Value) -> Result<Value, DispatchError> {
    let object_id = params
        .get("objectId")
        .and_then(Value::as_str)
        .ok_or_else(|| DispatchError::Failed("Runtime.getProperties requires objectId".into()))?;
    let own = params
        .get("ownProperties")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let accessors_only = params
        .get("accessorPropertiesOnly")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let generate_preview = params
        .get("generatePreview")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let script = RUNTIME_GET_PROPERTIES
        .replace("__ID__", &json_string(object_id))
        .replace("__OWN__", if own { "true" } else { "false" })
        .replace(
            "__ACCESSORS__",
            if accessors_only { "true" } else { "false" },
        )
        .replace(
            "__PREVIEW__",
            if generate_preview { "true" } else { "false" },
        );
    let value = tab
        .execute_browser_script(&script)
        .await
        .map_err(|error| DispatchError::Failed(error.to_string()))?;
    let RemoteValue::String(text) = value else {
        return Err(DispatchError::Failed(
            "Runtime.getProperties did not return JSON".into(),
        ));
    };
    let parsed: Value =
        serde_json::from_str(&text).map_err(|error| DispatchError::Failed(error.to_string()))?;
    if parsed.get("missing").and_then(Value::as_bool) == Some(true) {
        return Err(DispatchError::Failed(
            "Could not find object with given id".into(),
        ));
    }
    Ok(parsed)
}

/// Commands this engine accepts as no-ops, consulted only after every real
/// handler in both routing paths. An entry is honest only when the command's
/// effect is already the engine's fixed behavior, or when it is an event
/// subscription for a domain that never emits events.
///
/// State-changing commands are deliberately absent: script disabling, extra
/// headers, CSP bypass, permissions, interception, and cache toggles return
/// `MethodNotFound` instead of reporting an effect that did not happen.
pub(crate) fn accepted_noop(method: &str, params: &Value) -> bool {
    const UNCONDITIONAL: [&str; 12] = [
        // Playwright sends this while initializing every page session. The
        // engine never pauses a script, so there is no waiter to release.
        "Runtime.runIfWaitingForDebugger",
        // Subscription toggles for domains with no events to deliver.
        "Log.enable",
        "Network.enable",
        // Focus is not an engine state: there is no focused/unfocused
        // document distinction, so the request changes nothing.
        "Emulation.setFocusEmulationEnabled",
        // A page target has no child targets. The browser session's
        // `Target.setAutoAttach` is a real handler.
        "Target.setAutoAttach",
        // The renderer owns its font stack; there is no font configuration.
        "Page.setFontFamilies",
        // There is no window or window manager, so bounds have no effect.
        "Browser.setWindowBounds",
        "Browser.getWindowBounds",
        // The engine has no download pipeline, so no download can start and
        // there is no behavior to configure. Playwright's connectOverCDP
        // always sends this for its default context; refusing it aborts the
        // connection before any page command runs.
        "Browser.setDownloadBehavior",
        // The engine has no HTTP cache: clearing it is already true.
        "Network.clearBrowserCache",
        // Pure subscription toggles for domains with no events.
        "Overlay.enable",
        "Overlay.disable",
    ];
    match method {
        // Playwright's initial call is media "" with every feature empty,
        // which selects the engine's existing defaults. A real override
        // needs media-query state and returns `MethodNotFound`.
        "Emulation.setEmulatedMedia" => emulated_media_is_default(params),
        // With no cache there is nothing to disable; claiming caching could
        // be turned back on would be false.
        "Network.setCacheDisabled" => {
            params.get("cacheDisabled").and_then(Value::as_bool) == Some(true)
        }
        _ => UNCONDITIONAL.contains(&method),
    }
}

/// True when `Emulation.setEmulatedMedia` selects the engine defaults: empty
/// media and every feature value empty.
fn emulated_media_is_default(params: &Value) -> bool {
    if params
        .get("media")
        .and_then(Value::as_str)
        .is_some_and(|media| !media.is_empty())
    {
        return false;
    }
    match params.get("features") {
        None | Some(Value::Null) => true,
        Some(Value::Array(features)) => features.iter().all(|feature| {
            feature
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or("")
                .is_empty()
        }),
        Some(_) => false,
    }
}

/// `Page.addScriptToEvaluateOnNewDocument`: register `source` page-wide and
/// return its identifier.
///
/// `worldName` is accepted but ignored: this engine has no isolated worlds,
/// so scripts (including Playwright's utility-world registration, whose
/// source is empty) run in the main world. That is a known isolation gap
/// (docs/progress.md).
async fn add_init_script(params: &Value, tab: &TabHandle) -> Result<Value, DispatchError> {
    let source = params
        .get("source")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let run_immediately = params
        .get("runImmediately")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let id = tab
        .add_init_script(source, run_immediately)
        .await
        .map_err(|error| DispatchError::Failed(error.to_string()))?;
    Ok(json!({"identifier": id.to_string()}))
}

/// `Page.removeScriptToEvaluateOnNewDocument`: forget a registered script.
async fn remove_init_script(params: &Value, tab: &TabHandle) -> Result<Value, DispatchError> {
    if let Some(id) = params
        .get("identifier")
        .and_then(Value::as_str)
        .and_then(|value| value.parse::<u64>().ok())
    {
        tab.remove_init_script(id)
            .await
            .map_err(|error| DispatchError::Failed(error.to_string()))?;
    }
    Ok(json!({}))
}

/// `Emulation.setDeviceMetricsOverride`: persist the emulated viewport on the
/// tab and the renderer. The protocol requires integer `width`/`height`, and
/// zero on either side clears the override; `mobile`, non-default
/// `deviceScaleFactor`, and sizes beyond the renderer's cap are refused
/// rather than silently approximated
/// (<https://chromedevtools.github.io/devtools-protocol/tot/Emulation/#method-setDeviceMetricsOverride>).
async fn set_device_metrics(params: &Value, tab: &TabHandle) -> Result<Value, DispatchError> {
    let width = required_metric(params, "width")?;
    let height = required_metric(params, "height")?;
    if width == 0 || height == 0 {
        return clear_device_metrics(tab).await;
    }
    if params.get("mobile").and_then(Value::as_bool) == Some(true) {
        return Err(DispatchError::Failed(
            "mobile emulation is not supported".into(),
        ));
    }
    if params
        .get("deviceScaleFactor")
        .and_then(Value::as_f64)
        .is_some_and(|scale| (scale - 1.0).abs() > f64::EPSILON)
    {
        return Err(DispatchError::Failed(
            "device scale factors other than 1 are not supported".into(),
        ));
    }
    if f64::from(width) > MAX_SCREENSHOT_DIM || f64::from(height) > MAX_SCREENSHOT_DIM {
        return Err(DispatchError::Failed(format!(
            "viewport exceeds the {MAX_SCREENSHOT_DIM} pixel cap"
        )));
    }
    set_viewport(tab, width, height).await
}

/// `Emulation.clearDeviceMetricsOverride`: restore the default viewport.
async fn clear_device_metrics(tab: &TabHandle) -> Result<Value, DispatchError> {
    set_viewport(tab, DEFAULT_VIEWPORT.0, DEFAULT_VIEWPORT.1).await
}

/// Applies one viewport size to the tab.
async fn set_viewport(tab: &TabHandle, width: u32, height: u32) -> Result<Value, DispatchError> {
    tab.set_viewport(width, height)
        .await
        .map_err(|error| DispatchError::Failed(error.to_string()))?;
    Ok(json!({}))
}

/// One required integer emulation metric.
fn required_metric(params: &Value, name: &str) -> Result<u32, DispatchError> {
    params
        .get(name)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| {
            DispatchError::Failed(format!(
                "Emulation.setDeviceMetricsOverride requires an integer {name}"
            ))
        })
}

/// Capture surface side from a clip extent: at least 1, at most the renderer
/// cap, so a bad client value cannot ask for an unpaintable surface.
fn capture_side(extent: f64) -> u32 {
    if !extent.is_finite() || extent < 1.0 {
        return 1;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "extent is finite, positive, and capped at MAX_SCREENSHOT_DIM"
    )]
    return extent.min(MAX_SCREENSHOT_DIM) as u32;
}

#[cfg(test)]
mod tests {
    use super::{accepted_noop, capture_side, required_metric};
    use serde_json::json;

    #[test]
    fn emulation_metrics_require_integers() {
        let params = json!({"width": 1024, "height": 768});
        assert_eq!(required_metric(&params, "width").expect("width"), 1024);
        assert_eq!(required_metric(&params, "height").expect("height"), 768);
        assert!(required_metric(&json!({}), "width").is_err());
        assert!(required_metric(&json!({"width": 10.5}), "width").is_err());
        assert!(required_metric(&json!({"width": -1}), "width").is_err());
    }

    #[test]
    fn capture_sides_are_bounded() {
        assert_eq!(capture_side(f64::NAN), 1);
        assert_eq!(capture_side(0.0), 1);
        assert_eq!(capture_side(1500.0), 1500);
        assert_eq!(capture_side(10_000.0), 4096);
    }

    #[test]
    fn default_emulated_media_is_the_only_accepted_shape() {
        assert!(accepted_noop(
            "Emulation.setEmulatedMedia",
            &json!({"media": "", "features": [
                {"name": "prefers-color-scheme", "value": ""},
                {"name": "prefers-reduced-motion", "value": ""},
            ]})
        ));
        assert!(accepted_noop(
            "Emulation.setEmulatedMedia",
            &json!({"media": ""})
        ));
        assert!(!accepted_noop(
            "Emulation.setEmulatedMedia",
            &json!({"media": "print"})
        ));
        assert!(!accepted_noop(
            "Emulation.setEmulatedMedia",
            &json!({"media": "", "features": [
                {"name": "prefers-color-scheme", "value": "dark"},
            ]})
        ));
    }

    #[test]
    fn state_changing_commands_are_not_accepted() {
        for method in [
            "Emulation.setScriptExecutionDisabled",
            "Network.setExtraHTTPHeaders",
            "Network.clearBrowserCookies",
            "Page.setBypassCSP",
            "Fetch.enable",
            "Fetch.disable",
            "Runtime.addBinding",
            "Security.setIgnoreCertificateErrors",
            "Page.startScreenRecording",
            "Page.stopScreenRecording",
            "Browser.grantPermissions",
            "Browser.resetPermissions",
            "Page.setInterceptFileChooserDialog",
        ] {
            assert!(
                !accepted_noop(method, &json!({})),
                "{method} must go through a real handler or fail"
            );
        }
        assert!(!accepted_noop(
            "Network.setCacheDisabled",
            &json!({"cacheDisabled": false})
        ));
        assert!(accepted_noop(
            "Network.setCacheDisabled",
            &json!({"cacheDisabled": true})
        ));
    }
}

/// `Storage.getStorageKey`: the tab's origin.
async fn storage_key(tab: &TabHandle) -> Result<Value, DispatchError> {
    let value = tab
        .execute_browser_script("String(location.origin)")
        .await
        .map_err(|error| DispatchError::Failed(error.to_string()))?;
    let key = match value {
        RemoteValue::String(key) => key,
        _ => String::new(),
    };
    Ok(json!({"storageKey": key}))
}

/// `DOM.getDocument`: serialize the live tree, remembering each node so later
/// DOM calls can look it up by `nodeId`.
pub(crate) async fn dom_get_document(tab: &TabHandle) -> Result<Value, DispatchError> {
    const SCRIPT: &str = r#"(function(){
      host.__tb_dom_nodes = host.array();
      host.__tb_dom_id = 0;
      const register = node => {
        const nodes = host.__tb_dom_nodes;
        for (let id = 1; id < nodes.length; id++) { if (nodes[id] === node) return id; }
        const id = ++host.__tb_dom_id;
        nodes[id] = node;
        return id;
      };
      const walk = node => {
        const id = register(node);
        const entry = { __proto__: null, nodeId: id, backendNodeId: id, nodeType: node.nodeType, nodeName: node.nodeName };
        if (node.nodeType === 1) { entry.localName = node.localName; entry.nodeValue = ""; entry.attributes = host.array(); }
        else if (node.nodeType === 9) {
          entry.nodeValue = ""; entry.documentURL = node.URL; entry.baseURL = node.baseURI;
          entry.compatibilityMode = "NoQuirks";
        } else { entry.nodeValue = node.nodeValue || ""; }
        const children = host.array();
        for (let child = node.firstChild; child; child = child.nextSibling) host.push(children, walk(child));
        if (children.length) entry.children = children;
        entry.childNodeCount = children.length;
        return entry;
      };
      const root = walk(document);
      return host.stringify(root);
    })()"#;
    let value = tab
        .execute_browser_script(SCRIPT)
        .await
        .map_err(|error| DispatchError::Failed(error.to_string()))?;
    let RemoteValue::String(text) = value else {
        return Err(DispatchError::Failed(
            "DOM.getDocument could not serialize the document".into(),
        ));
    };
    let root: Value =
        serde_json::from_str(&text).map_err(|error| DispatchError::Failed(error.to_string()))?;
    Ok(json!({"root": root}))
}

/// `Input.dispatchMouseEvent`: one pointer step through the page-side
/// performer, preceded by a move so press/release land on the right target.
pub(crate) async fn input_mouse_event(
    tab: &TabHandle,
    params: &Value,
) -> Result<Value, DispatchError> {
    let kind = params.get("type").and_then(Value::as_str).unwrap_or("");
    let x = params.get("x").and_then(Value::as_f64).unwrap_or(0.0);
    let y = params.get("y").and_then(Value::as_f64).unwrap_or(0.0);
    let button = match params
        .get("button")
        .and_then(Value::as_str)
        .unwrap_or("left")
    {
        "middle" => 1,
        "right" => 2,
        "back" => 3,
        "forward" => 4,
        _ => 0,
    };
    if kind == "mouseWheel" {
        let actions = json!({"actions": [{"type": "wheel", "id": "wheel", "actions": [{
            "type": "scroll", "x": x, "y": y,
            "deltaX": params.get("deltaX").and_then(Value::as_f64).unwrap_or(0.0),
            "deltaY": params.get("deltaY").and_then(Value::as_f64).unwrap_or(0.0),
        }]}]});
        return run_actions(tab, &actions).await;
    }
    let mut items = vec![json!({"type": "pointerMove", "origin": "viewport", "x": x, "y": y})];
    match kind {
        "mousePressed" => items.push(json!({"type": "pointerDown", "button": button})),
        "mouseReleased" => items.push(json!({"type": "pointerUp", "button": button})),
        _ => {}
    }
    let actions = json!({"actions": [{
        "type": "pointer", "id": "mouse", "parameters": {"pointerType": "mouse"},
        "actions": items,
    }]});
    run_actions(tab, &actions).await
}

/// `Input.dispatchKeyEvent`: one key step through the page-side performer.
/// `char` events carry their text in `text`.
pub(crate) async fn input_key_event(
    tab: &TabHandle,
    params: &Value,
) -> Result<Value, DispatchError> {
    let kind = params.get("type").and_then(Value::as_str).unwrap_or("");
    let key = params.get("key").and_then(Value::as_str).unwrap_or("");
    let text = params.get("text").and_then(Value::as_str).unwrap_or("");
    let value = if kind == "char" { text } else { key };
    if value.is_empty() {
        return Ok(json!({}));
    }
    let down = kind != "keyUp";
    let actions = json!({"actions": [{"type": "key", "id": "keyboard", "actions": [{
        "type": if down { "keyDown" } else { "keyUp" },
        "value": value,
    }]}]});
    run_actions(tab, &actions).await
}

/// `Input.insertText`: type into the focused element.
pub(crate) async fn input_insert_text(
    tab: &TabHandle,
    params: &Value,
) -> Result<Value, DispatchError> {
    let text = params.get("text").and_then(Value::as_str).unwrap_or("");
    if text.is_empty() {
        return Ok(json!({}));
    }
    let actions = json!({"actions": [{"type": "key", "id": "keyboard", "actions": [
        {"type": "insertText", "value": text},
    ]}]});
    run_actions(tab, &actions).await
}

/// `Input.dispatchTouchEvent`: the first touch point as a touch pointer.
pub(crate) async fn input_touch_event(
    tab: &TabHandle,
    params: &Value,
) -> Result<Value, DispatchError> {
    let kind = params.get("type").and_then(Value::as_str).unwrap_or("");
    let Some(point) = params
        .get("touchPoints")
        .and_then(Value::as_array)
        .and_then(|points| points.first())
    else {
        return Ok(json!({}));
    };
    let x = point.get("x").and_then(Value::as_f64).unwrap_or(0.0);
    let y = point.get("y").and_then(Value::as_f64).unwrap_or(0.0);
    let mut items = vec![json!({"type": "pointerMove", "origin": "viewport", "x": x, "y": y})];
    match kind {
        "touchStart" => items.push(json!({"type": "pointerDown", "button": 0})),
        "touchEnd" => items.push(json!({"type": "pointerUp", "button": 0})),
        "touchCancel" => items.push(json!({"type": "pointerCancel"})),
        _ => {}
    }
    let actions = json!({"actions": [{
        "type": "pointer", "id": "touch", "parameters": {"pointerType": "touch"},
        "actions": items,
    }]});
    run_actions(tab, &actions).await
}

/// `Input.emulateTouchFromMouseEvent`: the mouse shape, touch-typed.
pub(crate) async fn input_emulate_touch_from_mouse(
    tab: &TabHandle,
    params: &Value,
) -> Result<Value, DispatchError> {
    let kind = params.get("type").and_then(Value::as_str).unwrap_or("");
    let x = params.get("x").and_then(Value::as_f64).unwrap_or(0.0);
    let y = params.get("y").and_then(Value::as_f64).unwrap_or(0.0);
    let mut items = vec![json!({"type": "pointerMove", "origin": "viewport", "x": x, "y": y})];
    match kind {
        "mousePressed" => items.push(json!({"type": "pointerDown", "button": 0})),
        "mouseReleased" => items.push(json!({"type": "pointerUp", "button": 0})),
        _ => {}
    }
    let actions = json!({"actions": [{
        "type": "pointer", "id": "touch", "parameters": {"pointerType": "touch"},
        "actions": items,
    }]});
    run_actions(tab, &actions).await
}

/// Runs a page-side action sequence and reports the CDP-shaped reply.
async fn run_actions(tab: &TabHandle, actions: &Value) -> Result<Value, DispatchError> {
    // The page performer takes the source array; callers may pass either the
    // array itself or a `{"actions": [...]}` envelope.
    let sources = actions.get("actions").unwrap_or(actions);
    let script = format!(
        "(function(){{return host.__tbWebDriverActions({});}})()",
        serde_json::to_string(sources).unwrap_or_else(|_| "[]".to_owned())
    );
    tab.execute_browser_script(&script)
        .await
        .map_err(|error| DispatchError::Failed(error.to_string()))?;
    Ok(json!({}))
}

/// Runs a DOM helper script that returns a JSON string and parses the reply.
async fn dom_eval(tab: &TabHandle, script: &str) -> Result<Value, DispatchError> {
    let value = tab
        .execute_browser_script(script)
        .await
        .map_err(|error| DispatchError::Failed(error.to_string()))?;
    let RemoteValue::String(text) = value else {
        return Err(DispatchError::Failed(
            "the DOM helper did not return JSON".into(),
        ));
    };
    serde_json::from_str(&text).map_err(|error| DispatchError::Failed(error.to_string()))
}

/// The `nodeId` a DOM method targets, defaulting to the document node.
fn requested_node(params: &Value) -> u64 {
    params.get("nodeId").and_then(Value::as_u64).unwrap_or(1)
}

/// A page-side expression resolving the node a DOM method targets, from any of
/// `objectId` (a remote handle), `backendNodeId`, or `nodeId`.
fn requested_node_expression(params: &Value) -> String {
    if let Some(id) = params.get("objectId").and_then(Value::as_str) {
        return format!("(host.__tb_handles || {{}})[{}]", json_string(id));
    }
    let id = params
        .get("backendNodeId")
        .and_then(Value::as_u64)
        .or_else(|| params.get("nodeId").and_then(Value::as_u64))
        .unwrap_or(1);
    format!("(host.__tb_dom_nodes || [])[{id}]")
}

/// `DOM.querySelector`/`DOM.querySelectorAll`: register the matches in the
/// page's node table and answer with their ids.
pub(crate) async fn dom_query_selector(
    tab: &TabHandle,
    params: &Value,
    all: bool,
) -> Result<Value, DispatchError> {
    const SINGLE: &str = r"(function(){
      const register = node => {
        const nodes = host.__tb_dom_nodes;
        for (let id = 1; id < nodes.length; id++) { if (nodes[id] === node) return id; }
        const id = ++host.__tb_dom_id;
        nodes[id] = node;
        return id;
      };
      const n = host.__tb_dom_nodes[__NODE__];
      if (!n || !n.querySelector) return host.stringify({__proto__: null, nodeId: 0});
      const found = n.querySelector(__SELECTOR__);
      if (!found) return host.stringify({__proto__: null, nodeId: 0});
      return host.stringify({__proto__: null, nodeId: register(found)});
    })()";
    const ALL: &str = r"(function(){
      const register = node => {
        const nodes = host.__tb_dom_nodes;
        for (let id = 1; id < nodes.length; id++) { if (nodes[id] === node) return id; }
        const id = ++host.__tb_dom_id;
        nodes[id] = node;
        return id;
      };
      const n = host.__tb_dom_nodes[__NODE__];
      if (!n || !n.querySelectorAll) return host.stringify({__proto__: null, nodeIds: []});
      const nodeIds = host.array();
      for (const found of n.querySelectorAll(__SELECTOR__)) host.push(nodeIds, register(found));
      return host.stringify({__proto__: null, nodeIds: nodeIds});
    })()";
    let selector =
        serde_json::to_string(params.get("selector").and_then(Value::as_str).unwrap_or(""))
            .unwrap_or_else(|_| "\"\"".to_owned());
    let script = (if all { ALL } else { SINGLE })
        .replace("__NODE__", &requested_node(params).to_string())
        .replace("__SELECTOR__", &selector);
    dom_eval(tab, &script).await
}

/// `DOM.describeNode`: the stored node's shape, optionally with children.
pub(crate) async fn dom_describe_node(
    tab: &TabHandle,
    params: &Value,
) -> Result<Value, DispatchError> {
    const TEMPLATE: &str = r"(function(){
      const n = host.__tb_dom_nodes[__NODE__];
      if (!n) return host.stringify({__proto__: null, node: null});
      const entry = { __proto__: null,
        nodeId: __NODE__, backendNodeId: __NODE__,
        nodeType: n.nodeType, nodeName: n.nodeName,
        childNodeCount: n.childNodes ? n.childNodes.length : 0,
      };
      if (n.nodeType === 1) { entry.localName = n.localName; entry.attributes = host.array(); }
      return host.stringify({__proto__: null, node: entry});
    })()";
    let script = TEMPLATE.replace("__NODE__", &requested_node(params).to_string());
    dom_eval(tab, &script).await
}

/// `DOM.getOuterHTML` and `DOM.getAttributes` share the stored-node lookup.
pub(crate) async fn dom_node_string(
    tab: &TabHandle,
    params: &Value,
    field: &str,
) -> Result<Value, DispatchError> {
    const TEMPLATE: &str = r#"(function(){
      const n = host.__tb_dom_nodes[__NODE__];
      if (!n) return host.stringify({__proto__: null, __FIELD__: null});
      if ("__FIELD__" === "outerHTML") return host.stringify({__proto__: null, outerHTML: n.outerHTML === undefined ? "" : n.outerHTML});
      const attributes = host.array();
      if (n.attributes) for (const a of n.attributes) { host.push(attributes, a.name, a.value); }
      return host.stringify({__proto__: null, attributes: attributes});
    })()"#;
    let script = TEMPLATE
        .replace("__NODE__", &requested_node(params).to_string())
        .replace("__FIELD__", field);
    dom_eval(tab, &script).await
}

/// `DOM.resolveNode`: intern the node in the runtime handle table.
pub(crate) async fn dom_resolve_node(
    tab: &TabHandle,
    params: &Value,
) -> Result<Value, DispatchError> {
    const TEMPLATE: &str = r#"(function(){
      const n = host.__tb_dom_nodes[__NODE__];
      if (!n) return host.stringify({__proto__: null, object: {__proto__: null, type: "undefined"}});
      const objectId = "dom-__NODE__";
      host.__tb_handles[objectId] = n;
      return host.stringify({__proto__: null, object: {__proto__: null,
        type: "object", subtype: "node", objectId: objectId,
        className: n.constructor ? n.constructor.name : "Node",
        description: n.nodeName,
      }});
    })()"#;
    let script = TEMPLATE.replace("__NODE__", &requested_node(params).to_string());
    dom_eval(tab, &script).await
}

/// `DOM.getNodeForLocation`: hit-test the viewport point.
pub(crate) async fn dom_node_for_location(
    tab: &TabHandle,
    params: &Value,
) -> Result<Value, DispatchError> {
    const TEMPLATE: &str = r"(function(){
      const register = node => {
        const nodes = host.__tb_dom_nodes;
        for (let id = 1; id < nodes.length; id++) { if (nodes[id] === node) return id; }
        const id = ++host.__tb_dom_id;
        nodes[id] = node;
        return id;
      };
      const x = __X__, y = __Y__;
      const found = document.elementFromPoint ? document.elementFromPoint(x, y) : null;
      if (!found) return host.stringify({__proto__: null, nodeId: 0});
      return host.stringify({__proto__: null, nodeId: register(found)});
    })()";
    let script = TEMPLATE
        .replace(
            "__X__",
            &params
                .get("x")
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
                .to_string(),
        )
        .replace(
            "__Y__",
            &params
                .get("y")
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
                .to_string(),
        );
    dom_eval(tab, &script).await
}

/// `CSS.enable`: one `CSS.styleSheetAdded` per document stylesheet.
pub(crate) async fn css_stylesheets(tab: &TabHandle) -> Result<Vec<Value>, DispatchError> {
    const SCRIPT: &str = r#"(function(){
      host.__tb_css_id = host.__tb_css_id || 0;
      const out = host.array();
      for (const sheet of document.styleSheets) {
        const id = String(++host.__tb_css_id);
        host.push(out, {__proto__: null,
          styleSheetId: id,
          sourceURL: sheet.href || "",
          origin: "regular",
          title: sheet.title || "",
          disabled: !!sheet.disabled,
          isInline: !sheet.href,
          startLine: 0, startColumn: 0, endLine: 0, endColumn: 0,
          length: sheet.cssRules ? sheet.cssRules.length : 0,
        });
      }
      return host.stringify(out);
    })()"#;
    let value = dom_eval(tab, SCRIPT).await?;
    Ok(value.as_array().cloned().unwrap_or_default())
}

/// `DOM.getBoxModel`: the node's border box as content/padding/border/margin
/// quads (the engine has no separate boxes, so all four are the border box).
pub(crate) async fn dom_box_model(tab: &TabHandle, params: &Value) -> Result<Value, DispatchError> {
    const TEMPLATE: &str = r"(function(){
      const n = (__NODE__);
      if (!n || !n.getBoundingClientRect) return host.stringify({__proto__: null, model: null});
      const r = n.getBoundingClientRect();
      const quad = [r.left, r.top, r.right, r.top, r.right, r.bottom, r.left, r.bottom];
      return host.stringify({__proto__: null, model: {__proto__: null,
        content: quad, padding: quad, border: quad, margin: quad,
        width: Math.round(r.width), height: Math.round(r.height),
      }});
    })()";
    let script = TEMPLATE.replace("__NODE__", &requested_node_expression(params));
    dom_eval(tab, &script).await
}

/// `DOM.getContentQuads`: one quad per client rect of the node.
pub(crate) async fn dom_content_quads(
    tab: &TabHandle,
    params: &Value,
) -> Result<Value, DispatchError> {
    const TEMPLATE: &str = r"(function(){
      const n = (__NODE__);
      if (!n || !n.getClientRects) return host.stringify({__proto__: null, quads: []});
      const quads = host.array();
      for (const r of n.getClientRects()) {
        host.push(quads, [r.left, r.top, r.right, r.top, r.right, r.bottom, r.left, r.bottom]);
      }
      return host.stringify({__proto__: null, quads: quads});
    })()";
    let script = TEMPLATE.replace("__NODE__", &requested_node_expression(params));
    dom_eval(tab, &script).await
}

/// `CSS.getComputedStyleForNode`: the resolved style as name/value pairs.
pub(crate) async fn css_computed_style(
    tab: &TabHandle,
    params: &Value,
) -> Result<Value, DispatchError> {
    const TEMPLATE: &str = r"(function(){
      const n = host.__tb_dom_nodes[__NODE__];
      if (!n || !globalThis.getComputedStyle) return host.stringify({__proto__: null, computedStyle: []});
      const style = globalThis.getComputedStyle(n);
      const computedStyle = host.array();
      for (let i = 0; i < style.length; i++) {
        const name = style.item(i);
        host.push(computedStyle, {__proto__: null, name: name, value: style.getPropertyValue(name)});
      }
      return host.stringify({__proto__: null, computedStyle: computedStyle});
    })()";
    let script = TEMPLATE.replace("__NODE__", &requested_node(params).to_string());
    dom_eval(tab, &script).await
}

/// One viewport rectangle for `Page.getLayoutMetrics`.
fn viewport_rect(width: f64, height: f64) -> Value {
    json!({"pageX": 0, "pageY": 0, "clientWidth": width, "clientHeight": height})
}

/// `Page.captureScreenshot`: render the tab and return base64 PNG data.
///
/// Playwright always sends `clip`; it is forwarded to the renderer, which
/// rasters the requested viewport and crops before encoding. Non-PNG formats
/// are refused rather than mislabeled.
pub(crate) async fn capture_screenshot(
    tab: &TabHandle,
    params: &Value,
) -> Result<Value, DispatchError> {
    let format = params
        .get("format")
        .and_then(Value::as_str)
        .unwrap_or("png");
    if !format.eq_ignore_ascii_case("png") {
        return Err(DispatchError::Failed(
            "only png screenshots are supported".into(),
        ));
    }
    let clip = params
        .get("clip")
        .filter(|value| !value.is_null())
        .and_then(parse_clip);
    let viewport_width = capture_side(clip.map_or(f64::from(DEFAULT_VIEWPORT.0), |clip| {
        f64::from(clip.x + clip.width)
    }));
    let viewport_height = capture_side(clip.map_or(f64::from(DEFAULT_VIEWPORT.1), |clip| {
        f64::from(clip.y + clip.height)
    }));
    let request = ScreenshotRequest {
        viewport_width,
        viewport_height,
        clip,
    };
    let frame = requested_frame(tab, params)?;
    let png = tab
        .screenshot_frame(frame, request)
        .await
        .map_err(|error| DispatchError::Failed(error.to_string()))?;
    Ok(json!({"data": BASE64_STANDARD.encode(&png)}))
}

/// Resolves the optional renderer-frame extension used by automation clients
/// that need to address a child realm directly. Standard CDP calls omit it
/// and therefore address the main frame.
pub(crate) fn requested_frame(tab: &TabHandle, params: &Value) -> Result<FrameId, DispatchError> {
    let Some(raw) = params.get("frameId").and_then(Value::as_str) else {
        return Ok(FrameId::MAIN);
    };
    let target_id = tab.id().to_string();
    if raw == target_id || raw == "main" {
        return Ok(FrameId::MAIN);
    }
    let (_, renderer_frame) = raw
        .split_once('.')
        .filter(|(target, renderer_frame)| {
            *target == target_id && !renderer_frame.is_empty() && !renderer_frame.contains('.')
        })
        .ok_or_else(|| DispatchError::Failed("invalid frameId".into()))?;
    let renderer_frame = renderer_frame
        .parse::<u64>()
        .map_err(|_| DispatchError::Failed("invalid frameId".into()))?;
    if renderer_frame == FrameId::MAIN.get() {
        return Err(DispatchError::Failed("invalid frameId".into()));
    }
    Ok(FrameId::new(renderer_frame))
}

/// Narrows a clipped CDP coordinate to the renderer's `f32` viewport.
#[expect(
    clippy::cast_possible_truncation,
    reason = "callers clamp values to 4096 before narrowing; f32 is exact there"
)]
fn narrow(value: f64) -> f32 {
    value as f32
}

/// Parses Playwright's `clip` (`{x, y, width, height, scale}`).
fn parse_clip(value: &Value) -> Option<ScreenshotClip> {
    Some(ScreenshotClip {
        x: narrow(value.get("x")?.as_f64()?),
        y: narrow(value.get("y")?.as_f64()?),
        width: narrow(value.get("width")?.as_f64()?),
        height: narrow(value.get("height")?.as_f64()?),
    })
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
                Some(TabEvent::NavigationFailed) => {
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
                parts.push(format!("host.__tb_handles[{}]", json_string(id)));
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

/// Schedules `__SOURCE__` and captures its settled value. Results key by
/// evaluation ID; a single shared slot would let overlapping evaluations
/// overwrite each other.
pub(crate) const RUNTIME_SCHEDULE: &str = r#"(() => {
  const slot = { __proto__: null, done: false };
  host.__tb_async[__ID__] = slot;
  Promise.resolve((__SOURCE__)).then(
    (v) => { slot.done = true; slot.value = v; },
    (e) => { slot.done = true; slot.error = String((e && e.message) || e); }
  );
  return "scheduled";
})()"#;

/// Evaluates `__SOURCE__`, stores a non-primitive in `__ID__`, and returns the
/// CDP `RemoteObject` JSON (primitives inline, per protocol).
pub(crate) const RUNTIME_HANDLE: &str = r#"(() => {
  const v = (__SOURCE__);
  if (v === null) return host.stringify({__proto__: null, type: "object", subtype: "null", value: null });
  const t = typeof v;
  if (t === "undefined") return host.stringify({__proto__: null, type: "undefined" });
  if (t === "number") {
    if (Number.isFinite(v)) return host.stringify({__proto__: null, type: "number", value: v });
    return host.stringify({__proto__: null, type: "number", unserializableValue: Number.isNaN(v) ? "NaN" : (v > 0 ? "Infinity" : "-Infinity") });
  }
  if (t === "string" || t === "boolean") return host.stringify({__proto__: null, type: t, value: v });
  host.__tb_handles[__ID__] = v;
  // A DOM node is an `ElementHandle` only when the remote object says
  // `subtype: "node"` (CDP `Runtime.RemoteObject`).
  if (t === "object" && typeof v.nodeType === "number" && typeof v.nodeName === "string")
    return host.stringify({__proto__: null, type: "object", subtype: "node", className: (v.constructor && v.constructor.name) || "", objectId: __ID__ });
  return host.stringify({__proto__: null, type: t === "function" ? "function" : "object", objectId: __ID__ });
})()"#;

/// Schedules an awaited evaluate whose result becomes a handle (or an inline
/// primitive), the `evaluateHandle` shape: `awaitPromise` with
/// `returnByValue: false`. Results key by evaluation ID; a single shared slot
/// would let overlapping evaluations overwrite each other.
pub(crate) const RUNTIME_HANDLE_SCHEDULE: &str = r#"(() => {
  const slot = { __proto__: null, done: false };
  host.__tb_async_handles[__ID__] = slot;
  Promise.resolve((__SOURCE__)).then(
    (v) => { slot.done = true; slot.value = v; },
    (e) => { slot.done = true; slot.error = String((e && e.message) || e); }
  );
  return "scheduled";
})()"#;

/// Reads the awaited handle result and serializes it as a CDP `RemoteObject`.
pub(crate) const RUNTIME_HANDLE_READ: &str = r#"(() => {
  const slot = host.__tb_async_handles && host.__tb_async_handles[__ID__];
  if (!slot || !slot.done) return host.stringify({__proto__: null, pending: true });
  delete host.__tb_async_handles[__ID__];
  if (slot.error !== undefined) return host.stringify({__proto__: null, error: slot.error });
  const v = slot.value;
  if (v === null) return host.stringify({__proto__: null, type: "object", subtype: "null", value: null });
  const t = typeof v;
  if (t === "undefined") return host.stringify({__proto__: null, type: "undefined" });
  if (t === "number") {
    if (Number.isFinite(v)) return host.stringify({__proto__: null, type: "number", value: v });
    return host.stringify({__proto__: null, type: "number", unserializableValue: Number.isNaN(v) ? "NaN" : (v > 0 ? "Infinity" : "-Infinity") });
  }
  if (t === "string" || t === "boolean") return host.stringify({__proto__: null, type: t, value: v });
  host.__tb_handles[__ID__] = v;
  // A DOM node is an `ElementHandle` only when the remote object says
  // `subtype: "node"` (CDP `Runtime.RemoteObject`).
  if (t === "object" && typeof v.nodeType === "number" && typeof v.nodeName === "string")
    return host.stringify({__proto__: null, type: "object", subtype: "node", className: (v.constructor && v.constructor.name) || "", objectId: __ID__ });
  return host.stringify({__proto__: null, type: t === "function" ? "function" : "object", objectId: __ID__ });
})()"#;

/// Enumerates a page-side handle's own and inherited properties as CDP
/// `PropertyDescriptor` JSON. Accessor descriptors expose `get`/`set`
/// `RemoteObject`s instead of invoking them. Property values that are not
/// primitives become handles with a per-call unique `objectId` prefix.
pub(crate) const RUNTIME_GET_PROPERTIES: &str = r#"(() => {
  const objectId = __ID__;
  if (!Object.prototype.hasOwnProperty.call(host.__tb_handles, objectId))
    return host.stringify({__proto__: null, missing: true});
  const target = host.__tb_handles[objectId];
  const ownProperties = __OWN__;
  const accessorsOnly = __ACCESSORS__;
  const generatePreview = __PREVIEW__;
  host.__tb_prop_next = (host.__tb_prop_next || 0) + 1;
  const prefix = "prop-" + host.__tb_prop_next + "-";
  let next = 0;
  const remote = (value, preview) => {
    if (value === null) return {__proto__: null, type: "object", subtype: "null", value: null};
    const kind = typeof value;
    if (value === undefined) return {__proto__: null, type: "undefined"};
    if (kind === "number") {
      if (Number.isFinite(value)) return {__proto__: null, type: "number", value: value};
      return {__proto__: null, type: "number", unserializableValue: Number.isNaN(value) ? "NaN" : (value > 0 ? "Infinity" : "-Infinity")};
    }
    if (kind === "string" || kind === "boolean") return {__proto__: null, type: kind, value: value};
    if (kind === "symbol") return {__proto__: null, type: "symbol", description: Symbol.prototype.toString.call(value)};
    if (kind === "bigint") return {__proto__: null, type: "bigint", unserializableValue: String(value)};
    const handle = prefix + (++next);
    host.__tb_handles[handle] = value;
    const out = {__proto__: null, type: kind === "function" ? "function" : "object", objectId: handle};
    if (kind === "function") {
      out.className = "Function";
    } else if (Array.isArray(value)) {
      out.subtype = "array";
      out.className = "Array";
    } else if (typeof globalThis.Node === "function" && value instanceof globalThis.Node) {
      out.subtype = "node";
      out.className = "Node";
      out.description = value.nodeName;
    } else {
      out.className = "Object";
    }
    if (preview) {
      const properties = host.array();
      let count = 0;
      let overflow = false;
      for (const name of Object.getOwnPropertyNames(value)) {
        if (count === 5) { overflow = true; break; }
        const descriptor = Object.getOwnPropertyDescriptor(value, name);
        if (!descriptor) continue;
        if (Object.prototype.hasOwnProperty.call(descriptor, "value")) {
          const entry = {__proto__: null, name: name, type: typeof descriptor.value};
          if (descriptor.value === null) {
            entry.type = "object";
            entry.subtype = "null";
            entry.value = "null";
          } else if (entry.type !== "object" && entry.type !== "function") {
            entry.value = String(descriptor.value);
          }
          host.push(properties, entry);
        } else {
          host.push(properties, {__proto__: null, name: name, type: "accessor"});
        }
        count++;
      }
      out.preview = {__proto__: null, type: out.type, overflow: overflow, properties: properties};
      if (out.subtype) out.preview.subtype = out.subtype;
    }
    return out;
  };
  const result = host.array();
  const seenNames = {__proto__: null};
  const seenSymbols = host.array();
  for (let object = target; object !== null && object !== undefined; object = Object.getPrototypeOf(object)) {
    const isOwn = object === target;
    const pushDescriptor = (name, descriptor, symbol) => {
      const accessor = !Object.prototype.hasOwnProperty.call(descriptor, "value");
      if (accessorsOnly && !accessor) return;
      const entry = {__proto__: null,
        name: name,
        configurable: !!descriptor.configurable,
        enumerable: !!descriptor.enumerable,
      };
      if (symbol) entry.symbol = remote(symbol, false);
      if (accessor) {
        if (descriptor.get) entry.get = remote(descriptor.get, false);
        if (descriptor.set) entry.set = remote(descriptor.set, false);
      } else {
        entry.value = remote(descriptor.value, generatePreview);
        entry.writable = !!descriptor.writable;
      }
      if (!ownProperties) entry.isOwn = isOwn;
      host.push(result, entry);
    };
    for (const name of Object.getOwnPropertyNames(object)) {
      if (seenNames[name]) continue;
      seenNames[name] = true;
      const descriptor = Object.getOwnPropertyDescriptor(object, name);
      if (descriptor) pushDescriptor(name, descriptor, null);
    }
    for (const symbol of Object.getOwnPropertySymbols(object)) {
      let seen = false;
      for (let index = 0; index < seenSymbols.length; index++) {
        if (seenSymbols[index] === symbol) { seen = true; break; }
      }
      if (seen) continue;
      host.push(seenSymbols, symbol);
      const descriptor = Object.getOwnPropertyDescriptor(object, symbol);
      if (descriptor) pushDescriptor(String(symbol), descriptor, symbol);
    }
    if (ownProperties) break;
  }
  const internal = host.array();
  host.push(internal, {__proto__: null,
    name: "[[Prototype]]",
    value: remote(Object.getPrototypeOf(target), false),
  });
  return host.stringify({__proto__: null, result: result, internalProperties: internal});
})()"#;

/// Reads the captured async result and serializes it as a CDP `RemoteObject`.
pub(crate) const RUNTIME_READ: &str = r#"(() => {
  const s = host.__tb_async && host.__tb_async[__ID__];
  if (!s || !s.done) return host.stringify({__proto__: null, pending: true });
  delete host.__tb_async[__ID__];
  if (s.error !== undefined) return host.stringify({__proto__: null, error: s.error });
  const v = s.value;
  if (v === null) return host.stringify({__proto__: null, type: "object", subtype: "null", value: null });
  const t = typeof v;
  if (t === "undefined") return host.stringify({__proto__: null, type: "undefined" });
  if (t === "number") {
    if (Number.isFinite(v)) return host.stringify({__proto__: null, type: "number", value: v });
    return host.stringify({__proto__: null, type: "number", unserializableValue: Number.isNaN(v) ? "NaN" : (v > 0 ? "Infinity" : "-Infinity") });
  }
  if (t === "string" || t === "boolean") return host.stringify({__proto__: null, type: t, value: v });
  try { return host.stringify({__proto__: null, type: "object", value: v }); }
  catch (e) { return host.stringify({__proto__: null, type: "object" }); }
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

#[derive(Debug)]
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
