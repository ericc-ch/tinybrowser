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
