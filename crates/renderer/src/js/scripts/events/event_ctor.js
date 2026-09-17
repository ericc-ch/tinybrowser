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
