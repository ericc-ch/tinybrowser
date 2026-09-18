//! `QuickJS` host for one document: eval, timers, `fetch`, DOM host objects.
//!
//! Callbacks live in JS (`__tb_timeouts`, `__tb_fetchCbs`). Rust holds
//! integer ids so a `Function` never crosses the JS boundary. Invocation
//! uses `Function::call` on the renderer thread.

mod bindings;
mod events;
mod intl;
mod world;

pub(crate) use world::{DocumentStreamCommand, FrameNavigation, RealmRegistry};

use std::cell::{Cell, OnceCell, RefCell};
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rquickjs::{
    Array, Coerced, Context, Ctx, FromJs, Function, Object, Runtime, Value, context::EvalOptions,
    prelude::Func,
};

pub(crate) use world::World;

use crate::document::Stop;

const MAX_RUNTIME_MEMORY: usize = 32 * 1024 * 1024;
const MAX_RUNTIME_STACK: usize = 512 * 1024;
const DEFAULT_SCRIPT_BUDGET: Duration = Duration::from_secs(5);

const INSTALL_WEB_APIS_JS: &str = include_str!("scripts/web_apis.js");

/// A value produced by script evaluation.
#[derive(Clone, Debug, PartialEq)]
pub enum ScriptValue {
    /// JS `undefined`.
    Undefined,
    /// JS `null`.
    Null,
    /// JS boolean.
    Bool(bool),
    /// JS number.
    Number(f64),
    /// JS string.
    String(String),
    /// A DOM node handle for `WebDriver` element encoding.
    Node(dom::NodeId),
    /// A JS array, for `WebDriver` JSON.
    List(Vec<ScriptValue>),
    /// A JS object, for `WebDriver` JSON.
    Map(Vec<(String, ScriptValue)>),
}

/// A classic `<script>` in document order.
///
/// [HTML prepare the script element](https://html.spec.whatwg.org/multipage/webappapis.html#prepare-the-script-element)
#[derive(Clone)]
pub(crate) enum ClassicScript {
    Inline(String),
    Src(String),
}

#[derive(Debug)]
pub(crate) enum JsError {
    Engine(Box<str>),
    Interrupted,
    BadTimerId,
}

impl JsError {
    fn engine(err: impl fmt::Display) -> Self {
        Self::Engine(err.to_string().into_boxed_str())
    }
}

impl fmt::Display for JsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Engine(message) => f.write_str(message),
            Self::Interrupted => f.write_str("interrupted"),
            Self::BadTimerId => f.write_str("bad timer id"),
        }
    }
}

impl std::error::Error for JsError {}

impl From<rquickjs::Error> for JsError {
    fn from(err: rquickjs::Error) -> Self {
        Self::Engine(err.to_string().into_boxed_str())
    }
}

pub(crate) struct PendingTimeout {
    pub delay: Duration,
    pub js_id: i32,
}

pub(crate) struct PendingJsFetch {
    pub url: String,
    pub js_id: i32,
}

/// One renderer process's `QuickJS` heap, created on first use and shared by
/// every frame in the process.
///
/// Several `QuickJS` contexts may share one runtime and its objects, "similar to
/// frames of the same origin sharing JavaScript objects in a web browser"
/// (<https://bellard.org/quickjs/quickjs.html>, JSRuntime): the same-site-frame
/// model.
/// The creation result is cached so a heap that cannot start fails every realm
/// the same way instead of retrying.
#[derive(Clone, Default)]
pub(crate) struct SharedJsRuntime(Rc<OnceCell<Result<Runtime, Box<str>>>>);

impl SharedJsRuntime {
    pub(crate) fn get(&self) -> Result<&Runtime, JsError> {
        let slot = self
            .0
            .get_or_init(|| Runtime::new().map_err(|err| err.to_string().into_boxed_str()));
        slot.as_ref()
            .map_err(|message| JsError::Engine(message.clone()))
    }
}

pub(crate) struct JsRealm {
    runtime: Runtime,
    context: Context,
    world: Rc<RefCell<World>>,
    stop: Arc<Stop>,
    pending_timeouts: Rc<RefCell<Vec<PendingTimeout>>>,
    pending_fetches: Rc<RefCell<Vec<PendingJsFetch>>>,
}

impl JsRealm {
    pub(crate) fn new(
        shared: &SharedJsRuntime,
        world: Rc<RefCell<World>>,
        stop: Arc<Stop>,
    ) -> Result<Self, JsError> {
        let runtime = shared.get()?.clone();
        runtime.set_memory_limit(MAX_RUNTIME_MEMORY);
        runtime.set_max_stack_size(MAX_RUNTIME_STACK);
        let context = Context::full(&runtime)?;
        let host = Self {
            runtime,
            context,
            world,
            stop,
            pending_timeouts: Rc::new(RefCell::new(Vec::new())),
            pending_fetches: Rc::new(RefCell::new(Vec::new())),
        };
        host.install()?;
        Ok(host)
    }

    pub(crate) fn eval(&self, source: &str) -> Result<String, JsError> {
        self.with_budget(None, || {
            self.context.with(|ctx| {
                let value: Value = eval_classic(&ctx, source)?;
                render_eval_result(&ctx, value)
            })
        })
    }

    pub(crate) fn eval_value_deadline(
        &self,
        source: &str,
        deadline: Option<Instant>,
    ) -> Result<crate::js::ScriptValue, JsError> {
        self.with_budget(deadline, || {
            self.context.with(|ctx| {
                let value: Value = eval_classic(&ctx, source)?;
                decode_value(&ctx, value)
            })
        })
    }

    pub(crate) fn take_pending_timeouts(&self) -> Vec<PendingTimeout> {
        std::mem::take(&mut *self.pending_timeouts.borrow_mut())
    }

    pub(crate) fn take_pending_fetches(&self) -> Vec<PendingJsFetch> {
        std::mem::take(&mut *self.pending_fetches.borrow_mut())
    }

    pub(crate) fn take_pending_cancels(&self) -> Vec<i32> {
        std::mem::take(&mut self.world.borrow_mut().pending_cancels)
    }

    pub(crate) fn fire_timer(&self, js_id: i32) -> Result<(), JsError> {
        self.with_budget(None, || {
            self.context.with(|ctx| {
                let timeouts: Array = ctx.globals().get("__tb_timeouts")?;
                let idx = usize::try_from(js_id).map_err(|_| JsError::BadTimerId)?;
                let func: Function = timeouts.get(idx)?;
                timeouts.as_object().remove(js_id)?;
                func.call::<_, ()>(())?;
                Ok(())
            })
        })
    }

    pub(crate) fn finish_js_fetch(
        &self,
        js_id: i32,
        ok: bool,
        status: i32,
        body: &str,
    ) -> Result<(), JsError> {
        self.with_budget(None, || {
            let body = body.to_owned();
            self.context.with(|ctx| {
                let cbs: Object = ctx.globals().get("__tb_fetchCbs")?;
                let func: Function = cbs.get(js_id)?;
                func.call::<_, ()>((ok, status, body))?;
                Ok(())
            })
        })
    }

    pub(crate) fn fire_load(&self) -> Result<(), JsError> {
        self.with_budget(None, || {
            self.context.with(|ctx| {
                bindings::fire_window_load(&ctx)?;
                Ok(())
            })
        })
    }

    /// Fires `DOMContentLoaded` at the document
    /// (<https://html.spec.whatwg.org/multipage/parsing.html#the-end>).
    pub(crate) fn fire_dom_content_loaded(&self) -> Result<(), JsError> {
        self.with_budget(None, || {
            self.context.with(|ctx| {
                bindings::fire_dom_content_loaded(&ctx)?;
                Ok(())
            })
        })
    }

    /// Fires `readystatechange` after a document readiness change.
    pub(crate) fn fire_ready_state_change(&self) -> Result<(), JsError> {
        self.with_budget(None, || {
            self.context.with(|ctx| {
                bindings::fire_ready_state_change(&ctx)?;
                Ok(())
            })
        })
    }

    pub(crate) fn fire_node_load(&self, id: dom::NodeId) -> Result<(), JsError> {
        self.with_budget(None, || {
            self.context.with(|ctx| {
                bindings::fire_node_load(&ctx, id)?;
                Ok(())
            })
        })
    }

    /// Decodes and dispatches one posted window message in this realm.
    ///
    /// Returns `false` when the payload cannot be decoded, which the engine
    /// turns into a `messageerror`
    /// (<https://html.spec.whatwg.org/multipage/web-messaging.html#window-post-message-steps>).
    pub(crate) fn deliver_window_message(
        &self,
        source: u64,
        origin: &str,
        payload: &str,
        ports: &[u64],
    ) -> Result<bool, JsError> {
        self.with_budget(None, || {
            let payload = payload.to_owned();
            let origin = origin.to_owned();
            let ports: Vec<f64> = ports.iter().map(|port| js_number(*port)).collect();
            let source = js_number(source);
            self.context.with(|ctx| {
                let deliver: Function = ctx.globals().get("__tbDeliverMessage")?;
                Ok(deliver.call((payload, source, origin, ports))?)
            })
        })
    }

    /// Dispatches `messageerror` for a payload this realm could not decode.
    pub(crate) fn deliver_window_message_error(
        &self,
        source: u64,
        origin: &str,
    ) -> Result<(), JsError> {
        self.with_budget(None, || {
            let origin = origin.to_owned();
            let source = js_number(source);
            self.context.with(|ctx| {
                let deliver: Function = ctx.globals().get("__tbDeliverMessageError")?;
                deliver.call::<_, ()>((source, origin))?;
                Ok(())
            })
        })
    }

    /// Fires one `storage` event in this realm; `storageArea` is this realm's
    /// own area, per the spec's broadcast steps
    /// (<https://html.spec.whatwg.org/multipage/webstorage.html#concept-storage-broadcast>).
    pub(crate) fn fire_storage_event(
        &self,
        event: &crate::storage::PendingStorageEvent,
    ) -> Result<(), JsError> {
        let kind = event.kind.as_str();
        let key = event.key.clone();
        let old_value = event.old_value.clone();
        let new_value = event.new_value.clone();
        let url = event.url.clone();
        self.with_budget(None, || {
            self.context.with(|ctx| {
                let fire: Function = ctx.globals().get("__tbFireStorageEvent")?;
                fire.call::<_, ()>((kind, key, old_value, new_value, url))?;
                Ok(())
            })
        })
    }

    /// Dispatches one cross-tab `message` event in this realm; the source is
    /// `null` because the posting window lives in another renderer.
    /// (<https://html.spec.whatwg.org/multipage/web-messaging.html#window-post-message-steps>)
    pub(crate) fn deliver_remote_message(&self, payload: &str) -> Result<(), JsError> {
        let payload = payload.to_owned();
        self.with_budget(None, || {
            self.context.with(|ctx| {
                let deliver: Function = ctx.globals().get("__tbDeliverRemoteMessage")?;
                deliver.call::<_, ()>((payload,))?;
                Ok(())
            })
        })
    }

    /// Decodes and dispatches one channel message in this realm.
    pub(crate) fn deliver_port_message(
        &self,
        endpoint: u64,
        payload: &str,
        ports: &[u64],
    ) -> Result<bool, JsError> {
        self.with_budget(None, || {
            let payload = payload.to_owned();
            let ports: Vec<f64> = ports.iter().map(|port| js_number(*port)).collect();
            let endpoint = js_number(endpoint);
            self.context.with(|ctx| {
                let deliver: Function = ctx.globals().get("__tbDeliverPortMessage")?;
                Ok(deliver.call((endpoint, payload, ports))?)
            })
        })
    }

    /// Dispatches `messageerror` at a port whose payload failed to decode.
    pub(crate) fn deliver_port_message_error(&self, endpoint: u64) -> Result<(), JsError> {
        self.with_budget(None, || {
            let endpoint = js_number(endpoint);
            self.context.with(|ctx| {
                let deliver: Function = ctx.globals().get("__tbDeliverPortMessageError")?;
                deliver.call::<_, ()>((endpoint,))?;
                Ok(())
            })
        })
    }

    /// Applies the proxy writes this realm stored for `frame` before its
    /// realm existed.
    pub(crate) fn flush_frame_sets(&self, frame: u64) -> Result<(), JsError> {
        self.with_budget(None, || {
            let frame = js_number(frame);
            self.context.with(|ctx| {
                let flush: Function = ctx.globals().get("__tbFlushFrameSets")?;
                flush.call::<_, ()>((frame,))?;
                Ok(())
            })
        })
    }

    /// Fires `close` at one channel endpoint
    /// (<https://html.spec.whatwg.org/multipage/web-messaging.html#disentangle>).
    pub(crate) fn deliver_port_close(&self, endpoint: u64) -> Result<(), JsError> {
        self.with_budget(None, || {
            let endpoint = js_number(endpoint);
            self.context.with(|ctx| {
                let deliver: Function = ctx.globals().get("__tbDeliverPortClose")?;
                deliver.call::<_, ()>((endpoint,))?;
                Ok(())
            })
        })
    }

    /// Microtask checkpoint for parser-driven mutations: schedules the
    /// delivery microtask when records are pending and runs the job queue.
    ///
    /// Parser insertions record mutations without entering a JS binding, so
    /// nothing else schedules delivery. Called between parser scripts, where
    /// the spec drains microtasks before the next script runs.
    pub(crate) fn deliver_mutations(&self) -> Result<(), JsError> {
        self.with_budget(None, || {
            self.context.with(|ctx| {
                bindings::schedule_mutation_delivery(&ctx)?;
                Ok(())
            })
        })
    }

    /// Runs `operation` under the script deadline and interrupt handler, then
    /// drains the job queue and clears the interrupt handler.
    ///
    /// The job drain always runs so a scheduled microtask cannot outlive the
    /// operation that scheduled it; the operation's error wins over a job
    /// error, and an interrupt wins over both.
    fn with_budget<T>(
        &self,
        deadline: Option<Instant>,
        operation: impl FnOnce() -> Result<T, JsError>,
    ) -> Result<T, JsError> {
        let deadline = deadline.unwrap_or_else(|| Instant::now() + DEFAULT_SCRIPT_BUDGET);
        let interrupted = Rc::new(Cell::new(false));
        let flag = Rc::clone(&interrupted);
        let stop = Arc::clone(&self.stop);
        self.runtime.set_interrupt_handler(Some(Box::new(move || {
            let should_interrupt = stop.is_set() || Instant::now() >= deadline;
            flag.set(should_interrupt);
            should_interrupt
        })));
        let _clear = ClearInterrupt {
            runtime: &self.runtime,
        };
        let result = operation();
        // https://html.spec.whatwg.org/multipage/webappapis.html#clean-up-after-running-script
        let jobs = self.run_jobs();
        if interrupted.get() {
            Err(JsError::Interrupted)
        } else {
            result.and_then(|value| jobs.map(|()| value))
        }
    }

    fn run_jobs(&self) -> Result<(), JsError> {
        loop {
            match self.runtime.execute_pending_job() {
                Ok(true) => {}
                Ok(false) => return Ok(()),
                Err(err) => return Err(JsError::engine(err)),
            }
        }
    }

    fn install(&self) -> Result<(), JsError> {
        let world = self.world.clone();
        self.context.with(|ctx| {
            bindings::install(&ctx, &world)?;
            intl::install(&ctx)?;
            self.install_task_host_functions(&ctx, &world)?;
            Self::install_document_host_functions(&ctx, &world)?;
            install_storage_host_functions(&ctx, &world)?;
            install_window_host_functions(&ctx, &world)?;
            bindings::install_messaging(&ctx)?;
            ctx.eval::<(), _>(INSTALL_WEB_APIS_JS)?;
            Ok(())
        })
    }

    /// Timer and fetch submission hooks the JS shim calls.
    fn install_task_host_functions(
        &self,
        ctx: &Ctx<'_>,
        world: &Rc<RefCell<World>>,
    ) -> Result<(), JsError> {
        let timeouts = self.pending_timeouts.clone();
        let fetches = self.pending_fetches.clone();
        let cancel_world = world.clone();

        ctx.globals().set(
            "__scheduleTimeout",
            Func::from(move |js_id: i32, delay: f64| {
                timeouts.borrow_mut().push(PendingTimeout {
                    delay: Duration::from_millis(u64::from(millis(delay))),
                    js_id,
                });
            }),
        )?;

        ctx.globals().set(
            "__cancelTimeout",
            Func::from(move |js_id: i32| {
                cancel_world.borrow_mut().pending_cancels.push(js_id);
            }),
        )?;

        ctx.globals().set(
            "__queueFetch",
            Func::from(move |url: String, js_id: i32| {
                fetches.borrow_mut().push(PendingJsFetch { url, js_id });
            }),
        )?;
        Ok(())
    }

    /// Cookie, object URL, and URL resolution hooks the JS shim calls.
    fn install_document_host_functions(
        ctx: &Ctx<'_>,
        world: &Rc<RefCell<World>>,
    ) -> Result<(), JsError> {
        let cookie_get = world.clone();
        let cookie_set = world.clone();
        let object_url_create = world.clone();
        let object_url_revoke = world.clone();
        let object_url_contents = world.clone();
        let object_url_type = world.clone();
        let url_resolve = world.clone();

        ctx.globals().set(
            "__cookieGet",
            Func::from(move || {
                let world = cookie_get.borrow();
                world.runtime.services.cookies_for(&world.document_url)
            }),
        )?;

        ctx.globals().set(
            "__cookieSet",
            Func::from(move |value: String| {
                let world = cookie_set.borrow();
                world
                    .runtime
                    .services
                    .set_cookie(&value, &world.document_url);
            }),
        )?;

        ctx.globals().set(
            "__tbCreateObjectURL",
            Func::from(move |contents: String, content_type: String| {
                object_url_create
                    .borrow_mut()
                    .create_object_url(contents, content_type)
            }),
        )?;

        ctx.globals().set(
            "__tbRevokeObjectURL",
            Func::from(move |url: String| {
                object_url_revoke.borrow_mut().revoke_object_url(&url);
            }),
        )?;

        ctx.globals().set(
            "__tbObjectUrlContents",
            Func::from(move |url: String| {
                object_url_contents
                    .borrow()
                    .object_url_contents(&url)
                    .map(|contents| contents.to_string())
            }),
        )?;

        ctx.globals().set(
            "__tbObjectUrlType",
            Func::from(move |url: String| {
                object_url_type
                    .borrow()
                    .object_url_type(&url)
                    .map(|content_type| content_type.to_string())
            }),
        )?;

        ctx.globals().set(
            "__tbParseUrl",
            Func::from(|input: String| url::Url::parse(&input).ok().map(|url| url.to_string())),
        )?;

        ctx.globals().set(
            "__tbResolveUrl",
            Func::from(move |input: String, base: Option<String>| {
                let fallback = url_resolve.borrow().document_url.clone();
                let resolved = match base {
                    Some(base) => url::Url::parse(&base)
                        .ok()
                        .and_then(|base| base.join(&input).ok()),
                    None => url::Url::parse(&input)
                        .ok()
                        .or_else(|| fallback.join(&input).ok()),
                };
                resolved.map(|url| url.to_string())
            }),
        )?;
        Ok(())
    }
}

/// `window.open`/`window.close` hooks the JS shim calls.
fn install_window_host_functions(ctx: &Ctx<'_>, world: &Rc<RefCell<World>>) -> Result<(), JsError> {
    let window_open = world.clone();
    ctx.globals().set(
        "__tbWindowOpen",
        Func::from(
            move |url: String,
                  name: String,
                  features: String,
                  seed_origin: String,
                  seed_entries: Vec<String>| {
                let spec = if url.is_empty() {
                    Some(String::new())
                } else {
                    window_open
                        .borrow()
                        .document_url
                        .join(&url)
                        .ok()
                        .map(|url| url.to_string())
                };
                let seed = if seed_origin.is_empty() {
                    None
                } else {
                    Some(crate::protocol::StorageSeed {
                        origin: seed_origin,
                        entries: seed_entries
                            .as_chunks::<2>()
                            .0
                            .iter()
                            .map(|pair| (pair[0].clone(), pair[1].clone()))
                            .collect(),
                    })
                };
                spec.and_then(|spec| {
                    window_open.borrow().runtime.services.window_open(
                        &spec,
                        &name,
                        &features,
                        seed.as_ref(),
                    )
                })
            },
        ),
    )?;

    let window_close = world.clone();
    ctx.globals().set(
        "__tbWindowClose",
        Func::from(move |tab: u64| {
            window_close.borrow().runtime.services.window_close(tab);
        }),
    )?;

    let opener = world.clone();
    ctx.globals().set(
        "__tbWindowOpener",
        Func::from(move || opener.borrow().runtime.services.window_opener()),
    )?;

    let post_message = world.clone();
    ctx.globals().set(
        "__tbWindowPostMessage",
        Func::from(move |tab: u64, payload: String| {
            post_message
                .borrow()
                .runtime
                .services
                .window_post_message(tab, &payload);
        }),
    )?;

    let remote_session = world.clone();
    ctx.globals().set(
        "__tbRemoteSessionGet",
        Func::from(move |tab: u64, key: String| {
            let world = remote_session.borrow();
            world.storage_origin().and_then(|origin| {
                world
                    .runtime
                    .services
                    .remote_session_get(tab, &origin, &key)
            })
        }),
    )?;
    Ok(())
}

/// `localStorage`/`sessionStorage` hooks the JS shim calls. The session area
/// lives in the engine; the local area crosses the service seam.
fn install_storage_host_functions(
    ctx: &Ctx<'_>,
    world: &Rc<RefCell<World>>,
) -> Result<(), JsError> {
    let origin = world.clone();
    ctx.globals().set(
        "__tbStorageOrigin",
        Func::from(move || origin.borrow().storage_origin()),
    )?;

    let get = world.clone();
    ctx.globals().set(
        "__tbStorageGet",
        Func::from(move |kind: String, key: String| get.borrow().storage_get(&kind, &key)),
    )?;

    let keys = world.clone();
    ctx.globals().set(
        "__tbStorageKeys",
        Func::from(move |kind: String| keys.borrow().storage_keys(&kind)),
    )?;

    let set = world.clone();
    ctx.globals().set(
        "__tbStorageSet",
        Func::from(move |kind: String, key: String, value: String| {
            set.borrow().storage_set(&kind, &key, &value).is_ok()
        }),
    )?;

    let remove = world.clone();
    ctx.globals().set(
        "__tbStorageRemove",
        Func::from(move |kind: String, key: String| {
            remove.borrow().storage_remove(&kind, &key).is_some()
        }),
    )?;

    let clear = world.clone();
    ctx.globals().set(
        "__tbStorageClear",
        Func::from(move |kind: String| clear.borrow().storage_clear(&kind).is_some()),
    )?;
    Ok(())
}

impl Drop for JsRealm {
    fn drop(&mut self) {
        bindings::forget_world(&self.context);
        let world = self.world.clone();
        let mut world = world.borrow_mut();
        // Release this realm's cached wrappers and document associations
        // before its QuickJS context goes away; sibling realms keep theirs.
        world.forget_owned_documents();
        world.clear_listeners();
    }
}

pub(crate) fn classic_script_at(world: &World, id: dom::NodeId) -> Option<ClassicScript> {
    let parsed = world.document(id)?;
    if !is_classic_script(&parsed.dom, id) {
        return None;
    }
    match parsed.dom.attribute(id, "src") {
        Some(src) if !src.trim().is_empty() => Some(ClassicScript::Src(src)),
        _ => Some(ClassicScript::Inline(element_text(&parsed.dom, id))),
    }
}

fn is_classic_script(tree: &dom::Dom, id: dom::NodeId) -> bool {
    match tree.kind(id) {
        Some(dom::NodeKind::Element { name, .. })
            if name.ns == dom::html_namespace()
                && name.local.as_ref().eq_ignore_ascii_case("script") =>
        {
            javascript_mime(tree.attribute(id, "type").as_deref())
        }
        _ => false,
    }
}

fn javascript_mime(typ: Option<&str>) -> bool {
    let Some(typ) = typ.map(str::trim).filter(|typ| !typ.is_empty()) else {
        return true;
    };
    let essence = typ
        .split(';')
        .next()
        .unwrap_or(typ)
        .trim()
        .to_ascii_lowercase();
    // https://mimesniff.spec.whatwg.org/#javascript-mime-type
    matches!(
        essence.as_str(),
        "application/ecmascript"
            | "application/javascript"
            | "application/x-ecmascript"
            | "application/x-javascript"
            | "text/ecmascript"
            | "text/javascript"
            | "text/javascript1.0"
            | "text/javascript1.1"
            | "text/javascript1.2"
            | "text/javascript1.3"
            | "text/javascript1.4"
            | "text/javascript1.5"
            | "text/jscript"
            | "text/livescript"
            | "text/x-ecmascript"
            | "text/x-javascript"
    )
}

fn element_text(tree: &dom::Dom, id: dom::NodeId) -> String {
    let mut text = String::new();
    let mut stack: Vec<_> = tree
        .children(id)
        .map(|kids| kids.copied().collect())
        .unwrap_or_default();
    stack.reverse();
    while let Some(child) = stack.pop() {
        match tree.kind(child) {
            Some(dom::NodeKind::Text { data }) => text.push_str(data),
            Some(dom::NodeKind::Element { .. }) => {
                if let Some(kids) = tree.children(child) {
                    let mut kids: Vec<_> = kids.copied().collect();
                    kids.reverse();
                    stack.extend(kids);
                }
            }
            _ => {}
        }
    }
    text
}

struct ClearInterrupt<'a> {
    runtime: &'a Runtime,
}

impl Drop for ClearInterrupt<'_> {
    fn drop(&mut self) {
        self.runtime.set_interrupt_handler(None);
    }
}

fn eval_classic<'js, V: FromJs<'js>>(ctx: &rquickjs::Ctx<'js>, source: &str) -> Result<V, JsError> {
    let mut options = EvalOptions::default();
    options.strict = false;
    ctx.eval_with_options(source, options)
        .map_err(|error| match error {
            rquickjs::Error::Exception => {
                let caught = ctx.catch();
                JsError::engine(match caught.into_exception() {
                    Some(exception) => format!(
                        "{} | {}",
                        exception.message().unwrap_or_default(),
                        exception.stack().unwrap_or_default()
                    ),
                    None => String::from("uncaught exception"),
                })
            }
            other => JsError::engine(other),
        })
}

fn decode_value<'js>(ctx: &rquickjs::Ctx<'js>, value: Value<'js>) -> Result<ScriptValue, JsError> {
    decode_value_inner(ctx, value, 0)
}

fn decode_value_inner<'js>(
    ctx: &rquickjs::Ctx<'js>,
    value: Value<'js>,
    depth: u8,
) -> Result<ScriptValue, JsError> {
    if depth > 32 {
        return Ok(ScriptValue::Null);
    }
    if value.is_undefined() {
        return Ok(ScriptValue::Undefined);
    }
    if value.is_null() {
        return Ok(ScriptValue::Null);
    }
    if let Some(flag) = value.as_bool() {
        return Ok(ScriptValue::Bool(flag));
    }
    if let Some(number) = value.as_number() {
        return Ok(ScriptValue::Number(number));
    }
    if let Ok(node) = rquickjs::Class::<bindings::JsNode>::from_js(ctx, value.clone()) {
        return Ok(ScriptValue::Node(node.borrow().node_id()));
    }
    if let Some(id) = bindings::host_node_id(ctx, &value) {
        return Ok(ScriptValue::Node(id));
    }
    if let Some(array) = value.as_array() {
        let mut items = Vec::with_capacity(array.len());
        for index in 0..array.len() {
            let item: Value = array.get(index)?;
            items.push(decode_value_inner(ctx, item, depth.saturating_add(1))?);
        }
        return Ok(ScriptValue::List(items));
    }
    if value.as_function().is_some() {
        return Ok(ScriptValue::Null);
    }
    if let Some(object) = value.as_object() {
        let mut map = Vec::new();
        for key in object.keys::<String>() {
            let key = key?;
            let nested: Value = object.get(key.as_str())?;
            map.push((
                key,
                decode_value_inner(ctx, nested, depth.saturating_add(1))?,
            ));
        }
        return Ok(ScriptValue::Map(map));
    }
    Ok(ScriptValue::String(
        Coerced::<String>::from_js(ctx, value)?.0,
    ))
}

fn render_eval_result<'js>(ctx: &rquickjs::Ctx<'js>, value: Value<'js>) -> Result<String, JsError> {
    if value.is_undefined() || value.is_null() {
        return Ok(String::new());
    }
    Ok(Coerced::<String>::from_js(ctx, value)?.0)
}

fn millis(delay: f64) -> u32 {
    if !delay.is_finite() || delay <= 0.0 {
        return 0;
    }
    let duration = Duration::from_secs_f64((delay / 1000.0).clamp(0.0, 86_400.0));
    u32::try_from(duration.as_millis()).unwrap_or(u32::MAX)
}

/// A protocol id as the JS number the shim exchanges; ids are small counters,
/// so a value that cannot be numbered exactly is not addressable at all.
pub(crate) fn js_number(id: u64) -> f64 {
    u32::try_from(id).map_or(f64::NAN, f64::from)
}
