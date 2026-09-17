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
