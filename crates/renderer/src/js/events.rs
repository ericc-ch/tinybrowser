//! DOM events: the `Event` and `EventTarget` platform objects and the
//! dispatch algorithm (<https://dom.spec.whatwg.org/#events>).
//!
//! One event listener list is keyed per target in the [`World`] that owns the
//! target, so nodes, the window, and constructible `EventTarget`s all share
//! the same add/remove/invoke code.

use std::cell::{Cell, Ref, RefCell, RefMut};
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::Instant;

use rquickjs::{
    Array, Class, Ctx, Exception, FromJs, Object, Persistent, Result, Value,
    class::{Trace, Tracer},
    function::{Opt, Rest, This},
};

use super::bindings;
use super::world::{EventTargetKey, Listener, World};

/// `Event.NONE` (<https://dom.spec.whatwg.org/#dom-event-none>).
pub(crate) const NONE: u16 = 0;
/// `Event.CAPTURING_PHASE` (<https://dom.spec.whatwg.org/#dom-event-capturing_phase>).
pub(crate) const CAPTURING_PHASE: u16 = 1;
/// `Event.AT_TARGET` (<https://dom.spec.whatwg.org/#dom-event-at_target>).
pub(crate) const AT_TARGET: u16 = 2;
/// `Event.BUBBLING_PHASE` (<https://dom.spec.whatwg.org/#dom-event-bubbling_phase>).
pub(crate) const BUBBLING_PHASE: u16 = 3;

/// `DOMHighResTimeStamp` for `Event.timeStamp`, relative to this process's
/// first event (<https://dom.spec.whatwg.org/#dom-event-timestamp>).
fn now_millis() -> f64 {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    let origin = ORIGIN.get_or_init(Instant::now);
    origin.elapsed().as_secs_f64() * 1000.0
}

/// One event target together with the world that owns it
/// (<https://dom.spec.whatwg.org/#concept-event-target>).
///
/// The world is needed to resolve a `Window` target from a realm that is not
/// the target's own: `ctx.globals()` is the caller's window.
#[derive(Clone)]
pub(crate) struct EventTargetRef {
    pub(crate) key: EventTargetKey,
    pub(crate) world: Rc<RefCell<World>>,
}

/// The mutable state of one `Event` object
/// (<https://dom.spec.whatwg.org/#concept-event>).
///
/// Targets and path entries are [`EventTargetRef`]s, not JS values: a
/// `Persistent` stored inside a class instance pins the `QuickJS` context
/// across realm teardown, which aborts the runtime at exit. References resolve
/// to wrappers on demand and hold no JS reference.
///
/// The booleans mirror the specification's event flags one-for-one; a bitmask
/// would obscure that mapping.
#[allow(
    clippy::struct_excessive_bools,
    reason = "the DOM event flag set is specified as individual flags"
)]
#[derive(Default)]
pub(crate) struct EventState {
    pub(crate) typ: String,
    pub(crate) bubbles: bool,
    pub(crate) cancelable: bool,
    pub(crate) composed: bool,
    pub(crate) is_trusted: bool,
    pub(crate) initialized: bool,
    pub(crate) time_stamp: f64,
    pub(crate) target: Option<EventTargetRef>,
    pub(crate) current_target: Option<EventTargetRef>,
    pub(crate) related_target: Option<EventTargetRef>,
    pub(crate) phase: u16,
    pub(crate) stop_propagation: bool,
    pub(crate) stop_immediate: bool,
    pub(crate) canceled: bool,
    pub(crate) in_passive: bool,
    pub(crate) dispatching: bool,
    pub(crate) path: Vec<EventTargetRef>,
}

pub(crate) struct EventStateCell(RefCell<EventState>);

impl<'js> Trace<'js> for EventStateCell {
    fn trace<'a>(&self, _tracer: Tracer<'a, 'js>) {}
}

/// The `CustomEvent` platform object.
///
/// `detail` is the one `any` attribute of the interface, so it lives as a
/// symbol-keyed own property on the object (`INSTALL_CUSTOM_EVENT_JS`) rather
/// than in this struct; every Rust-held JS value would pin the context.
#[derive(Trace, rquickjs::JsLifetime)]
#[rquickjs::class(rename = "Event")]
pub struct JsEvent {
    pub(crate) state: EventStateCell,
}

impl JsEvent {
    /// A user-agent-created event: `isTrusted` true and `initialized` set
    /// (<https://dom.spec.whatwg.org/#concept-event-create>).
    pub(crate) fn trusted(typ: &str, bubbles: bool, cancelable: bool) -> Self {
        Self {
            state: EventStateCell(RefCell::new(EventState {
                typ: typ.to_owned(),
                bubbles,
                cancelable,
                is_trusted: true,
                initialized: true,
                time_stamp: now_millis(),
                ..EventState::default()
            })),
        }
    }

    /// The event `Document.createEvent()` returns: type empty and
    /// `initialized` unset until `initEvent()`
    /// (<https://dom.spec.whatwg.org/#dom-document-createevent>).
    pub(crate) fn uninitialized() -> Self {
        Self {
            state: EventStateCell(RefCell::new(EventState {
                time_stamp: now_millis(),
                ..EventState::default()
            })),
        }
    }

    pub(crate) fn state(&self) -> Ref<'_, EventState> {
        self.state.0.borrow()
    }

    pub(crate) fn state_mut(&self) -> RefMut<'_, EventState> {
        self.state.0.borrow_mut()
    }

    /// [Initialize](https://dom.spec.whatwg.org/#concept-event-initialize) the
    /// event. `initEvent()` and `initCustomEvent()` both land here.
    pub(crate) fn initialize(&self, typ: String, bubbles: bool, cancelable: bool) {
        let mut state = self.state_mut();
        state.initialized = true;
        state.stop_propagation = false;
        state.stop_immediate = false;
        state.canceled = false;
        state.is_trusted = false;
        state.target = None;
        state.related_target = None;
        state.typ = typ;
        state.bubbles = bubbles;
        state.cancelable = cancelable;
    }
}

#[rquickjs::methods]
#[allow(
    clippy::needless_pass_by_value,
    reason = "the rquickjs method macro passes Ctx and This by value"
)]
impl JsEvent {
    // https://dom.spec.whatwg.org/#dom-event-event
    #[qjs(constructor)]
    fn new<'js>(ctx: Ctx<'js>, args: Rest<Value<'js>>) -> Result<Self> {
        let (typ, init) = event_arguments(&ctx, args)?;
        event_from_init(&ctx, typ, init.as_ref())
    }

    // https://dom.spec.whatwg.org/#dom-event-type
    #[qjs(get, rename = "type")]
    fn get_type(&self) -> String {
        self.state().typ.clone()
    }

    // https://dom.spec.whatwg.org/#dom-event-target
    #[qjs(get, rename = "target")]
    fn get_target<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        stored_target(&ctx, self.state().target.as_ref())
    }

    // https://dom.spec.whatwg.org/#dom-event-srcelement
    #[qjs(get, rename = "srcElement")]
    fn get_src_element<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        stored_target(&ctx, self.state().target.as_ref())
    }

    // https://dom.spec.whatwg.org/#dom-event-currenttarget
    #[qjs(get, rename = "currentTarget")]
    fn get_current_target<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        stored_target(&ctx, self.state().current_target.as_ref())
    }

    // https://w3c.github.io/uievents/#dom-focusevent-relatedtarget
    #[qjs(get, rename = "relatedTarget")]
    fn get_related_target<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        stored_target(&ctx, self.state().related_target.as_ref())
    }

    // https://dom.spec.whatwg.org/#dom-event-eventphase
    #[qjs(get, rename = "eventPhase")]
    fn get_event_phase(&self) -> u16 {
        self.state().phase
    }

    // https://dom.spec.whatwg.org/#dom-event-bubbles
    #[qjs(get, rename = "bubbles")]
    fn get_bubbles(&self) -> bool {
        self.state().bubbles
    }

    // https://dom.spec.whatwg.org/#dom-event-cancelable
    #[qjs(get, rename = "cancelable")]
    fn get_cancelable(&self) -> bool {
        self.state().cancelable
    }

    // https://dom.spec.whatwg.org/#dom-event-composed
    #[qjs(get, rename = "composed")]
    fn get_composed(&self) -> bool {
        self.state().composed
    }

    // https://dom.spec.whatwg.org/#dom-event-defaultprevented
    #[qjs(get, rename = "defaultPrevented")]
    fn get_default_prevented(&self) -> bool {
        self.state().canceled
    }

    // https://dom.spec.whatwg.org/#dom-event-istrusted
    #[qjs(get, rename = "isTrusted")]
    fn get_is_trusted(&self) -> bool {
        self.state().is_trusted
    }

    // https://dom.spec.whatwg.org/#dom-event-timestamp
    #[qjs(get, rename = "timeStamp")]
    fn get_time_stamp(&self) -> f64 {
        self.state().time_stamp
    }

    // https://dom.spec.whatwg.org/#dom-event-cancelbubble
    #[qjs(get, rename = "cancelBubble")]
    fn get_cancel_bubble(&self) -> bool {
        self.state().stop_propagation
    }

    #[qjs(set, rename = "cancelBubble")]
    fn set_cancel_bubble(&self, value: Value<'_>) {
        if bindings::to_boolean(&value) {
            self.state_mut().stop_propagation = true;
        }
    }

    // https://dom.spec.whatwg.org/#dom-event-returnvalue
    #[qjs(get, rename = "returnValue")]
    fn get_return_value(&self) -> bool {
        !self.state().canceled
    }

    #[qjs(set, rename = "returnValue")]
    fn set_return_value(&self, value: Value<'_>) {
        if !bindings::to_boolean(&value) {
            self.set_canceled_flag();
        }
    }

    // https://dom.spec.whatwg.org/#dom-event-stoppropagation
    #[qjs(rename = "stopPropagation")]
    fn stop_propagation(&self) {
        self.state_mut().stop_propagation = true;
    }

    // https://dom.spec.whatwg.org/#dom-event-stopimmediatepropagation
    #[qjs(rename = "stopImmediatePropagation")]
    fn stop_immediate_propagation(&self) {
        let mut state = self.state_mut();
        state.stop_propagation = true;
        state.stop_immediate = true;
    }

    // https://dom.spec.whatwg.org/#dom-event-preventdefault
    #[qjs(rename = "preventDefault")]
    fn prevent_default(&self) {
        self.set_canceled_flag();
    }

    // https://dom.spec.whatwg.org/#dom-event-composedpath
    #[qjs(rename = "composedPath")]
    fn composed_path<'js>(&self, ctx: Ctx<'js>) -> Result<Array<'js>> {
        let state = self.state();
        let path = Array::new(ctx.clone())?;
        for (index, reference) in state.path.iter().enumerate() {
            path.set(index, resolve_target(&ctx, reference)?)?;
        }
        Ok(path)
    }

    // https://dom.spec.whatwg.org/#dom-event-initevent
    #[qjs(rename = "initEvent")]
    fn init_event<'js>(&self, ctx: Ctx<'js>, args: Rest<Value<'js>>) -> Result<()> {
        // Web IDL converts the arguments before the algorithm runs, so a
        // missing or throwing `type` fails even while dispatching.
        let mut args = args.0.into_iter();
        let Some(typ) = args.next() else {
            return Err(Exception::throw_type(&ctx, "type is required"));
        };
        let typ = bindings::webidl_to_string(&ctx, typ)?;
        let bubbles = boolean_argument(args.next());
        let cancelable = boolean_argument(args.next());
        if self.state().dispatching {
            return Ok(());
        }
        self.initialize(typ, bubbles, cancelable);
        Ok(())
    }

    /// [Set the canceled flag](https://dom.spec.whatwg.org/#set-the-canceled-flag).
    fn set_canceled_flag(&self) {
        let mut state = self.state_mut();
        if state.canceled || state.in_passive || !state.cancelable {
            return;
        }
        state.canceled = true;
    }
}

/// The constructible `EventTarget` interface
/// (<https://dom.spec.whatwg.org/#interface-eventtarget>).
///
/// Nodes get their own copies of the three methods on the JavaScript `Node`
/// prototype (`INSTALL_BRANDS_JS` copies them from the Rust `Node.prototype`),
/// so these methods only ever see `new EventTarget()` receivers. That matters
/// because rquickjs methods brand-check their receiver as the defining class.
#[derive(Trace, rquickjs::JsLifetime)]
#[rquickjs::class(rename = "EventTarget")]
pub struct JsEventTarget {
    id: u64,
}

#[rquickjs::methods]
#[allow(
    clippy::needless_pass_by_value,
    reason = "the rquickjs method macro passes Ctx and This by value"
)]
impl JsEventTarget {
    #[qjs(constructor)]
    fn new(ctx: Ctx<'_>) -> Result<Self> {
        let id = bindings::world(&ctx)?.borrow_mut().next_standalone_target();
        Ok(Self { id })
    }

    #[qjs(rename = "addEventListener")]
    fn add_event_listener<'js>(
        &self,
        ctx: Ctx<'js>,
        this: This<Object<'js>>,
        typ: Value<'js>,
        callback: Value<'js>,
        options: Opt<Value<'js>>,
    ) -> Result<()> {
        register_standalone(&ctx, self.id, &this.0)?;
        add_listener(
            &ctx,
            EventTargetKey::Standalone(self.id),
            typ,
            callback,
            options.0,
        )
    }

    #[qjs(rename = "removeEventListener")]
    fn remove_event_listener<'js>(
        &self,
        ctx: Ctx<'js>,
        this: This<Object<'js>>,
        typ: Value<'js>,
        callback: Value<'js>,
        options: Opt<Value<'js>>,
    ) -> Result<()> {
        register_standalone(&ctx, self.id, &this.0)?;
        remove_listener(
            &ctx,
            EventTargetKey::Standalone(self.id),
            typ,
            callback,
            options.0,
        )
    }

    #[qjs(rename = "dispatchEvent")]
    fn dispatch_event<'js>(
        &self,
        ctx: Ctx<'js>,
        this: This<Object<'js>>,
        event: Class<'js, JsEvent>,
    ) -> Result<bool> {
        register_standalone(&ctx, self.id, &this.0)?;
        dispatch_event(&ctx, EventTargetKey::Standalone(self.id), &event)
    }

    /// User-agent delivery for a shim-fired event: same as `dispatchEvent`
    /// but the event keeps its trust bit.
    #[qjs(rename = "__tbDispatchTrusted")]
    fn dispatch_trusted<'js>(
        &self,
        ctx: Ctx<'js>,
        this: This<Object<'js>>,
        event: Class<'js, JsEvent>,
    ) -> Result<bool> {
        register_standalone(&ctx, self.id, &this.0)?;
        dispatch_trusted_event(&ctx, EventTargetKey::Standalone(self.id), &event)
    }
}

/// Wraps the native `EventTarget` constructor so a call without `new` throws
/// (<https://webidl.spec.whatwg.org/#interface-object>); the wrapper shares the
/// native prototype so `Class::<JsEventTarget>` conversions keep working.
pub(crate) const INSTALL_EVENT_TARGET_CTOR_JS: &str = r"
(function() {
  const Native = globalThis.EventTarget;
  function EventTarget() {
    if (new.target === undefined) {
      throw new TypeError('Class constructor EventTarget cannot be invoked without new');
    }
    return Reflect.construct(Native, arguments, new.target);
  }
  Object.defineProperty(EventTarget, 'prototype', { value: Native.prototype, writable: false, configurable: false });
  Object.defineProperty(Native.prototype, 'constructor', { value: EventTarget, writable: true, configurable: true });
  Object.defineProperty(globalThis, 'EventTarget', { value: EventTarget, writable: true, configurable: true });
})();
";

/// `AbortController` and `AbortSignal`
/// (<https://dom.spec.whatwg.org/#interface-abortcontroller>,
/// <https://dom.spec.whatwg.org/#abortsignal>).
pub(crate) const INSTALL_ABORT_JS: &str = r"
(function() {
  const STATE = Symbol('abort-state');
  function createSignal() {
    const signal = Reflect.construct(globalThis.EventTarget, [], AbortSignal);
    signal[STATE] = { aborted: false, reason: undefined };
    return signal;
  }
  function signalAbort(signal, reason) {
    const state = signal[STATE];
    if (!state || state.aborted) {
      return;
    }
    state.aborted = true;
    state.reason = reason !== undefined
      ? reason
      : new DOMException('signal is aborted without reason', 'AbortError');
    signal.dispatchEvent(new Event('abort'));
  }
  function AbortSignal() {
    throw new TypeError('Illegal constructor');
  }
  const signalProto = Object.create(globalThis.EventTarget.prototype);
  Object.defineProperty(signalProto, 'constructor', {
    value: AbortSignal, writable: true, configurable: true,
  });
  Object.defineProperty(signalProto, 'aborted', {
    get: function() { return !!(this[STATE] && this[STATE].aborted); },
    enumerable: true, configurable: true,
  });
  Object.defineProperty(signalProto, 'reason', {
    get: function() { return this[STATE] ? this[STATE].reason : undefined; },
    enumerable: true, configurable: true,
  });
  Object.defineProperty(signalProto, 'throwIfAborted', {
    value: function() {
      if (this[STATE] && this[STATE].aborted) {
        throw this[STATE].reason;
      }
    },
    writable: true, enumerable: true, configurable: true,
  });
  Object.defineProperty(AbortSignal, 'prototype', {
    value: signalProto, writable: false, configurable: false,
  });
  Object.defineProperty(AbortSignal, 'abort', {
    value: function(reason) {
      const signal = createSignal();
      signalAbort(signal, reason);
      return signal;
    },
    writable: true, enumerable: true, configurable: true,
  });
  Object.defineProperty(AbortSignal, 'timeout', {
    value: function(milliseconds) {
      const signal = createSignal();
      globalThis.setTimeout(function() {
        signalAbort(signal, new DOMException('The operation timed out.', 'TimeoutError'));
      }, Number(milliseconds));
      return signal;
    },
    writable: true, enumerable: true, configurable: true,
  });
  function AbortController() {
    if (new.target === undefined) {
      throw new TypeError('Class constructor AbortController cannot be invoked without new');
    }
    const signal = createSignal();
    Object.defineProperty(this, 'signal', {
      get: function() { return signal; },
      enumerable: true, configurable: true,
    });
  }
  Object.defineProperty(AbortController.prototype, 'abort', {
    value: function(reason) { signalAbort(this.signal, reason); },
    writable: true, enumerable: true, configurable: true,
  });
  Object.defineProperty(AbortController.prototype, 'constructor', {
    value: AbortController, writable: true, configurable: true,
  });
  Object.defineProperty(globalThis, 'AbortSignal', {
    value: AbortSignal, writable: true, configurable: true,
  });
  Object.defineProperty(globalThis, 'AbortController', {
    value: AbortController, writable: true, configurable: true,
  });
})();
";

/// Wraps the native `Event` constructor so a call without `new` throws
/// (<https://webidl.spec.whatwg.org/#interface-object>); the wrapper shares the
/// native prototype so `Class::<JsEvent>` conversions keep working.
pub(crate) const INSTALL_EVENT_CTOR_JS: &str = r"
(function() {
  const Native = globalThis.Event;
  const isTrustedGet = Object.getOwnPropertyDescriptor(Native.prototype, 'isTrusted').get;
  function Event() {
    if (new.target === undefined) {
      throw new TypeError('Class constructor Event cannot be invoked without new');
    }
    const event = Reflect.construct(Native, arguments, new.target);
    // [LegacyUnforgeable] own getter
    // (<https://dom.spec.whatwg.org/#dom-event-istrusted>,
    // <https://webidl.spec.whatwg.org/#dfn-unforgeable>).
    Object.defineProperty(event, 'isTrusted', {
      get: isTrustedGet, enumerable: true, configurable: false,
    });
    return event;
  }
  // Constants are `{writable:false, enumerable:true, configurable:false}` on
  // both the interface object and its prototype
  // (<https://webidl.spec.whatwg.org/#define-the-constants>).
  function defineConstant(target, name, value) {
    Object.defineProperty(target, name, { value: value, writable: false, enumerable: true, configurable: false });
  }
  for (const [name, value] of [['NONE', 0], ['CAPTURING_PHASE', 1], ['AT_TARGET', 2], ['BUBBLING_PHASE', 3]]) {
    defineConstant(Event, name, value);
    defineConstant(Native.prototype, name, value);
  }
  Object.defineProperty(Event, 'prototype', { value: Native.prototype, writable: false, configurable: false });
  Object.defineProperty(Native.prototype, 'constructor', { value: Event, writable: true, configurable: true });
  Object.defineProperty(globalThis, 'Event', { value: Event, writable: true, configurable: true });
})();
";

/// Keeps a constructible target's object reachable from its world, so dispatch
/// can use it as `target`/`currentTarget`.
fn register_standalone<'js>(ctx: &Ctx<'js>, id: u64, target: &Object<'js>) -> Result<()> {
    bindings::world(ctx)?
        .borrow_mut()
        .intern_standalone_target(id, Persistent::save(ctx, target.clone()));
    Ok(())
}

/// `document.createEvent(interface)`
/// (<https://dom.spec.whatwg.org/#dom-document-createevent>).
///
/// The interface name matches ASCII case-insensitively.
pub(crate) fn create_event<'js>(ctx: &Ctx<'js>, interface: &str) -> Result<Value<'js>> {
    match interface.to_ascii_lowercase().as_str() {
        "event" | "events" | "htmlevents" => Ok(Class::into_value(Class::instance(
            ctx.clone(),
            JsEvent::uninitialized(),
        )?)),
        "customevent" => {
            let class = Class::instance(ctx.clone(), JsEvent::uninitialized())?;
            set_custom_event_prototype(ctx, &class)?;
            Ok(Class::into_value(class))
        }
        _ => Err(bindings::throw_dom(
            ctx,
            "NotSupportedError",
            "the requested event interface is not supported",
        )),
    }
}

fn set_custom_event_prototype<'js>(ctx: &Ctx<'js>, class: &Class<'js, JsEvent>) -> Result<()> {
    let ctor: Object = ctx.globals().get("CustomEvent")?;
    let proto: Object = ctor.get("prototype")?;
    class.set_prototype(Some(&proto))
}

/// The `new CustomEvent(type, eventInitDict)` constructor, called from the
/// JavaScript wrapper so instances can carry `CustomEvent.prototype`
/// (<https://dom.spec.whatwg.org/#dom-customevent-customevent>).
///
/// The wrapper owns arity; this native owns the Web IDL conversions.
#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
pub(crate) fn construct_custom_event<'js>(
    ctx: Ctx<'js>,
    args: Rest<Value<'js>>,
) -> Result<Value<'js>> {
    let (typ, init) = event_arguments(&ctx, args)?;
    let class = Class::instance(ctx.clone(), event_from_init(&ctx, typ, init.as_ref())?)?;
    set_custom_event_prototype(&ctx, &class)?;
    Ok(Class::into_value(class))
}

/// The initialization steps of `initCustomEvent`, without `detail`
/// (<https://dom.spec.whatwg.org/#dom-customevent-initcustomevent>).
///
/// Returns false when the event is being dispatched, in which case the caller
/// must not set `detail` either.
#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
pub(crate) fn init_custom_event<'js>(ctx: Ctx<'js>, args: Rest<Value<'js>>) -> Result<bool> {
    // Convert the arguments before the algorithm's dispatch-flag check.
    let mut args = args.0.into_iter();
    let Some(event) = args.next() else {
        return Err(Exception::throw_type(&ctx, "event is required"));
    };
    let Some(typ) = args.next() else {
        return Err(Exception::throw_type(&ctx, "type is required"));
    };
    let class = Class::<JsEvent>::from_js(&ctx, event)?;
    let typ = bindings::webidl_to_string(&ctx, typ)?;
    let bubbles = boolean_argument(args.next());
    let cancelable = boolean_argument(args.next());
    if class.borrow().state().dispatching {
        return Ok(false);
    }
    class.borrow().initialize(typ, bubbles, cancelable);
    Ok(true)
}

/// Registers the `CustomEvent` constructor and its prototype chain.
pub(crate) const INSTALL_CUSTOM_EVENT_JS: &str = r"
(function() {
  const TB_DETAIL = Symbol('tb-custom-event-detail');
  function CustomEvent(type) {
    if (new.target === undefined) {
      throw new TypeError('Class constructor CustomEvent cannot be invoked without new');
    }
    const event = globalThis.__tb_new_custom_event.apply(globalThis, arguments);
    // A subclass constructor keeps its own prototype
    // (<https://webidl.spec.whatwg.org/#interface-object>).
    if (new.target !== CustomEvent) {
      Object.setPrototypeOf(event, new.target.prototype);
    }
    // `detail` is read after the EventInit members and defaults to null
    // (<https://dom.spec.whatwg.org/#dictdef-customeventinit>).
    const init = arguments[1];
    const detail = (init === undefined || init === null) ? null : init.detail;
    event[TB_DETAIL] = detail === undefined ? null : detail;
    return event;
  }
  const proto = Object.create(globalThis.Event.prototype);
  Object.defineProperty(proto, 'constructor', { value: CustomEvent, writable: true, configurable: true });
  Object.defineProperty(proto, 'detail', {
    get: function() {
      const detail = this[TB_DETAIL];
      return detail === undefined ? null : detail;
    },
    enumerable: true,
    configurable: true,
  });
  Object.defineProperty(proto, 'initCustomEvent', {
    value: function(type, bubbles, cancelable, detail) {
      if (arguments.length < 1) {
        throw new TypeError('initCustomEvent: at least 1 argument required');
      }
      if (globalThis.__tb_init_custom_event(this, type, bubbles, cancelable)) {
        this[TB_DETAIL] = (arguments.length < 4 || arguments[3] === undefined) ? null : detail;
      }
    },
    writable: true,
    configurable: true,
  });
  Object.defineProperty(CustomEvent, 'prototype', { value: proto, writable: false });
  Object.defineProperty(globalThis, 'CustomEvent', { value: CustomEvent, writable: true, configurable: true });
})();
";

fn event_arguments<'js>(
    ctx: &Ctx<'js>,
    args: Rest<Value<'js>>,
) -> Result<(Value<'js>, Option<Object<'js>>)> {
    let mut args = args.0.into_iter();
    let Some(typ) = args.next() else {
        return Err(Exception::throw_type(
            ctx,
            "1 argument required, but only 0 present",
        ));
    };
    let init = match args.next() {
        None => None,
        Some(value) if value.is_undefined() || value.is_null() => None,
        Some(value) => Some(
            value
                .into_object()
                .ok_or_else(|| Exception::throw_type(ctx, "eventInitDict must be an object"))?,
        ),
    };
    Ok((typ, init))
}

fn event_from_init<'js>(
    ctx: &Ctx<'js>,
    typ: Value<'js>,
    init: Option<&Object<'js>>,
) -> Result<JsEvent> {
    let typ = bindings::webidl_to_string(ctx, typ)?;
    let mut bubbles = false;
    let mut cancelable = false;
    let mut composed = false;
    if let Some(init) = init {
        // `EventInit` member order; `CustomEventInit.detail` is read by the
        // JavaScript wrapper afterwards
        // (<https://dom.spec.whatwg.org/#dictdef-eventinit>).
        bubbles = bindings::option_truthy(init, "bubbles")?;
        cancelable = bindings::option_truthy(init, "cancelable")?;
        composed = bindings::option_truthy(init, "composed")?;
    }
    Ok(JsEvent {
        state: EventStateCell(RefCell::new(EventState {
            typ,
            bubbles,
            cancelable,
            composed,
            initialized: true,
            time_stamp: now_millis(),
            ..EventState::default()
        })),
    })
}

/// Appends one listener to a target's list
/// (<https://dom.spec.whatwg.org/#dom-eventtarget-addeventlistener>).
pub(crate) fn add_listener<'js>(
    ctx: &Ctx<'js>,
    target: EventTargetKey,
    typ: Value<'js>,
    callback: Value<'js>,
    options: Option<Value<'js>>,
) -> Result<()> {
    let typ = bindings::webidl_to_string(ctx, typ)?;
    let callback = listener_callback(ctx, callback)?;
    let options = ListenerOptions::read(ctx, options)?;
    if options
        .signal
        .as_ref()
        .is_some_and(|signal| signal_aborted(ctx, signal))
    {
        return Ok(());
    }
    // A null callback still flattens the options but is never added
    // (<https://dom.spec.whatwg.org/#add-an-event-listener> step 3).
    let Some(callback) = callback else {
        return Ok(());
    };
    let passive = match options.passive {
        Some(passive) => passive,
        None => default_passive(ctx, &typ, target)?,
    };
    let world = target_world(ctx, target)?;
    // Abort steps run when the list is touched or a listener is about to be
    // invoked (<https://dom.spec.whatwg.org/#add-an-event-listener>).
    let existing_listeners = world.borrow().listener_snapshot(target);
    for existing in &existing_listeners {
        if let Some(signal) = &existing.signal
            && signal_aborted(ctx, signal)
        {
            existing.removed.set(true);
            world.borrow_mut().remove_listener(target, existing);
        }
    }
    for existing in &existing_listeners {
        if existing.typ == typ
            && existing.capture == options.capture
            && same_callback(ctx, existing.callback.as_ref(), Some(&callback))?
        {
            return Ok(());
        }
    }
    world.borrow_mut().add_listener(
        target,
        Rc::new(Listener {
            typ,
            callback: Some(callback),
            capture: options.capture,
            once: options.once,
            passive,
            signal: options.signal,
            removed: Cell::new(false),
        }),
    );
    Ok(())
}

/// Removes one listener from a target's list
/// (<https://dom.spec.whatwg.org/#dom-eventtarget-removeeventlistener>).
pub(crate) fn remove_listener<'js>(
    ctx: &Ctx<'js>,
    target: EventTargetKey,
    typ: Value<'js>,
    callback: Value<'js>,
    options: Option<Value<'js>>,
) -> Result<()> {
    let typ = bindings::webidl_to_string(ctx, typ)?;
    let callback = listener_callback(ctx, callback)?;
    let capture = ListenerOptions::read_capture(options)?;
    let world = target_world(ctx, target)?;
    let mut world = world.borrow_mut();
    let mut removed = Vec::new();
    for existing in world.listener_snapshot(target) {
        if existing.typ == typ
            && existing.capture == capture
            && same_callback(ctx, existing.callback.as_ref(), callback.as_ref())?
        {
            existing.removed.set(true);
            removed.push(existing);
        }
    }
    for listener in removed {
        world.remove_listener(target, &listener);
    }
    Ok(())
}

/// `dispatchEvent()`
/// (<https://dom.spec.whatwg.org/#dom-eventtarget-dispatchevent>).
pub(crate) fn dispatch_event<'js>(
    ctx: &Ctx<'js>,
    target: EventTargetKey,
    event: &Class<'js, JsEvent>,
) -> Result<bool> {
    {
        let class = event.borrow();
        let state = class.state();
        if state.dispatching || !state.initialized {
            return Err(bindings::throw_dom(
                ctx,
                "InvalidStateError",
                "the event is already being dispatched or was never initialized",
            ));
        }
    }
    event.borrow().state_mut().is_trusted = false;
    dispatch(ctx, target, event)
}

/// Dispatches a user-agent event with the trust bit set, without the
/// `dispatchEvent()` step that clears `isTrusted`
/// (<https://dom.spec.whatwg.org/#concept-event-dispatch>).
pub(crate) fn dispatch_trusted_event<'js>(
    ctx: &Ctx<'js>,
    target: EventTargetKey,
    event: &Class<'js, JsEvent>,
) -> Result<bool> {
    {
        let class = event.borrow();
        let state = class.state();
        if state.dispatching || !state.initialized {
            return Err(bindings::throw_dom(
                ctx,
                "InvalidStateError",
                "the event is already being dispatched or was never initialized",
            ));
        }
    }
    event.borrow().state_mut().is_trusted = true;
    dispatch(ctx, target, event)
}

/// Creates and dispatches a user-agent event.
pub(crate) fn fire_trusted(
    ctx: &Ctx<'_>,
    target: EventTargetKey,
    typ: &str,
    bubbles: bool,
    cancelable: bool,
) -> Result<()> {
    fire_trusted_with_related(ctx, target, typ, bubbles, cancelable, None)
}

/// Creates and dispatches a trusted event with a `relatedTarget`, as the
/// focus update steps require
/// (<https://html.spec.whatwg.org/multipage/interaction.html#focus-update-steps>).
pub(crate) fn fire_trusted_with_related(
    ctx: &Ctx<'_>,
    target: EventTargetKey,
    typ: &str,
    bubbles: bool,
    cancelable: bool,
    related: Option<EventTargetRef>,
) -> Result<()> {
    let event = Class::instance(ctx.clone(), JsEvent::trusted(typ, bubbles, cancelable))?;
    event.borrow().state_mut().related_target = related;
    dispatch(ctx, target, &event)?;
    Ok(())
}

/// [Dispatch](https://dom.spec.whatwg.org/#concept-event-dispatch) an event.
fn dispatch<'js>(
    ctx: &Ctx<'js>,
    target: EventTargetKey,
    event: &Class<'js, JsEvent>,
) -> Result<bool> {
    let path = build_path(ctx, target)?;
    {
        let class = event.borrow();
        let mut state = class.state_mut();
        state.dispatching = true;
        state.target = Some(EventTargetRef {
            key: target,
            world: Rc::clone(&path[0].reference.world),
        });
        state.current_target = None;
        state.phase = NONE;
        // The stop flags are intentionally not cleared here: an event that was
        // stopped before dispatch must not reach any listener, and the end of
        // the algorithm unsets the flags
        // (<https://dom.spec.whatwg.org/#concept-event-dispatch>).
        state.path = path.iter().map(|item| item.reference.clone()).collect();
    }
    let bubbles = event.borrow().state().bubbles;
    let typ = event.borrow().state().typ.clone();
    let window = current_event_window(ctx, &path)?;
    // https://html.spec.whatwg.org/multipage/webappapis.html#set-the-current-event
    let previous: Value<'js> = window.get("event")?;
    window.set("event", Class::into_value(event.clone()))?;
    let result = run_invocations(ctx, event, &path, target, bubbles);
    // Handler attributes (`onreadystatechange`, `onload`, …) act as listeners;
    // the engine runs them after the listener list until it models the
    // handler registration slot.
    let handler = if result.is_ok() {
        let event_value = Class::into_value(event.clone());
        call_handler_attribute(
            ctx,
            path.first().map(|item| &item.target),
            &typ,
            &event_value,
        )
    } else {
        Ok(())
    };
    // Clear the dispatch state on both paths, so a failed invocation cannot
    // leave the event permanently undispatchable.
    let canceled = {
        let class = event.borrow();
        let mut state = class.state_mut();
        state.phase = NONE;
        state.current_target = None;
        state.path.clear();
        state.dispatching = false;
        state.stop_propagation = false;
        state.stop_immediate = false;
        state.canceled
    };
    let restored = window.set("event", previous);
    result?;
    handler?;
    restored?;
    Ok(!canceled)
}

/// The `Window` whose [current event](https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-window-event)
/// this dispatch updates: the target's realm window, else the caller's.
fn current_event_window<'js>(ctx: &Ctx<'js>, path: &[PathItem<'js>]) -> Result<Object<'js>> {
    if let Some(item) = path.first()
        && let Some(window) = item.reference.world.borrow().window_object()
    {
        return window.restore(ctx);
    }
    Ok(ctx.globals())
}

fn run_invocations<'js>(
    ctx: &Ctx<'js>,
    event: &Class<'js, JsEvent>,
    path: &[PathItem<'js>],
    target: EventTargetKey,
    bubbles: bool,
) -> Result<()> {
    // Capture phase: root to target.
    for item in path.iter().rev() {
        let phase = if item.reference.key == target {
            AT_TARGET
        } else {
            CAPTURING_PHASE
        };
        invoke(ctx, event, item, phase, true)?;
    }
    // Bubble phase: target to root.
    for item in path {
        let phase = if item.reference.key == target {
            AT_TARGET
        } else if bubbles {
            BUBBLING_PHASE
        } else {
            continue;
        };
        invoke(ctx, event, item, phase, false)?;
    }
    Ok(())
}

/// One event path item (<https://dom.spec.whatwg.org/#event-path-item>).
struct PathItem<'js> {
    reference: EventTargetRef,
    target: Value<'js>,
}

/// Builds the event path: the target, its ancestors, and the window
/// (<https://dom.spec.whatwg.org/#concept-event-path-append>).
fn build_path<'js>(ctx: &Ctx<'js>, target: EventTargetKey) -> Result<Vec<PathItem<'js>>> {
    match target {
        EventTargetKey::Window => {
            let reference = EventTargetRef {
                key: target,
                world: bindings::world(ctx)?,
            };
            let value = resolve_target(ctx, &reference)?;
            Ok(vec![PathItem {
                reference,
                target: value,
            }])
        }
        EventTargetKey::Standalone(id) => {
            let reference = EventTargetRef {
                key: target,
                world: bindings::world(ctx)?,
            };
            if reference.world.borrow().standalone_target(id).is_none() {
                return Err(Exception::throw_internal(ctx, "missing EventTarget"));
            }
            let value = resolve_target(ctx, &reference)?;
            Ok(vec![PathItem {
                reference,
                target: value,
            }])
        }
        EventTargetKey::Node(id) => {
            let world = bindings::world_for_node(ctx, id)?;
            let mut path = Vec::new();
            let mut cursor = Some(id);
            let mut reached_document = false;
            while let Some(node) = cursor {
                let reference = EventTargetRef {
                    key: EventTargetKey::Node(node),
                    world: Rc::clone(&world),
                };
                let value = resolve_target(ctx, &reference)?;
                path.push(PathItem {
                    reference,
                    target: value,
                });
                let parent = world.borrow().node_parent(node);
                if let Some(parent) = parent {
                    cursor = Some(parent);
                } else {
                    reached_document = world.borrow().node_is_document(node);
                    cursor = None;
                }
            }
            if reached_document {
                // A document's parent is its own window, not the dispatching
                // realm's (<https://dom.spec.whatwg.org/#get-the-parent>).
                let reference = EventTargetRef {
                    key: EventTargetKey::Window,
                    world,
                };
                let value = resolve_target(ctx, &reference)?;
                path.push(PathItem {
                    reference,
                    target: value,
                });
            }
            Ok(path)
        }
    }
}

/// [Invoke](https://dom.spec.whatwg.org/#concept-event-listener-invoke) the
/// listeners of one path item.
fn invoke<'js>(
    ctx: &Ctx<'js>,
    event: &Class<'js, JsEvent>,
    item: &PathItem<'js>,
    phase: u16,
    capturing: bool,
) -> Result<()> {
    {
        let class = event.borrow();
        if class.state().stop_propagation {
            return Ok(());
        }
        let mut state = class.state_mut();
        state.phase = phase;
        state.current_target = Some(item.reference.clone());
    }
    let listeners = item
        .reference
        .world
        .borrow()
        .listener_snapshot(item.reference.key);
    let event_value = Class::into_value(event.clone());
    for listener in listeners {
        if listener.removed.get() {
            continue;
        }
        let typ = event.borrow().state().typ.clone();
        if typ != listener.typ {
            continue;
        }
        if capturing != listener.capture {
            continue;
        }
        if let Some(signal) = &listener.signal
            && signal_aborted(ctx, signal)
        {
            listener.removed.set(true);
            item.reference
                .world
                .borrow_mut()
                .remove_listener(item.reference.key, &listener);
            continue;
        }
        if listener.once {
            listener.removed.set(true);
            item.reference
                .world
                .borrow_mut()
                .remove_listener(item.reference.key, &listener);
        }
        if listener.passive {
            event.borrow().state_mut().in_passive = true;
        }
        if let Some(callback) = &listener.callback {
            let value: Value<'js> = callback.clone().restore(ctx)?;
            if let Err(error) = call_listener(ctx, &value, &item.target, &event_value) {
                report_exception(ctx, &error);
            }
        }
        event.borrow().state_mut().in_passive = false;
        if event.borrow().state().stop_immediate {
            break;
        }
    }
    Ok(())
}

/// The Web IDL "call a user object's operation" step for an event listener:
/// a callable callback is called directly, any other object through its
/// `handleEvent` operation (<https://webidl.spec.whatwg.org/#call-a-user-objects-operation>).
fn call_listener<'js>(
    ctx: &Ctx<'js>,
    callback: &Value<'js>,
    this: &Value<'js>,
    event: &Value<'js>,
) -> Result<()> {
    if let Some(function) = callback.as_function() {
        function.call::<_, ()>((This(this.clone()), event.clone()))?;
        return Ok(());
    }
    if let Some(object) = callback.as_object() {
        let handler: Value = object.get("handleEvent")?;
        if let Some(function) = handler.as_function() {
            function.call::<_, ()>((This(object.clone()), event.clone()))?;
            return Ok(());
        }
    }
    Err(Exception::throw_type(ctx, "callback is not callable"))
}

/// [Report the exception](https://html.spec.whatwg.org/multipage/webappapis.html#report-an-exception)
/// step of the dispatch algorithm.
///
/// The engine has no `ErrorEvent`/`window.onerror` plumbing yet, so the
/// exception is dropped. Dropping it keeps a throwing listener from aborting
/// the rest of the dispatch, and consuming it keeps a later JavaScript
/// operation from observing the stale pending exception.
fn report_exception(ctx: &Ctx<'_>, error: &rquickjs::Error) {
    if error.is_exception() {
        let _caught = ctx.catch();
    }
}

/// `EventListener?` conversion: null and undefined become a null callback and
/// any object is a callback interface value, whose `handleEvent` is looked up
/// when invoked (<https://webidl.spec.whatwg.org/#es-callback-interface>).
fn listener_callback<'js>(
    ctx: &Ctx<'js>,
    callback: Value<'js>,
) -> Result<Option<Persistent<Value<'static>>>> {
    if callback.is_null() || callback.is_undefined() {
        return Ok(None);
    }
    if !callback.is_object() {
        return Err(Exception::throw_type(
            ctx,
            "callback must be an object or null",
        ));
    }
    Ok(Some(Persistent::save(ctx, callback)))
}

/// Parsed `AddEventListenerOptions` / `EventListenerOptions`
/// (<https://dom.spec.whatwg.org/#dictdef-addeventlisteneroptions>).
struct ListenerOptions {
    capture: bool,
    once: bool,
    /// `None` means the member was omitted; the default passive value is
    /// resolved when the listener is added
    /// (<https://dom.spec.whatwg.org/#default-passive-value>).
    passive: Option<bool>,
    signal: Option<Persistent<Object<'static>>>,
}

impl ListenerOptions {
    fn read<'js>(ctx: &Ctx<'js>, options: Option<Value<'js>>) -> Result<Self> {
        let mut parsed = Self {
            capture: false,
            once: false,
            passive: None,
            signal: None,
        };
        let Some(value) = options else {
            return Ok(parsed);
        };
        if value.is_undefined() || value.is_null() {
            return Ok(parsed);
        }
        let Some(object) = value.as_object() else {
            // A non-object is the boolean form.
            parsed.capture = bindings::to_boolean(&value);
            return Ok(parsed);
        };
        parsed.capture = bindings::option_truthy(object, "capture")?;
        parsed.once = bindings::option_truthy(object, "once")?;
        let passive: Value = object.get("passive")?;
        if !passive.is_undefined() {
            parsed.passive = Some(bindings::to_boolean(&passive));
        }
        let signal: Value = object.get("signal")?;
        if !signal.is_undefined() {
            // `AbortSignal signal` is not nullable
            // (<https://dom.spec.whatwg.org/#dictdef-addeventlisteneroptions>,
            // <https://webidl.spec.whatwg.org/#es-interface>).
            let Some(signal) = signal.as_object() else {
                return Err(Exception::throw_type(
                    ctx,
                    "Failed to convert 'signal' to AbortSignal",
                ));
            };
            parsed.signal = Some(Persistent::save(ctx, signal.clone()));
        }
        Ok(parsed)
    }

    fn read_capture(options: Option<Value<'_>>) -> Result<bool> {
        // `removeEventListener` reads only `capture`; reading the other
        // members would run their getters
        // (<https://dom.spec.whatwg.org/#dom-eventtarget-removeeventlistener>).
        let Some(value) = options else {
            return Ok(false);
        };
        if value.is_undefined() || value.is_null() {
            return Ok(false);
        }
        let Some(object) = value.as_object() else {
            return Ok(bindings::to_boolean(&value));
        };
        bindings::option_truthy(object, "capture")
    }
}

/// The default passive value for an event type on a target
/// (<https://dom.spec.whatwg.org/#default-passive-value>): wheel and
/// touch-start listeners are passive on the window, the document, its
/// document element, and its body element.
fn default_passive(ctx: &Ctx<'_>, typ: &str, target: EventTargetKey) -> Result<bool> {
    if !matches!(typ, "touchstart" | "touchmove" | "wheel" | "mousewheel") {
        return Ok(false);
    }
    match target {
        EventTargetKey::Window => Ok(true),
        EventTargetKey::Standalone(_) => Ok(false),
        EventTargetKey::Node(id) => {
            let world = bindings::world_for_node(ctx, id)?;
            let world = world.borrow();
            let Some(parsed) = world.document(id) else {
                return Ok(false);
            };
            let dom = &parsed.dom;
            if dom.document() == id {
                return Ok(true);
            }
            let html = dom.select_first(dom.document(), "html").ok().flatten();
            if html == Some(id) {
                return Ok(true);
            }
            let body = dom.select_first(dom.document(), "body").ok().flatten();
            Ok(body == Some(id))
        }
    }
}

/// Whether an `AbortSignal` has its aborted flag set
/// (<https://dom.spec.whatwg.org/#abortsignal-aborted>).
fn signal_aborted(ctx: &Ctx<'_>, signal: &Persistent<Object<'static>>) -> bool {
    signal
        .clone()
        .restore(ctx)
        .ok()
        .and_then(|object| object.get::<_, Value>("aborted").ok())
        .is_some_and(|value| bindings::to_boolean(&value))
}

fn target_world(ctx: &Ctx<'_>, target: EventTargetKey) -> Result<Rc<RefCell<World>>> {
    match target {
        EventTargetKey::Node(id) => bindings::world_for_node(ctx, id),
        EventTargetKey::Window | EventTargetKey::Standalone(_) => bindings::world(ctx),
    }
}

fn stored_target<'js>(ctx: &Ctx<'js>, reference: Option<&EventTargetRef>) -> Result<Value<'js>> {
    match reference {
        Some(reference) => resolve_target(ctx, reference),
        None => Ok(Value::new_null(ctx.clone())),
    }
}

/// Resolves an event target back to its platform object, using the world that
/// owns it rather than the caller's
/// (<https://dom.spec.whatwg.org/#concept-event-target>).
fn resolve_target<'js>(ctx: &Ctx<'js>, reference: &EventTargetRef) -> Result<Value<'js>> {
    match reference.key {
        EventTargetKey::Window => match reference.world.borrow().window_object() {
            Some(window) => Ok(window.restore(ctx)?.into_value()),
            None => Ok(ctx.globals().into_value()),
        },
        EventTargetKey::Node(id) => bindings::wrap_node(ctx, id),
        EventTargetKey::Standalone(id) => match reference.world.borrow().standalone_target(id) {
            Some(saved) => Ok(saved.restore(ctx)?.into_value()),
            None => Ok(Value::new_null(ctx.clone())),
        },
    }
}

/// Calls a target's `on<type>` handler attribute, if one is assigned
/// (<https://html.spec.whatwg.org/multipage/webappapis.html#event-handlers>).
///
/// The engine does not model the handler registration slot yet, so this runs
/// after the listener list instead of at the handler's registration position.
fn call_handler_attribute<'js>(
    ctx: &Ctx<'js>,
    target: Option<&Value<'js>>,
    typ: &str,
    event: &Value<'js>,
) -> Result<()> {
    let Some(object) = target.and_then(|target| target.as_object()) else {
        return Ok(());
    };
    let name = format!("on{typ}");
    let handler: Value = object.get(name.as_str())?;
    if let Some(function) = handler.as_function()
        && let Err(error) = function.call::<_, ()>((This(object.clone()), event.clone()))
    {
        report_exception(ctx, &error);
    }
    Ok(())
}

/// `ToBoolean` for an optional argument; a missing or undefined argument is
/// false (<https://webidl.spec.whatwg.org/#es-boolean>).
fn boolean_argument(value: Option<Value<'_>>) -> bool {
    match value {
        Some(value) if !value.is_undefined() => bindings::to_boolean(&value),
        _ => false,
    }
}

fn same_callback(
    ctx: &Ctx<'_>,
    stored: Option<&Persistent<Value<'static>>>,
    live: Option<&Persistent<Value<'static>>>,
) -> Result<bool> {
    match (stored, live) {
        (None, None) => Ok(true),
        (Some(stored), Some(live)) => {
            let stored = stored.clone().restore(ctx)?;
            let live = live.clone().restore(ctx)?;
            Ok(stored == live)
        }
        _ => Ok(false),
    }
}
