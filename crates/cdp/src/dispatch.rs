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
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::DEFAULT_BROWSER_CONTEXT_ID;

/// Largest capture or emulated dimension the renderer accepts.
const MAX_SCREENSHOT_DIM: f64 = 4096.0;

/// The target a command arrived on. A page session addresses one tab; a
/// browser session addresses browser-wide state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TargetKind {
    Page,
    Browser,
}

const PAGE: u8 = 1;
const BROWSER: u8 = 2;

/// What one protocol method does on one target kind.
#[derive(Clone, Copy)]
pub(crate) enum Handler {
    /// The requested effect is already the engine's fixed behavior.
    Accept(Accepted),
    ClearBrowserCookies,
    GetTargetInfo,
    Page(PageHandler),
    Browser(BrowserHandler),
}

/// Page-session handlers that need connection state.
#[derive(Clone, Copy)]
pub(crate) enum PageHandler {
    Enable,
    Disable,
    Navigate,
    NavigateToHistoryEntry,
    CaptureScreenshot,
    CreateIsolatedWorld,
    SetLifecycleEventsEnabled,
    RuntimeEvaluate,
    RuntimeCallFunctionOn,
    RuntimeEnable,
    RuntimeDisable,
    RuntimeReleaseObject,
    RuntimeReleaseObjectGroup,
    CssEnable,
    CssDisable,
    TracingEnd,
    Script(ScriptMethod),
}

/// Browser-session handlers.
#[derive(Clone, Copy)]
pub(crate) enum BrowserHandler {
    GetVersion,
    Close,
    SetAutoAttach,
    GetTargets,
    CreateTarget,
    CloseTarget,
    AttachToTarget,
    AttachToBrowserTarget,
    DetachFromTarget,
}

/// Page handlers that run one page-side helper and return its JSON reply.
#[derive(Clone, Copy)]
pub(crate) enum ScriptMethod {
    GetFrameTree,
    GetLayoutMetrics,
    GetResourceTree,
    Reload,
    AddScriptToEvaluateOnNewDocument,
    RemoveScriptToEvaluateOnNewDocument,
    GetNavigationHistory,
    GetProperties,
    SetDeviceMetricsOverride,
    ClearDeviceMetricsOverride,
    GetStorageKey,
    DomGetDocument,
    DomQuerySelector,
    DomQuerySelectorAll,
    DomDescribeNode,
    DomGetOuterHtml,
    DomGetAttributes,
    DomResolveNode,
    DomGetNodeForLocation,
    DomGetBoxModel,
    DomGetContentQuads,
    InputMouseEvent,
    InputKeyEvent,
    InputInsertText,
    InputTouchEvent,
    InputEmulateTouchFromMouse,
    CssGetComputedStyleForNode,
}

/// Why a command is an accepted no-op. The reason is data so the registry
/// states the justification instead of a comment beside a string list.
#[derive(Clone, Copy)]
pub(crate) enum Accepted {
    /// Nothing waits for a debugger, so there is no waiter to release.
    WaitingForDebugger,
    /// The domain emits no events, so its subscription toggles change nothing.
    DomainWithoutEvents,
    /// Focus is not engine state.
    FocusIsNotEngineState,
    /// A page target has no child targets.
    PageHasNoChildTargets,
    /// The renderer owns its font stack; there is no font configuration.
    RendererOwnsFonts,
    /// The engine has no download pipeline, so no download can start.
    NoDownloadPipeline,
    /// The engine has no HTTP cache, so clearing it is already true.
    NoHttpCache,
    /// The request selects the engine's existing media defaults.
    DefaultEmulatedMedia,
    /// The engine has no HTTP cache, so caching is already disabled.
    CacheAlreadyDisabled,
}

struct Route {
    method: &'static str,
    kinds: u8,
    handler: Handler,
}

const ROUTES: &[Route] = &[
    Route {
        method: "Browser.getVersion",
        kinds: BROWSER,
        handler: Handler::Browser(BrowserHandler::GetVersion),
    },
    Route {
        method: "Browser.close",
        kinds: BROWSER,
        handler: Handler::Browser(BrowserHandler::Close),
    },
    Route {
        method: "Browser.setDownloadBehavior",
        kinds: BROWSER,
        handler: Handler::Accept(Accepted::NoDownloadPipeline),
    },
    Route {
        method: "Target.setAutoAttach",
        kinds: BROWSER,
        handler: Handler::Browser(BrowserHandler::SetAutoAttach),
    },
    Route {
        method: "Target.setAutoAttach",
        kinds: PAGE,
        handler: Handler::Accept(Accepted::PageHasNoChildTargets),
    },
    Route {
        method: "Target.getTargetInfo",
        kinds: PAGE | BROWSER,
        handler: Handler::GetTargetInfo,
    },
    Route {
        method: "Target.getTargets",
        kinds: BROWSER,
        handler: Handler::Browser(BrowserHandler::GetTargets),
    },
    Route {
        method: "Target.createTarget",
        kinds: BROWSER,
        handler: Handler::Browser(BrowserHandler::CreateTarget),
    },
    Route {
        method: "Target.closeTarget",
        kinds: BROWSER,
        handler: Handler::Browser(BrowserHandler::CloseTarget),
    },
    Route {
        method: "Target.attachToTarget",
        kinds: BROWSER,
        handler: Handler::Browser(BrowserHandler::AttachToTarget),
    },
    Route {
        method: "Target.attachToBrowserTarget",
        kinds: BROWSER,
        handler: Handler::Browser(BrowserHandler::AttachToBrowserTarget),
    },
    Route {
        method: "Target.detachFromTarget",
        kinds: BROWSER,
        handler: Handler::Browser(BrowserHandler::DetachFromTarget),
    },
    Route {
        method: "Network.clearBrowserCookies",
        kinds: PAGE | BROWSER,
        handler: Handler::ClearBrowserCookies,
    },
    Route {
        method: "Page.enable",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::Enable),
    },
    Route {
        method: "Page.disable",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::Disable),
    },
    Route {
        method: "Page.navigate",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::Navigate),
    },
    Route {
        method: "Page.navigateToHistoryEntry",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::NavigateToHistoryEntry),
    },
    Route {
        method: "Page.captureScreenshot",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::CaptureScreenshot),
    },
    Route {
        method: "Page.createIsolatedWorld",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::CreateIsolatedWorld),
    },
    Route {
        method: "Page.setLifecycleEventsEnabled",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::SetLifecycleEventsEnabled),
    },
    Route {
        method: "Page.getFrameTree",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::Script(ScriptMethod::GetFrameTree)),
    },
    Route {
        method: "Page.getLayoutMetrics",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::Script(ScriptMethod::GetLayoutMetrics)),
    },
    Route {
        method: "Page.getResourceTree",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::Script(ScriptMethod::GetResourceTree)),
    },
    Route {
        method: "Page.reload",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::Script(ScriptMethod::Reload)),
    },
    Route {
        method: "Page.addScriptToEvaluateOnNewDocument",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::Script(
            ScriptMethod::AddScriptToEvaluateOnNewDocument,
        )),
    },
    Route {
        method: "Page.removeScriptToEvaluateOnNewDocument",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::Script(
            ScriptMethod::RemoveScriptToEvaluateOnNewDocument,
        )),
    },
    Route {
        method: "Page.getNavigationHistory",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::Script(ScriptMethod::GetNavigationHistory)),
    },
    Route {
        method: "Page.setFontFamilies",
        kinds: PAGE,
        handler: Handler::Accept(Accepted::RendererOwnsFonts),
    },
    Route {
        method: "Runtime.enable",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::RuntimeEnable),
    },
    Route {
        method: "Runtime.disable",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::RuntimeDisable),
    },
    Route {
        method: "Runtime.evaluate",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::RuntimeEvaluate),
    },
    Route {
        method: "Runtime.callFunctionOn",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::RuntimeCallFunctionOn),
    },
    Route {
        method: "Runtime.getProperties",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::Script(ScriptMethod::GetProperties)),
    },
    Route {
        method: "Runtime.releaseObject",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::RuntimeReleaseObject),
    },
    Route {
        method: "Runtime.releaseObjectGroup",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::RuntimeReleaseObjectGroup),
    },
    Route {
        method: "Runtime.runIfWaitingForDebugger",
        kinds: PAGE,
        handler: Handler::Accept(Accepted::WaitingForDebugger),
    },
    Route {
        method: "DOM.getDocument",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::Script(ScriptMethod::DomGetDocument)),
    },
    Route {
        method: "DOM.querySelector",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::Script(ScriptMethod::DomQuerySelector)),
    },
    Route {
        method: "DOM.querySelectorAll",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::Script(ScriptMethod::DomQuerySelectorAll)),
    },
    Route {
        method: "DOM.describeNode",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::Script(ScriptMethod::DomDescribeNode)),
    },
    Route {
        method: "DOM.getOuterHTML",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::Script(ScriptMethod::DomGetOuterHtml)),
    },
    Route {
        method: "DOM.getAttributes",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::Script(ScriptMethod::DomGetAttributes)),
    },
    Route {
        method: "DOM.resolveNode",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::Script(ScriptMethod::DomResolveNode)),
    },
    Route {
        method: "DOM.getNodeForLocation",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::Script(ScriptMethod::DomGetNodeForLocation)),
    },
    Route {
        method: "DOM.getBoxModel",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::Script(ScriptMethod::DomGetBoxModel)),
    },
    Route {
        method: "DOM.getContentQuads",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::Script(ScriptMethod::DomGetContentQuads)),
    },
    Route {
        method: "Input.dispatchMouseEvent",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::Script(ScriptMethod::InputMouseEvent)),
    },
    Route {
        method: "Input.dispatchKeyEvent",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::Script(ScriptMethod::InputKeyEvent)),
    },
    Route {
        method: "Input.insertText",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::Script(ScriptMethod::InputInsertText)),
    },
    Route {
        method: "Input.dispatchTouchEvent",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::Script(ScriptMethod::InputTouchEvent)),
    },
    Route {
        method: "Input.emulateTouchFromMouseEvent",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::Script(
            ScriptMethod::InputEmulateTouchFromMouse,
        )),
    },
    Route {
        method: "CSS.enable",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::CssEnable),
    },
    Route {
        method: "CSS.disable",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::CssDisable),
    },
    Route {
        method: "CSS.getComputedStyleForNode",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::Script(
            ScriptMethod::CssGetComputedStyleForNode,
        )),
    },
    Route {
        method: "Tracing.end",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::TracingEnd),
    },
    Route {
        method: "Storage.getStorageKey",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::Script(ScriptMethod::GetStorageKey)),
    },
    Route {
        method: "Log.enable",
        kinds: PAGE | BROWSER,
        handler: Handler::Accept(Accepted::DomainWithoutEvents),
    },
    Route {
        method: "Network.enable",
        kinds: PAGE | BROWSER,
        handler: Handler::Accept(Accepted::DomainWithoutEvents),
    },
    Route {
        method: "Network.clearBrowserCache",
        kinds: PAGE | BROWSER,
        handler: Handler::Accept(Accepted::NoHttpCache),
    },
    Route {
        method: "Network.setCacheDisabled",
        kinds: PAGE | BROWSER,
        handler: Handler::Accept(Accepted::CacheAlreadyDisabled),
    },
    Route {
        method: "Emulation.setFocusEmulationEnabled",
        kinds: PAGE | BROWSER,
        handler: Handler::Accept(Accepted::FocusIsNotEngineState),
    },
    Route {
        method: "Emulation.setEmulatedMedia",
        kinds: PAGE,
        handler: Handler::Accept(Accepted::DefaultEmulatedMedia),
    },
    Route {
        method: "Emulation.setDeviceMetricsOverride",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::Script(ScriptMethod::SetDeviceMetricsOverride)),
    },
    Route {
        method: "Emulation.clearDeviceMetricsOverride",
        kinds: PAGE,
        handler: Handler::Page(PageHandler::Script(
            ScriptMethod::ClearDeviceMetricsOverride,
        )),
    },
    Route {
        method: "Overlay.enable",
        kinds: PAGE | BROWSER,
        handler: Handler::Accept(Accepted::DomainWithoutEvents),
    },
    Route {
        method: "Overlay.disable",
        kinds: PAGE | BROWSER,
        handler: Handler::Accept(Accepted::DomainWithoutEvents),
    },
];

/// Looks up the handler for `method` on `kind`. An unregistered pair is
/// `MethodNotFound`; there is no fallback table.
pub(crate) fn route(method: &str, kind: TargetKind) -> Option<Handler> {
    let flag = match kind {
        TargetKind::Page => PAGE,
        TargetKind::Browser => BROWSER,
    };
    ROUTES
        .iter()
        .find(|route| route.method == method && route.kinds & flag != 0)
        .map(|route| route.handler)
}

/// Answers an accepted no-op. Conditional entries prove their premise from the
/// params and otherwise report `MethodNotFound`, exactly like a real handler
/// that cannot honor the request.
pub(crate) fn accept(reason: Accepted, params: &Value) -> Result<Value, DispatchError> {
    if premise_holds(reason, params) {
        Ok(json!({}))
    } else {
        Err(DispatchError::MethodNotFound)
    }
}

/// Whether an accepted no-op applies. Conditional variants prove their premise
/// here, once; the rest hold unconditionally.
fn premise_holds(reason: Accepted, params: &Value) -> bool {
    match reason {
        Accepted::DefaultEmulatedMedia => emulated_media_is_default(params),
        Accepted::CacheAlreadyDisabled => {
            params.get("cacheDisabled").and_then(Value::as_bool) == Some(true)
        }
        Accepted::WaitingForDebugger
        | Accepted::DomainWithoutEvents
        | Accepted::FocusIsNotEngineState
        | Accepted::PageHasNoChildTargets
        | Accepted::RendererOwnsFonts
        | Accepted::NoDownloadPipeline
        | Accepted::NoHttpCache => true,
    }
}

/// Runs one stateless page handler and returns its protocol reply.
pub(crate) async fn run_script(
    method: ScriptMethod,
    params: &Value,
    tab: &TabHandle,
) -> Result<Value, DispatchError> {
    match method {
        ScriptMethod::GetFrameTree | ScriptMethod::GetResourceTree => frame_tree(tab).await,
        ScriptMethod::GetLayoutMetrics => layout_metrics(tab).await,
        ScriptMethod::Reload => {
            let url = tab
                .document_url()
                .await
                .map_err(|error| DispatchError::Failed(error.to_string()))?;
            open_url(tab, &url).await?;
            Ok(json!({}))
        }
        ScriptMethod::AddScriptToEvaluateOnNewDocument => add_init_script(params, tab).await,
        ScriptMethod::RemoveScriptToEvaluateOnNewDocument => remove_init_script(params, tab).await,
        ScriptMethod::GetNavigationHistory => navigation_history(tab).await,
        ScriptMethod::GetProperties => runtime_get_properties(tab, params).await,
        ScriptMethod::SetDeviceMetricsOverride => set_device_metrics(params, tab).await,
        ScriptMethod::ClearDeviceMetricsOverride => clear_device_metrics(tab).await,
        ScriptMethod::GetStorageKey => storage_key(tab).await,
        ScriptMethod::DomGetDocument => dom_get_document(tab).await,
        ScriptMethod::DomQuerySelector => dom_query_selector(tab, params).await,
        ScriptMethod::DomQuerySelectorAll => dom_query_selector_all(tab, params).await,
        ScriptMethod::DomDescribeNode => dom_describe_node(tab, params).await,
        ScriptMethod::DomGetOuterHtml => dom_node_string(tab, params, "outerHTML").await,
        ScriptMethod::DomGetAttributes => dom_node_string(tab, params, "attributes").await,
        ScriptMethod::DomResolveNode => dom_resolve_node(tab, params).await,
        ScriptMethod::DomGetNodeForLocation => dom_node_for_location(tab, params).await,
        ScriptMethod::DomGetBoxModel => dom_box_model(tab, params).await,
        ScriptMethod::DomGetContentQuads => dom_content_quads(tab, params).await,
        ScriptMethod::InputMouseEvent => input_mouse_event(tab, params).await,
        ScriptMethod::InputKeyEvent => input_key_event(tab, params).await,
        ScriptMethod::InputInsertText => input_insert_text(tab, params).await,
        ScriptMethod::InputTouchEvent => input_touch_event(tab, params).await,
        ScriptMethod::InputEmulateTouchFromMouse => {
            input_emulate_touch_from_mouse(tab, params).await
        }
        ScriptMethod::CssGetComputedStyleForNode => css_computed_style(tab, params).await,
    }
}

async fn frame_tree(tab: &TabHandle) -> Result<Value, DispatchError> {
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

async fn layout_metrics(tab: &TabHandle) -> Result<Value, DispatchError> {
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

/// `Page.getNavigationHistory`: the tab's committed session-history entries in
/// traversal order and the active index.
/// <https://chromedevtools.github.io/devtools-protocol/tot/Page/#method-getNavigationHistory>
async fn navigation_history(tab: &TabHandle) -> Result<Value, DispatchError> {
    let history = tab
        .history()
        .await
        .map_err(|error| DispatchError::Failed(error.to_string()))?;
    let entries: Vec<Value> = history
        .entries()
        .iter()
        .map(|entry| {
            json!({
                "id": entry.id().get(),
                "url": entry.url().as_str(),
                "userTypedURL": entry.url().as_str(),
                "title": entry.title(),
                "transitionType": "other",
            })
        })
        .collect();
    Ok(json!({"currentIndex": history.current_index(), "entries": entries}))
}

/// Which properties a `Runtime.getProperties` request wants.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
struct PropertyFilter {
    own_properties: bool,
    non_indexed_properties_only: bool,
}

/// How a `Runtime.getProperties` request wants them rendered.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
struct PropertyRender {
    accessor_properties_only: bool,
    generate_preview: bool,
}

/// `Runtime.getProperties` params, parsed once at the protocol boundary.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct GetPropertiesParams {
    pub(crate) object_id: String,
    pub(crate) object_group: String,
    #[serde(flatten)]
    filter: PropertyFilter,
    #[serde(flatten)]
    render: PropertyRender,
}

fn get_properties_script(params: &GetPropertiesParams) -> String {
    let arguments = serde_json::to_string(params).unwrap_or_else(|_| "{}".to_owned());
    RUNTIME_GET_PROPERTIES.replace("__ARGS__", &arguments)
}

/// `Runtime.getProperties`: own and inherited properties of a page-side handle,
/// plus its `[[Prototype]]` internal property. Accessors are reported as
/// `get`/`set` pairs; getters never run. `privateProperties` is absent because
/// JavaScript has no reflection API for private names.
/// <https://chromedevtools.github.io/devtools-protocol/tot/Runtime/#method-getProperties>
async fn runtime_get_properties(tab: &TabHandle, params: &Value) -> Result<Value, DispatchError> {
    let params: GetPropertiesParams = serde_json::from_value(params.clone())
        .map_err(|error| DispatchError::Failed(error.to_string()))?;
    if params.object_id.is_empty() {
        return Err(DispatchError::Failed(
            "Runtime.getProperties requires objectId".into(),
        ));
    }
    let reply = execute_json(tab, &get_properties_script(&params)).await?;
    match reply.get("ok").and_then(Value::as_bool) {
        Some(true) => Ok(reply.get("value").cloned().unwrap_or(Value::Null)),
        Some(false) => Err(DispatchError::Failed(
            reply
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("Runtime.getProperties failed")
                .to_owned(),
        )),
        None => Err(DispatchError::Failed(
            "Runtime.getProperties returned a malformed reply".into(),
        )),
    }
}

/// `Runtime.releaseObject`: forget one page-side handle.
pub(crate) async fn runtime_release_object(
    tab: &TabHandle,
    params: &Value,
) -> Result<Value, DispatchError> {
    let object_id = params
        .get("objectId")
        .and_then(Value::as_str)
        .ok_or_else(|| DispatchError::Failed("Runtime.releaseObject requires objectId".into()))?;
    let script = RUNTIME_RELEASE.replace("__ARGS__", &json!({"objectId": object_id}).to_string());
    tab.execute_browser_script(&script)
        .await
        .map_err(|error| DispatchError::Failed(error.to_string()))?;
    Ok(json!({}))
}

/// `Runtime.releaseObjectGroup`: forget every handle in one object group.
pub(crate) async fn runtime_release_object_group(
    tab: &TabHandle,
    params: &Value,
) -> Result<Value, DispatchError> {
    let object_group = params
        .get("objectGroup")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            DispatchError::Failed("Runtime.releaseObjectGroup requires objectGroup".into())
        })?;
    let script = RUNTIME_RELEASE.replace(
        "__ARGS__",
        &json!({"objectGroup": object_group}).to_string(),
    );
    tab.execute_browser_script(&script)
        .await
        .map_err(|error| DispatchError::Failed(error.to_string()))?;
    Ok(json!({}))
}

/// Runs a browser helper that returns a JSON string and parses it.
async fn execute_json(tab: &TabHandle, script: &str) -> Result<Value, DispatchError> {
    let value = tab
        .execute_browser_script(script)
        .await
        .map_err(|error| DispatchError::Failed(error.to_string()))?;
    let RemoteValue::String(text) = value else {
        return Err(DispatchError::BadHelperReply("browser"));
    };
    serde_json::from_str(&text).map_err(|error| DispatchError::Failed(error.to_string()))
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
    use super::{BROWSER, PAGE, ROUTES, capture_side, required_metric};
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
    fn every_registered_pair_has_one_handler() {
        for (index, route) in ROUTES.iter().enumerate() {
            assert!(route.kinds & (PAGE | BROWSER) != 0);
            for other in &ROUTES[index + 1..] {
                assert!(
                    route.method != other.method || route.kinds & other.kinds == 0,
                    "{} has overlapping handlers for the same target kind",
                    route.method
                );
            }
        }
    }
}

/// `Storage.getStorageKey`: the tab's origin.
async fn storage_key(tab: &TabHandle) -> Result<Value, DispatchError> {
    let value = tab
        .execute_browser_script("String(location.origin)")
        .await
        .map_err(|error| DispatchError::Failed(error.to_string()))?;
    let RemoteValue::String(key) = value else {
        return Err(DispatchError::BadHelperReply("Storage.getStorageKey"));
    };
    Ok(json!({"storageKey": key}))
}

/// `DOM.getDocument`: serialize the live tree, remembering each node so later
/// DOM calls can look it up by `nodeId`.
pub(crate) async fn dom_get_document(tab: &TabHandle) -> Result<Value, DispatchError> {
    const SCRIPT: &str = r#"(function(){
      host.__tbDomRegister = undefined;
      host.__tb_dom_nodes = host.array();
      host.__tb_dom_by_node = undefined;
      const register = __REGISTER__;
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
        .execute_browser_script(SCRIPT.replace("__REGISTER__", REGISTER_PRELUDE).as_str())
        .await
        .map_err(|error| DispatchError::Failed(error.to_string()))?;
    let RemoteValue::String(text) = value else {
        return Err(DispatchError::BadHelperReply("DOM.getDocument"));
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
        return Err(DispatchError::BadHelperReply("DOM"));
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

/// The page-side node registrar shared by every `DOM.*` method that answers
/// with node ids. It installs itself once per realm; the `Map` is the
/// node-to-id reverse lookup, so a `querySelectorAll` over N nodes registers in
/// O(N) instead of scanning the table per match. `DOM.getDocument` drops it to
/// restart the ids.
const REGISTER_PRELUDE: &str = r"(function(){
  if (host.__tbDomRegister === undefined) {
    const nodes = host.array();
    const byNode = new Map();
    let next = 0;
    host.__tb_dom_nodes = nodes;
    host.__tb_dom_by_node = byNode;
    host.__tb_dom_id = next;
    host.__tbDomRegister = node => {
      const known = byNode.get(node);
      if (known !== undefined) return known;
      const id = ++next;
      host.__tb_dom_id = id;
      nodes[id] = node;
      byNode.set(node, id);
      return id;
    };
  }
  return host.__tbDomRegister;
})()";

/// The `selector` parameter a DOM query method requires.
fn requested_selector(params: &Value) -> Result<String, DispatchError> {
    let Some(selector) = params.get("selector").and_then(Value::as_str) else {
        return Err(DispatchError::InvalidParams("selector"));
    };
    Ok(json_string(selector))
}

/// `DOM.querySelector`: the first matching node's id.
pub(crate) async fn dom_query_selector(
    tab: &TabHandle,
    params: &Value,
) -> Result<Value, DispatchError> {
    const SCRIPT: &str = r"(function(){
      const register = __REGISTER__;
      const n = host.__tb_dom_nodes[__NODE__];
      if (!n || !n.querySelector) return host.stringify({__proto__: null, nodeId: 0});
      const found = n.querySelector(__SELECTOR__);
      if (!found) return host.stringify({__proto__: null, nodeId: 0});
      return host.stringify({__proto__: null, nodeId: register(found)});
    })()";
    let selector = requested_selector(params)?;
    let script = SCRIPT
        .replace("__REGISTER__", REGISTER_PRELUDE)
        .replace("__NODE__", &requested_node(params).to_string())
        .replace("__SELECTOR__", &selector);
    dom_eval(tab, &script).await
}

/// `DOM.querySelectorAll`: every matching node's id.
pub(crate) async fn dom_query_selector_all(
    tab: &TabHandle,
    params: &Value,
) -> Result<Value, DispatchError> {
    const SCRIPT: &str = r"(function(){
      const register = __REGISTER__;
      const n = host.__tb_dom_nodes[__NODE__];
      if (!n || !n.querySelectorAll) return host.stringify({__proto__: null, nodeIds: []});
      const nodeIds = host.array();
      for (const found of n.querySelectorAll(__SELECTOR__)) host.push(nodeIds, register(found));
      return host.stringify({__proto__: null, nodeIds: nodeIds});
    })()";
    let selector = requested_selector(params)?;
    let script = SCRIPT
        .replace("__REGISTER__", REGISTER_PRELUDE)
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
      const register = __REGISTER__;
      const x = __X__, y = __Y__;
      const found = document.elementFromPoint ? document.elementFromPoint(x, y) : null;
      if (!found) return host.stringify({__proto__: null, nodeId: 0});
      return host.stringify({__proto__: null, nodeId: register(found)});
    })()";
    let script = TEMPLATE
        .replace("__REGISTER__", REGISTER_PRELUDE)
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

/// Evaluates `__SOURCE__` and serializes the value with the bridge's one
/// `RemoteObject` serializer. Handles join `__GROUP__`.
pub(crate) const RUNTIME_HANDLE: &str = r"(() => {
  const v = (__SOURCE__);
  return host.stringify(host.remoteObject(v, { __proto__: null, objectGroup: __GROUP__ }));
})()";

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

/// Reads the awaited handle result and serializes it with the bridge's one
/// `RemoteObject` serializer.
pub(crate) const RUNTIME_HANDLE_READ: &str = r"(() => {
  const slot = host.__tb_async_handles && host.__tb_async_handles[__ID__];
  if (!slot || !slot.done) return host.stringify({ __proto__: null, pending: true });
  delete host.__tb_async_handles[__ID__];
  if (slot.error !== undefined) return host.stringify({ __proto__: null, error: slot.error });
  return host.stringify({ __proto__: null, value: host.remoteObject(slot.value, { __proto__: null, objectGroup: __GROUP__ }) });
})()";

/// Releases one handle or every handle in one object group.
pub(crate) const RUNTIME_RELEASE: &str = r#"(() => {
  const args = __ARGS__;
  if (args.objectGroup !== undefined) host.__tb_release_group(args.objectGroup);
  else host.__tb_release_handle(args.objectId);
  return "released";
})()"#;

/// Drops every handle owned by the outgoing execution context.
pub(crate) const RUNTIME_CLEAR_HANDLES: &str = "host.__tb_clear_handles()";

/// Enumerates a page-side handle's own and inherited properties as CDP
/// `PropertyDescriptor` JSON. Accessor descriptors expose `get`/`set`
/// `RemoteObject`s instead of invoking them. Values that are not primitives
/// become handles in the request's object group.
pub(crate) const RUNTIME_GET_PROPERTIES: &str = r#"(() => {
  const args = __ARGS__;
  const objectId = args.objectId;
  if (!(objectId in host.__tb_handles))
    return host.stringify({ __proto__: null, ok: false, error: "Could not find object with given id" });
  const target = host.__tb_handles[objectId];
  const ownProperties = args.ownProperties;
  const accessorsOnly = args.accessorPropertiesOnly;
  const generatePreview = args.generatePreview;
  const remote = (value, preview) => host.remoteObject(value, {
    __proto__: null,
    objectGroup: args.objectGroup,
    generatePreview: preview,
  });
  const result = [];
  const pushDescriptor = (name, descriptor, symbol, isOwn) => {
    const accessor = !('value' in descriptor);
    if (accessorsOnly && !accessor) return;
    const entry = { __proto__: null,
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
    result[result.length] = entry;
  };
  const seenNames = { __proto__: null };
  const seenSymbols = new host.__tb_set();
  for (let object = target; object !== null && object !== undefined;
       object = host.__tb_getPrototypeOf(object)) {
    const isOwn = object === target;
    const names = host.__tb_getOwnPropertyNames(object);
    for (let index = 0; index < names.length; index++) {
      const name = names[index];
      if (seenNames[name]) continue;
      seenNames[name] = true;
      const descriptor = host.__tb_getOwnPropertyDescriptor(object, name);
      if (descriptor) pushDescriptor(name, descriptor, null, isOwn);
    }
    const symbols = host.__tb_getOwnPropertySymbols(object);
    for (let index = 0; index < symbols.length; index++) {
      const symbol = symbols[index];
      if (host.apply(host.__tb_set_has, seenSymbols, [symbol])) continue;
      host.apply(host.__tb_set_add, seenSymbols, [symbol]);
      const descriptor = host.__tb_getOwnPropertyDescriptor(object, symbol);
      if (descriptor) {
        const name = host.apply(host.__tb_symbolToString, symbol, []);
        pushDescriptor(name, descriptor, symbol, isOwn);
      }
    }
    if (ownProperties) break;
  }
  const internal = [];
  if (!accessorsOnly) {
    internal[internal.length] = { __proto__: null,
      name: "[[Prototype]]",
      value: remote(host.__tb_getPrototypeOf(target), false),
    };
  }
  return host.stringify({ __proto__: null, ok: true, value: {
    __proto__: null,
    result: result,
    internalProperties: internal,
  }});
})()"#;

/// Reads the captured async result and serializes it with the bridge's one
/// `RemoteObject` serializer, asking for a by-value reply.
pub(crate) const RUNTIME_READ: &str = r#"(() => {
  const s = host.__tb_async && host.__tb_async[__ID__];
  if (!s || !s.done) return host.stringify({__proto__: null, pending: true });
  delete host.__tb_async[__ID__];
  if (s.error !== undefined) return host.stringify({__proto__: null, error: s.error });
  const remote = host.remoteObject(s.value, { __proto__: null, returnByValue: true });
  try { return host.stringify(remote); }
  catch (e) { return host.stringify({__proto__: null, type: "object"}); }
})()"#;

pub(crate) async fn target_info(browser: &BrowserHandle, id: TabId) -> Value {
    let (url, title) = match browser.tab(id).await {
        Ok(tab) => {
            let url = tab
                .document_url()
                .await
                .unwrap_or_else(|_| "about:blank".into());
            let title = tab.document_title().await.unwrap_or_default();
            (url, title)
        }
        Err(_) => ("about:blank".into(), String::new()),
    };
    json!({
        "targetId": id.to_string(),
        "type": "page",
        "title": title,
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
    /// A required parameter is missing or malformed.
    InvalidParams(&'static str),
    /// A browser or page helper answered in a shape the dispatch cannot read.
    BadHelperReply(&'static str),
    Failed(String),
}

impl DispatchError {
    /// The JSON-RPC error code for the reply.
    pub(crate) fn code(&self) -> i32 {
        match self {
            Self::MethodNotFound => -32601,
            Self::InvalidParams(_) => -32602,
            Self::BadHelperReply(_) | Self::Failed(_) => -32000,
        }
    }

    /// The JSON-RPC error message for the reply.
    pub(crate) fn message(&self, method: &str) -> String {
        match self {
            Self::MethodNotFound => format!("'{method}' wasn't found"),
            Self::InvalidParams(name) => format!("Invalid params: {name}"),
            Self::BadHelperReply(helper) => {
                format!("the {helper} helper did not return a string result")
            }
            Self::Failed(message) => message.clone(),
        }
    }
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
