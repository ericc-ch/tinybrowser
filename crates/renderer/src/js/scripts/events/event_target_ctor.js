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
