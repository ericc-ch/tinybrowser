//! `QuickJS` host for one [`Page`]: eval, `setTimeout`, `fetch`, `document.cookie`.
//!
//! Callbacks live in JS (`__tb_timeouts`, `__tb_fetchCbs`). Rust holds
//! integer ids so a `Function` never crosses `spawn_blocking`. Invocation
//! uses `Function::call` on the page thread.

use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;
use std::time::Duration;

use net::Agent;
use rquickjs::{Array, Coerced, Context, FromJs, Function, Object, Runtime, Value, prelude::Func};
use url::Url;

#[derive(Debug)]
pub(crate) enum JsError {
    Engine(Box<str>),
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

pub(crate) struct JsHost {
    runtime: Runtime,
    context: Context,
    pending_timeouts: Rc<RefCell<Vec<PendingTimeout>>>,
    pending_fetches: Rc<RefCell<Vec<PendingJsFetch>>>,
    cookie: Rc<RefCell<(Agent, Url)>>,
}

impl JsHost {
    pub(crate) fn new(agent: Agent, document_url: Url) -> Result<Self, JsError> {
        let runtime = Runtime::new().map_err(JsError::engine)?;
        let context = Context::full(&runtime).map_err(JsError::engine)?;
        let pending_timeouts = Rc::new(RefCell::new(Vec::new()));
        let pending_fetches = Rc::new(RefCell::new(Vec::new()));
        let cookie = Rc::new(RefCell::new((agent, document_url)));
        let host = Self {
            runtime,
            context,
            pending_timeouts,
            pending_fetches,
            cookie,
        };
        host.install()?;
        Ok(host)
    }

    pub(crate) fn set_document_url(&self, url: Url) {
        self.cookie.borrow_mut().1 = url;
    }

    pub(crate) fn eval(&self, source: &str) -> Result<String, JsError> {
        let rendered: Result<String, JsError> = self.context.with(|ctx| {
            let value: Value = ctx.eval(source).map_err(JsError::engine)?;
            render_eval_result(&ctx, value)
        });
        let out = rendered?;
        self.run_jobs()?;
        Ok(out)
    }

    pub(crate) fn take_pending_timeouts(&self) -> Vec<PendingTimeout> {
        std::mem::take(&mut *self.pending_timeouts.borrow_mut())
    }

    pub(crate) fn take_pending_fetches(&self) -> Vec<PendingJsFetch> {
        std::mem::take(&mut *self.pending_fetches.borrow_mut())
    }

    pub(crate) fn fire_timer(&self, js_id: i32) -> Result<(), JsError> {
        let called: Result<(), JsError> = self.context.with(|ctx| {
            let timeouts: Array = ctx
                .globals()
                .get("__tb_timeouts")
                .map_err(JsError::engine)?;
            let idx = usize::try_from(js_id).map_err(|_| JsError::BadTimerId)?;
            let func: Function = timeouts.get(idx).map_err(JsError::engine)?;
            func.call(()).map_err(JsError::engine)
        });
        let jobs = self.run_jobs();
        called.and(jobs)
    }

    pub(crate) fn finish_js_fetch(
        &self,
        js_id: i32,
        ok: bool,
        status: i32,
        body: &str,
    ) -> Result<(), JsError> {
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
        let cookie_get = self.cookie.clone();
        let cookie_set = self.cookie.clone();
        self.context.with(|ctx| {
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
                        let (agent, url) = cookie_get.borrow().clone();
                        agent.cookies_for(&url)
                    }),
                )
                .map_err(JsError::engine)?;

            ctx.globals()
                .set(
                    "__cookieSet",
                    Func::from(move |value: String| {
                        let (agent, url) = cookie_set.borrow().clone();
                        agent.set_cookie(&value, &url);
                    }),
                )
                .map_err(JsError::engine)?;

            ctx.eval::<(), _>(
                r"
globalThis.__tb_timeouts = [];
globalThis.__tb_fetchCbs = Object.create(null);
globalThis.__tb_fetchSeq = 0;
['__scheduleTimeout','__queueFetch','__cookieGet','__cookieSet','__tb_timeouts','__tb_fetchCbs'].forEach(function(k) {
  Object.defineProperty(globalThis, k, { writable: false, configurable: false, enumerable: false });
});
globalThis.setTimeout = function(fn, ms) {
  var id = globalThis.__tb_timeouts.length;
  globalThis.__tb_timeouts.push(fn);
  globalThis.__scheduleTimeout(id, Number(ms));
  return id;
};
globalThis.document = {};
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
