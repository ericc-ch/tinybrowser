//! CDP dispatch helpers: session methods, runtime script shapes, and replies.
//!
//! Split out of [`crate`] so the socket state machine and the method surface
//! have separate reasons to change.

use std::io;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use browser::{
    BrowserHandle, FrameId, RemoteValue, ScreenshotClip, ScreenshotRequest, TabError, TabEvent,
    TabHandle, TabId,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::DEFAULT_BROWSER_CONTEXT_ID;

/// Wide viewport fallback for layout metrics and default screenshots, matching
/// the renderer's virtual viewport.
pub(crate) const VIEWPORT_WIDTH: f64 = 800.0;
/// Tall viewport fallback; see [`VIEWPORT_WIDTH`].
pub(crate) const VIEWPORT_HEIGHT: f64 = 600.0;
/// Largest screenshot dimension the renderer will raster.
const MAX_SCREENSHOT_DIM: f64 = 4096.0;

pub(crate) async fn session_method(method: &str, tab: &TabHandle) -> Result<Value, DispatchError> {
    if noop_method(method) {
        return Ok(json!({}));
    }
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
        // every screenshot, so all three viewports are present.
        "Page.getLayoutMetrics" => Ok(json!({
            "layoutViewport": viewport_rect(VIEWPORT_WIDTH, VIEWPORT_HEIGHT),
            "visualViewport": {
                "offsetX": 0,
                "offsetY": 0,
                "pageX": 0,
                "pageY": 0,
                "clientWidth": VIEWPORT_WIDTH,
                "clientHeight": VIEWPORT_HEIGHT,
                "scale": 1,
            },
            "contentSize": {"x": 0, "y": 0, "width": VIEWPORT_WIDTH, "height": VIEWPORT_HEIGHT},
            "cssLayoutViewport": viewport_rect(VIEWPORT_WIDTH, VIEWPORT_HEIGHT),
            "cssVisualViewport": {
                "offsetX": 0,
                "offsetY": 0,
                "pageX": 0,
                "pageY": 0,
                "clientWidth": VIEWPORT_WIDTH,
                "clientHeight": VIEWPORT_HEIGHT,
                "scale": 1,
            },
            "cssContentSize": {"x": 0, "y": 0, "width": VIEWPORT_WIDTH, "height": VIEWPORT_HEIGHT},
        })),
        "Page.addScriptToEvaluateOnNewDocument" => Ok(json!({"identifier": "1"})),
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
        "Runtime.getProperties" => Ok(json!({"result": [], "internalProperties": []})),
        "Storage.getStorageKey" => storage_key(tab).await,
        _ => {
            if let Some(reply) = static_reply(method) {
                return Ok(reply);
            }
            if stubbed_domain(method) {
                return Ok(json!({}));
            }
            Err(DispatchError::MethodNotFound)
        }
    }
}

/// Replies with a fixed shape for methods whose client reads specific fields.
pub(crate) fn static_reply(method: &str) -> Option<Value> {
    Some(match method {
        "CSS.getMatchedStylesForNode" => json!({
            "matchedCSSRules": [], "pseudoElements": [], "inherited": [],
            "inlineStyle": null, "attributesStyle": null,
        }),
        "CSS.getInlineStylesForNode" => json!({"inlineStyle": null, "attributesStyle": null}),
        "CSS.getMediaQueries" => json!({"medias": []}),
        "CSS.getBackgroundColors" => json!({
            "backgroundColors": [], "computedFontSize": "16px", "computedFontWeight": "400",
        }),
        "CSS.takeCoverageDelta" => json!({"coverage": [], "timestamp": 0}),
        "DOMSnapshot.getSnapshot" | "DOMSnapshot.captureSnapshot" => {
            json!({"documents": [], "strings": []})
        }
        "Network.getResponseBody" => json!({"body": "", "base64Encoded": false}),
        "Debugger.getScriptSource" => json!({"scriptSource": ""}),
        "DOM.getContentQuads" => json!({"quads": []}),
        "Page.getNavigationHistory" => json!({"currentIndex": 0, "entries": []}),
        "Target.attachToBrowserTarget" => json!({"sessionId": "browser"}),
        _ => return None,
    })
}

/// Domains whose remaining methods answer an empty result until their real
/// behavior lands. The corpus then classifies those tests as protocol
/// failures or timeouts instead of `UNSUPPORTED_METHOD`, which keeps the
/// remaining work visible as behavior rather than as missing plumbing.
pub(crate) fn stubbed_domain(method: &str) -> bool {
    const DOMAINS: [&str; 44] = [
        "CSS",
        "DOM",
        "DOMDebugger",
        "DOMSnapshot",
        "DOMStorage",
        "Emulation",
        "Page",
        "Target",
        "Network",
        "Debugger",
        "Overlay",
        "BluetoothEmulation",
        "DeviceOrientation",
        "Memory",
        "WebAuthn",
        "WebMCP",
        "Storage",
        "BackgroundService",
        "IndexedDB",
        "ServiceWorker",
        "Fetch",
        "Audits",
        "Animation",
        "Profiler",
        "Preload",
        "Log",
        "Security",
        "Accessibility",
        "Tracing",
        "Input",
        "Browser",
        "Runtime",
        "DeviceAccess",
        "WebAudio",
        "Performance",
        "PerformanceTimeline",
        "LayerTree",
        "IO",
        "HeapProfiler",
        "Timeline",
        "Media",
        "EventBreakpoints",
        "SystemInfo",
        "CrashReportContext",
    ];
    DOMAINS.iter().any(|domain| {
        method
            .strip_prefix(domain)
            .is_some_and(|suffix| suffix.starts_with('.'))
    })
}

/// Methods the protocol surface accepts without a behavior change: domains the
/// engine does not implement yet. Accepting them keeps the corpus classifying
/// tests as protocol failures or timeouts instead of `UNSUPPORTED_METHOD`.
fn noop_method(method: &str) -> bool {
    matches!(
        method,
        "Runtime.disable"
            | "Target.setAutoAttach"
            | "Runtime.runIfWaitingForDebugger"
            | "Log.enable"
            | "Page.setLifecycleEventsEnabled"
            | "Network.enable"
            | "Emulation.setFocusEmulationEnabled"
            | "Emulation.setDeviceMetricsOverride"
            | "Emulation.clearDeviceMetricsOverride"
            | "Emulation.setTouchEmulationEnabled"
            | "Emulation.setEmulatedMedia"
            | "Emulation.setScriptExecutionDisabled"
            | "Emulation.setPressureSourceOverrideEnabled"
            | "Emulation.setPressureStateOverride"
            | "Runtime.addBinding"
            | "Security.setIgnoreCertificateErrors"
            | "Page.setBypassCSP"
            | "Page.startScreenRecording"
            | "DOM.enable"
            | "DOM.disable"
            | "DOMSnapshot.enable"
            | "Debugger.enable"
            | "Debugger.disable"
            | "Fetch.enable"
            | "Fetch.disable"
            | "Audits.enable"
            | "Audits.disable"
            | "Animation.enable"
            | "Animation.disable"
            | "BluetoothEmulation.enable"
            | "BluetoothEmulation.disable"
            | "IndexedDB.enable"
            | "WebMCP.enable"
            | "Accessibility.enable"
            | "ServiceWorker.enable"
            | "Tracing.start"
            | "Network.clearBrowserCookies"
            | "Network.clearBrowserCache"
            | "Network.setCacheDisabled"
            | "Network.setExtraHTTPHeaders"
            | "Network.emulateNetworkConditionsByRule"
            | "CSS.disable"
            | "Emulation.setSensorOverrideEnabled"
            | "Emulation.setDevicePostureOverride"
            | "Memory.startSampling"
            | "BackgroundService.startObserving"
            | "Storage.setStorageBucketTracking"
            | "DOMStorage.enable"
            | "Debugger.setAsyncCallStackDepth"
            | "DOMDebugger.setInstrumentationBreakpoint"
            | "Target.activateTarget"
            | "Page.stopScreenRecording"
            | "Overlay.enable"
            | "Overlay.disable"
            | "DOMDebugger.enable"
            | "Profiler.enable"
            | "Profiler.disable"
            | "Preload.enable"
            | "Target.setDiscoverTargets"
            | "Browser.grantPermissions"
            | "Browser.resetPermissions"
    )
}

/// `Storage.getStorageKey`: the tab's origin.
async fn storage_key(tab: &TabHandle) -> Result<Value, DispatchError> {
    let value = tab
        .execute_script("String(location.origin)")
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
      globalThis.__tb_dom_nodes = [];
      globalThis.__tb_dom_id = 0;
      const register = node => {
        const nodes = globalThis.__tb_dom_nodes;
        for (let id = 1; id < nodes.length; id++) { if (nodes[id] === node) return id; }
        const id = ++globalThis.__tb_dom_id;
        nodes[id] = node;
        return id;
      };
      const walk = node => {
        const id = register(node);
        const entry = { nodeId: id, backendNodeId: id, nodeType: node.nodeType, nodeName: node.nodeName };
        if (node.nodeType === 1) { entry.localName = node.localName; entry.nodeValue = ""; entry.attributes = []; }
        else if (node.nodeType === 9) {
          entry.nodeValue = ""; entry.documentURL = node.URL; entry.baseURL = node.baseURI;
          entry.compatibilityMode = "NoQuirks";
        } else { entry.nodeValue = node.nodeValue || ""; }
        const children = [];
        for (let child = node.firstChild; child; child = child.nextSibling) children.push(walk(child));
        if (children.length) entry.children = children;
        entry.childNodeCount = children.length;
        return entry;
      };
      const root = walk(document);
      return JSON.stringify(root);
    })()"#;
    let value = tab
        .execute_script(SCRIPT)
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
        "(function(){{return globalThis.__tbWebDriverActions({});}})()",
        serde_json::to_string(sources).unwrap_or_else(|_| "[]".to_owned())
    );
    tab.execute_script(&script)
        .await
        .map_err(|error| DispatchError::Failed(error.to_string()))?;
    Ok(json!({}))
}

/// Runs a DOM helper script that returns a JSON string and parses the reply.
async fn dom_eval(tab: &TabHandle, script: &str) -> Result<Value, DispatchError> {
    let value = tab
        .execute_script(script)
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
        return format!("(globalThis.__tb_handles || {{}})[{}]", json_string(id));
    }
    let id = params
        .get("backendNodeId")
        .and_then(Value::as_u64)
        .or_else(|| params.get("nodeId").and_then(Value::as_u64))
        .unwrap_or(1);
    format!("(globalThis.__tb_dom_nodes || [])[{id}]")
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
        const nodes = globalThis.__tb_dom_nodes;
        for (let id = 1; id < nodes.length; id++) { if (nodes[id] === node) return id; }
        const id = ++globalThis.__tb_dom_id;
        nodes[id] = node;
        return id;
      };
      const n = globalThis.__tb_dom_nodes[__NODE__];
      if (!n || !n.querySelector) return JSON.stringify({nodeId: 0});
      const found = n.querySelector(__SELECTOR__);
      if (!found) return JSON.stringify({nodeId: 0});
      return JSON.stringify({nodeId: register(found)});
    })()";
    const ALL: &str = r"(function(){
      const register = node => {
        const nodes = globalThis.__tb_dom_nodes;
        for (let id = 1; id < nodes.length; id++) { if (nodes[id] === node) return id; }
        const id = ++globalThis.__tb_dom_id;
        nodes[id] = node;
        return id;
      };
      const n = globalThis.__tb_dom_nodes[__NODE__];
      if (!n || !n.querySelectorAll) return JSON.stringify({nodeIds: []});
      const nodeIds = [];
      for (const found of n.querySelectorAll(__SELECTOR__)) nodeIds.push(register(found));
      return JSON.stringify({nodeIds: nodeIds});
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
      const n = globalThis.__tb_dom_nodes[__NODE__];
      if (!n) return JSON.stringify({node: null});
      const entry = {
        nodeId: __NODE__, backendNodeId: __NODE__,
        nodeType: n.nodeType, nodeName: n.nodeName,
        childNodeCount: n.childNodes ? n.childNodes.length : 0,
      };
      if (n.nodeType === 1) { entry.localName = n.localName; entry.attributes = []; }
      return JSON.stringify({node: entry});
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
      const n = globalThis.__tb_dom_nodes[__NODE__];
      if (!n) return JSON.stringify({__FIELD__: null});
      if ("__FIELD__" === "outerHTML") return JSON.stringify({outerHTML: n.outerHTML === undefined ? "" : n.outerHTML});
      const attributes = [];
      if (n.attributes) for (const a of n.attributes) { attributes.push(a.name, a.value); }
      return JSON.stringify({attributes: attributes});
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
      const n = globalThis.__tb_dom_nodes[__NODE__];
      if (!n) return JSON.stringify({object: {type: "undefined"}});
      const objectId = "dom-__NODE__";
      globalThis.__tb_handles = globalThis.__tb_handles || {};
      globalThis.__tb_handles[objectId] = n;
      return JSON.stringify({object: {
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
        const nodes = globalThis.__tb_dom_nodes;
        for (let id = 1; id < nodes.length; id++) { if (nodes[id] === node) return id; }
        const id = ++globalThis.__tb_dom_id;
        nodes[id] = node;
        return id;
      };
      const x = __X__, y = __Y__;
      const found = document.elementFromPoint ? document.elementFromPoint(x, y) : null;
      if (!found) return JSON.stringify({nodeId: 0});
      return JSON.stringify({nodeId: register(found)});
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
      globalThis.__tb_css_id = globalThis.__tb_css_id || 0;
      const out = [];
      for (const sheet of document.styleSheets) {
        const id = String(++globalThis.__tb_css_id);
        out.push({
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
      return JSON.stringify(out);
    })()"#;
    let value = dom_eval(tab, SCRIPT).await?;
    Ok(value.as_array().cloned().unwrap_or_default())
}

/// `DOM.getBoxModel`: the node's border box as content/padding/border/margin
/// quads (the engine has no separate boxes, so all four are the border box).
pub(crate) async fn dom_box_model(tab: &TabHandle, params: &Value) -> Result<Value, DispatchError> {
    const TEMPLATE: &str = r"(function(){
      const n = (__NODE__);
      if (!n || !n.getBoundingClientRect) return JSON.stringify({model: null});
      const r = n.getBoundingClientRect();
      const quad = [r.left, r.top, r.right, r.top, r.right, r.bottom, r.left, r.bottom];
      return JSON.stringify({model: {
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
      if (!n || !n.getClientRects) return JSON.stringify({quads: []});
      const quads = [];
      for (const r of n.getClientRects()) {
        quads.push([r.left, r.top, r.right, r.top, r.right, r.bottom, r.left, r.bottom]);
      }
      return JSON.stringify({quads: quads});
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
      const n = globalThis.__tb_dom_nodes[__NODE__];
      if (!n || !globalThis.getComputedStyle) return JSON.stringify({computedStyle: []});
      const style = globalThis.getComputedStyle(n);
      const computedStyle = [];
      for (let i = 0; i < style.length; i++) {
        const name = style.item(i);
        computedStyle.push({name: name, value: style.getPropertyValue(name)});
      }
      return JSON.stringify({computedStyle: computedStyle});
    })()";
    let script = TEMPLATE.replace("__NODE__", &requested_node(params).to_string());
    dom_eval(tab, &script).await
}

/// One viewport rectangle for [`Page.getLayoutMetrics`].
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
    let viewport_width = clip
        .map_or(VIEWPORT_WIDTH, |clip| f64::from(clip.x + clip.width))
        .clamp(VIEWPORT_WIDTH, MAX_SCREENSHOT_DIM);
    let viewport_height = clip
        .map_or(VIEWPORT_HEIGHT, |clip| f64::from(clip.y + clip.height))
        .clamp(VIEWPORT_HEIGHT, MAX_SCREENSHOT_DIM);
    let request = ScreenshotRequest {
        viewport_width: narrow(viewport_width),
        viewport_height: narrow(viewport_height),
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

/// Schedules `__SOURCE__` and captures its settled value. Results key by
/// evaluation ID; a single shared slot would let overlapping evaluations
/// overwrite each other.
pub(crate) const RUNTIME_SCHEDULE: &str = r#"(() => {
  globalThis.__tb_async = globalThis.__tb_async || {};
  const slot = { done: false };
  globalThis.__tb_async[__ID__] = slot;
  Promise.resolve((__SOURCE__)).then(
    (v) => { slot.done = true; slot.value = v; },
    (e) => { slot.done = true; slot.error = String((e && e.message) || e); }
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
  // A DOM node is an `ElementHandle` only when the remote object says
  // `subtype: "node"` (CDP `Runtime.RemoteObject`).
  if (t === "object" && typeof v.nodeType === "number" && typeof v.nodeName === "string")
    return JSON.stringify({ type: "object", subtype: "node", className: (v.constructor && v.constructor.name) || "", objectId: __ID__ });
  return JSON.stringify({ type: t === "function" ? "function" : "object", objectId: __ID__ });
})()"#;

/// Schedules an awaited evaluate whose result becomes a handle (or an inline
/// primitive), the `evaluateHandle` shape: `awaitPromise` with
/// `returnByValue: false`. Results key by evaluation ID; a single shared slot
/// would let overlapping evaluations overwrite each other.
pub(crate) const RUNTIME_HANDLE_SCHEDULE: &str = r#"(() => {
  globalThis.__tb_async_handles = globalThis.__tb_async_handles || {};
  const slot = { done: false };
  globalThis.__tb_async_handles[__ID__] = slot;
  Promise.resolve((__SOURCE__)).then(
    (v) => { slot.done = true; slot.value = v; },
    (e) => { slot.done = true; slot.error = String((e && e.message) || e); }
  );
  return "scheduled";
})()"#;

/// Reads the awaited handle result and serializes it as a CDP `RemoteObject`.
pub(crate) const RUNTIME_HANDLE_READ: &str = r#"(() => {
  const slot = globalThis.__tb_async_handles && globalThis.__tb_async_handles[__ID__];
  if (!slot || !slot.done) return JSON.stringify({ pending: true });
  delete globalThis.__tb_async_handles[__ID__];
  if (slot.error !== undefined) return JSON.stringify({ error: slot.error });
  globalThis.__tb_handles = globalThis.__tb_handles || {};
  const v = slot.value;
  if (v === null) return JSON.stringify({ type: "object", subtype: "null", value: null });
  const t = typeof v;
  if (t === "undefined") return JSON.stringify({ type: "undefined" });
  if (t === "number") {
    if (Number.isFinite(v)) return JSON.stringify({ type: "number", value: v });
    return JSON.stringify({ type: "number", unserializableValue: Number.isNaN(v) ? "NaN" : (v > 0 ? "Infinity" : "-Infinity") });
  }
  if (t === "string" || t === "boolean") return JSON.stringify({ type: t, value: v });
  globalThis.__tb_handles[__ID__] = v;
  // A DOM node is an `ElementHandle` only when the remote object says
  // `subtype: "node"` (CDP `Runtime.RemoteObject`).
  if (t === "object" && typeof v.nodeType === "number" && typeof v.nodeName === "string")
    return JSON.stringify({ type: "object", subtype: "node", className: (v.constructor && v.constructor.name) || "", objectId: __ID__ });
  return JSON.stringify({ type: t === "function" ? "function" : "object", objectId: __ID__ });
})()"#;

/// Reads the captured async result and serializes it as a CDP `RemoteObject`.
pub(crate) const RUNTIME_READ: &str = r#"(() => {
  const s = globalThis.__tb_async && globalThis.__tb_async[__ID__];
  if (!s || !s.done) return JSON.stringify({ pending: true });
  delete globalThis.__tb_async[__ID__];
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
