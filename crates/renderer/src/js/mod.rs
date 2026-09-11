//! `QuickJS` host for one [`crate::Document`]: eval, timers, `fetch`, DOM host objects.
//!
//! Callbacks live in JS (`__tb_timeouts`, `__tb_fetchCbs`). Rust holds
//! integer ids so a `Function` never crosses the JS boundary. Invocation
//! uses `Function::call` on the renderer thread.

mod bindings;
mod world;

use std::cell::{Cell, OnceCell, RefCell};
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rquickjs::{
    Array, Coerced, Context, FromJs, Function, Object, Runtime, Value, context::EvalOptions,
    prelude::Func,
};

pub(crate) use world::World;

use crate::document::Stop;

const MAX_RUNTIME_MEMORY: usize = 32 * 1024 * 1024;
const MAX_RUNTIME_STACK: usize = 512 * 1024;
const DEFAULT_SCRIPT_BUDGET: Duration = Duration::from_secs(5);

/// A value produced by [`crate::Document::execute_script`].
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
/// model of [ADR 0014](../../../../docs/adrs/0014-frames-and-per-frame-realms.md).
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
        let context = Context::full(&runtime).map_err(JsError::engine)?;
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
            let rendered: Result<String, JsError> = self.context.with(|ctx| {
                let value: Value = eval_classic(&ctx, source)?;
                render_eval_result(&ctx, value)
            });
            // https://html.spec.whatwg.org/multipage/webappapis.html#clean-up-after-running-script
            let jobs = self.run_jobs();
            rendered.and_then(|out| jobs.map(|()| out))
        })
    }

    pub(crate) fn eval_value_deadline(
        &self,
        source: &str,
        deadline: Option<Instant>,
    ) -> Result<crate::js::ScriptValue, JsError> {
        self.with_budget(deadline, || {
            let decoded: Result<ScriptValue, JsError> = self.context.with(|ctx| {
                let value: Value = eval_classic(&ctx, source)?;
                decode_value(&ctx, value)
            });
            let jobs = self.run_jobs();
            decoded.and_then(|value| jobs.map(|()| value))
        })
    }

    pub(crate) fn has_pending_work(&self) -> bool {
        !self.pending_timeouts.borrow().is_empty() || !self.pending_fetches.borrow().is_empty()
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
            let called: Result<(), JsError> = self.context.with(|ctx| {
                let timeouts: Array = ctx
                    .globals()
                    .get("__tb_timeouts")
                    .map_err(JsError::engine)?;
                let idx = usize::try_from(js_id).map_err(|_| JsError::BadTimerId)?;
                let func: Function = timeouts.get(idx).map_err(JsError::engine)?;
                timeouts
                    .as_object()
                    .remove(js_id)
                    .map_err(JsError::engine)?;
                func.call(()).map_err(JsError::engine)
            });
            let jobs = self.run_jobs();
            called.and(jobs)
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
            let called: Result<(), JsError> = self.context.with(|ctx| {
                let cbs: Object = ctx
                    .globals()
                    .get("__tb_fetchCbs")
                    .map_err(JsError::engine)?;
                let func: Function = cbs.get(js_id).map_err(JsError::engine)?;
                func.call((ok, status, body)).map_err(JsError::engine)
            });
            let jobs = self.run_jobs();
            called.and(jobs)
        })
    }

    pub(crate) fn fire_load(&self) -> Result<(), JsError> {
        self.with_budget(None, || {
            let fired: Result<(), JsError> = self
                .context
                .with(|ctx| bindings::fire_window_load(&ctx).map_err(JsError::engine));
            let jobs = self.run_jobs();
            fired.and(jobs)
        })
    }

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
        if interrupted.get() {
            Err(JsError::Interrupted)
        } else {
            result
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
        let timeouts = self.pending_timeouts.clone();
        let fetches = self.pending_fetches.clone();
        let world = self.world.clone();
        let cookie_get = world.clone();
        let cookie_set = world.clone();
        let cancel_world = world.clone();
        self.context.with(|ctx| {
            bindings::install(&ctx, &world).map_err(JsError::engine)?;

            ctx.globals()
                .set(
                    "__scheduleTimeout",
                    Func::from(move |js_id: i32, delay: f64| {
                        timeouts.borrow_mut().push(PendingTimeout {
                            delay: Duration::from_millis(u64::from(millis(delay))),
                            js_id,
                        });
                    }),
                )
                .map_err(JsError::engine)?;

            ctx.globals()
                .set(
                    "__cancelTimeout",
                    Func::from(move |js_id: i32| {
                        cancel_world.borrow_mut().pending_cancels.push(js_id);
                    }),
                )
                .map_err(JsError::engine)?;

            ctx.globals()
                .set(
                    "__queueFetch",
                    Func::from(move |url: String, js_id: i32| {
                        fetches.borrow_mut().push(PendingJsFetch { url, js_id });
                    }),
                )
                .map_err(JsError::engine)?;

            ctx.globals()
                .set(
                    "__cookieGet",
                    Func::from(move || {
                        let world = cookie_get.borrow();
                        world.services.cookies_for(&world.document_url)
                    }),
                )
                .map_err(JsError::engine)?;

            ctx.globals()
                .set(
                    "__cookieSet",
                    Func::from(move |value: String| {
                        let world = cookie_set.borrow();
                        world.services.set_cookie(&value, &world.document_url);
                    }),
                )
                .map_err(JsError::engine)?;

            ctx.eval::<(), _>(
                r"
globalThis.__tb_timeouts = [];
globalThis.__tb_fetchCbs = Object.create(null);
globalThis.__tb_fetchSeq = 0;
['__scheduleTimeout','__cancelTimeout','__queueFetch','__cookieGet','__cookieSet','__tb_timeouts','__tb_fetchCbs'].forEach(function(k) {
  Object.defineProperty(globalThis, k, { writable: false, configurable: false, enumerable: false });
});
globalThis.setTimeout = function(fn, ms) {
  var id = globalThis.__tb_timeouts.length;
  globalThis.__tb_timeouts.push(fn);
  globalThis.__scheduleTimeout(id, Number(ms));
  return id;
};
globalThis.clearTimeout = function(id) {
  globalThis.__tb_timeouts[id] = function() {};
  globalThis.__cancelTimeout(Number(id));
};
if (!globalThis.document) {
  globalThis.document = {};
}
Object.defineProperty(document, 'cookie', {
  get() { return globalThis.__cookieGet(); },
  set(v) { globalThis.__cookieSet(String(v)); }
});
globalThis.fetch = function(url) {
  return new Promise(function(resolve, reject) {
    var id = ++globalThis.__tb_fetchSeq;
    globalThis.__tb_fetchCbs[id] = function(ok, status, body) {
      delete globalThis.__tb_fetchCbs[id];
      if (ok) resolve({
        status: status,
        text: function() { return Promise.resolve(String(body)); }
      });
      else reject(new Error('fetch failed'));
    };
    globalThis.__queueFetch(String(url), id);
  });
};
",
            )
            .map_err(JsError::engine)?;
            Ok(())
        })
    }
}

impl Drop for JsRealm {
    fn drop(&mut self) {
        bindings::forget_world(&self.context);
        self.world.borrow_mut().clear_listeners();
    }
}

pub(crate) fn classic_script_at(world: &World, id: dom::NodeId) -> Option<ClassicScript> {
    let parsed = world.parsed.as_ref()?;
    if !is_classic_script(&parsed.dom, id) {
        return None;
    }
    match parsed.dom.attribute(id, "src") {
        Some(src) if !src.trim().is_empty() => Some(ClassicScript::Src(src)),
        _ => Some(ClassicScript::Inline(element_text(&parsed.dom, id))),
    }
}

fn is_classic_script(tree: &dom::Dom, id: dom::NodeId) -> bool {
    match tree.get(id).map(|node| node.kind()) {
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
        match tree.get(child).map(|node| node.kind()) {
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
        .map_err(JsError::engine)
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
            let item: Value = array.get(index).map_err(JsError::engine)?;
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
            let key = key.map_err(JsError::engine)?;
            let nested: Value = object.get(key.as_str()).map_err(JsError::engine)?;
            map.push((
                key,
                decode_value_inner(ctx, nested, depth.saturating_add(1))?,
            ));
        }
        return Ok(ScriptValue::Map(map));
    }
    Coerced::<String>::from_js(ctx, value)
        .map(|coerced| ScriptValue::String(coerced.0))
        .map_err(JsError::engine)
}

fn render_eval_result<'js>(ctx: &rquickjs::Ctx<'js>, value: Value<'js>) -> Result<String, JsError> {
    if value.is_undefined() || value.is_null() {
        return Ok(String::new());
    }
    Coerced::<String>::from_js(ctx, value)
        .map(|coerced| coerced.0)
        .map_err(JsError::engine)
}

fn millis(delay: f64) -> u32 {
    if !delay.is_finite() || delay <= 0.0 {
        return 0;
    }
    let duration = Duration::from_secs_f64((delay / 1000.0).clamp(0.0, 86_400.0));
    u32::try_from(duration.as_millis()).unwrap_or(u32::MAX)
}
