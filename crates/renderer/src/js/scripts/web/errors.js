// Uncaught-error plumbing: the `ErrorEvent` interface and the bridge the host
// calls when a script or an event handler throws
// (<https://html.spec.whatwg.org/multipage/webappapis.html#report-the-error>).
(function() {
  const hostToken = globalThis.__tbHostToken;
  const MESSAGE = Symbol('tb-error-message');
  const FILENAME = Symbol('tb-error-filename');
  const LINENO = Symbol('tb-error-lineno');
  const COLNO = Symbol('tb-error-colno');
  const ERROR = Symbol('tb-error-error');

  // `ErrorEventInit` members beyond `EventInit`
  // (<https://html.spec.whatwg.org/multipage/webappapis.html#erroreventinit>).
  function ErrorEvent(type) {
    if (new.target === undefined) {
      throw new TypeError('Class constructor ErrorEvent cannot be invoked without new');
    }
    const event = Reflect.construct(globalThis.Event, arguments, new.target);
    const init = arguments[1];
    const dictionary = (init === undefined || init === null) ? {} : init;
    event[MESSAGE] = dictionary.message === undefined ? '' : String(dictionary.message);
    event[FILENAME] = dictionary.filename === undefined ? '' : String(dictionary.filename);
    event[LINENO] = dictionary.lineno === undefined ? 0 : (dictionary.lineno >>> 0);
    event[COLNO] = dictionary.colno === undefined ? 0 : (dictionary.colno >>> 0);
    event[ERROR] = dictionary.error === undefined ? null : dictionary.error;
    return event;
  }

  const proto = Object.create(globalThis.Event.prototype);
  Object.defineProperty(proto, 'constructor', {
    value: ErrorEvent, writable: true, configurable: true,
  });
  const member = (name, symbol, fallback) => {
    Object.defineProperty(proto, name, {
      get() {
        const value = this[symbol];
        return value === undefined ? fallback : value;
      },
      enumerable: true,
      configurable: true,
    });
  };
  member('message', MESSAGE, '');
  member('filename', FILENAME, '');
  member('lineno', LINENO, 0);
  member('colno', COLNO, 0);
  member('error', ERROR, null);
  Object.defineProperty(proto, Symbol.toStringTag, {
    value: 'ErrorEvent', writable: false, enumerable: false, configurable: true,
  });
  Object.defineProperty(ErrorEvent, 'prototype', { value: proto, writable: false });
  Object.defineProperty(globalThis, 'ErrorEvent', {
    value: ErrorEvent, writable: true, configurable: true,
  });

  // The first `file:line:column` a QuickJS stack frame ends with, script-relative.
  const locationOf = stack => {
    if (typeof stack !== 'string') return null;
    for (const frame of stack.split('\n')) {
      const match = /(\d+):(\d+)\)?\s*$/.exec(frame);
      if (match !== null) {
        return { line: Number(match[1]), column: Number(match[2]) };
      }
    }
    return null;
  };

  // One report at a time: an exception thrown by the `onerror` handler itself
  // is not reported back into `onerror` again.
  let reporting = false;

  globalThis.__tbReportException = function(caught, meta) {
    if (reporting) return false;
    reporting = true;
    try {
      const isObject =
        caught !== null && (typeof caught === 'object' || typeof caught === 'function');
      let message = '';
      let error = null;
      let stack = '';
      if (isObject) {
        message = caught.message === undefined ? String(caught) : String(caught.message);
        error = caught;
        if (typeof caught.stack === 'string') stack = caught.stack;
      } else {
        message = String(caught);
      }
      const location = locationOf(stack);
      const baseLine = meta !== null && typeof meta === 'object' &&
        typeof meta.baseLine === 'number' ? meta.baseLine : 0;
      const filename = meta !== null && typeof meta === 'object' &&
        typeof meta.filename === 'string' && meta.filename !== ''
        ? meta.filename
        : String(globalThis.location === undefined ? '' : globalThis.location.href);
      const event = new ErrorEvent('error', {
        message: message,
        filename: filename,
        lineno: location === null ? baseLine : baseLine + location.line - 1,
        colno: location === null ? 0 : location.column,
        error: error,
        cancelable: true,
      });
      globalThis.__tbDispatchTrusted(hostToken, event);
      return event.defaultPrevented;
    } finally {
      reporting = false;
    }
  };
  Object.defineProperty(globalThis, '__tbReportException', {
    writable: false, configurable: false, enumerable: false,
  });
})();
