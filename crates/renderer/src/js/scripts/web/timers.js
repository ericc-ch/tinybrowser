// Unforgeable host token, captured before install deletes the global. Our
// shims pass it to the trusted-event bridge; page script cannot name it.
const __tbHostToken = globalThis.__tbHostToken;
globalThis.__tb_timeouts = [];
globalThis.__tb_fetchCbs = Object.create(null);
globalThis.__tb_fetchSeq = 0;
['__scheduleTimeout','__cancelTimeout','__queueFetch','__cookieGet','__cookieSet','__tbCreateObjectURL','__tbRevokeObjectURL','__tbResolveUrl','__tbParseUrl','__tb_timeouts','__tb_fetchCbs'].forEach(function(k) {
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
// The engine has no rendering pipeline; a frame callback is a 16ms timer
// (<https://html.spec.whatwg.org/multipage/imagebitmap-and-animations.html#dom-animationframeprovider-requestanimationframe>).
// Deviations: callbacks queued in the same frame do not share a
// DOMHighResTimeStamp (each gets `Date.now()`), handles share the timer
// table with the +1 offset, and `cancelAnimationFrame` of a plain
// `setTimeout` handle cancels that timer.
globalThis.requestAnimationFrame = function(fn) {
  if (typeof fn !== 'function') {
    throw new TypeError('requestAnimationFrame requires a callback');
  }
  return globalThis.setTimeout(function() {
    fn(Date.now());
  }, 16) + 1;
};
globalThis.cancelAnimationFrame = function(id) {
  globalThis.clearTimeout(Number(id) - 1);
};
if (!globalThis.document) {
  globalThis.document = {};
}
// A no-op console: enough for scripts that only log, without a logging pipe.
if (typeof globalThis.console === 'undefined') {
  const noop = function() {};
  globalThis.console = {
    log: noop, info: noop, warn: noop, error: noop, debug: noop,
    trace: noop, dir: noop, group: noop, groupEnd: noop, table: noop,
    assert: noop, time: noop, timeEnd: noop, count: noop,
  };
}
Object.defineProperty(document, 'cookie', {
  get() { return globalThis.__cookieGet(); },
  set(v) { globalThis.__cookieSet(String(v)); }
});
