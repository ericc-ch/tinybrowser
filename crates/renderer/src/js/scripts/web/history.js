// https://html.spec.whatwg.org/multipage/nav-history-apis.html#the-history-interface
(function() {
  const initialState = globalThis.__tbHistoryState;
  let currentState = initialState == null ? null : __tbDecode(initialState, Object.create(null));
  let length = globalThis.__tbHistoryLength;
  function updateLocation(url) {
    const parsed = new URL(url);
    const location = globalThis.location;
    location.href = parsed.href;
    location.pathname = parsed.pathname;
    location.search = parsed.search;
    location.hash = parsed.hash;
  }
  globalThis.PopStateEvent = class PopStateEvent extends Event {
    constructor(type, init) {
      super(type, init);
      this.state = init == null || init.state === undefined ? null : init.state;
    }
  };
  globalThis.__tbHistoryRestore = function(url, payload, count) {
    currentState = payload == null ? null : __tbDecode(payload, Object.create(null));
    length = count;
    updateLocation(url);
    return currentState;
  };
  globalThis.history = {
    get length() { return length; },
    get state() { return currentState; },
    scrollRestoration: 'auto',
    pushState(data, unused, url) { update(data, url, false); },
    replaceState(data, unused, url) { update(data, url, true); },
    go(delta = 0) { globalThis.__tbHistoryTraverse(Number(delta) | 0); },
    back() { this.go(-1); },
    forward() { this.go(1); },
  };
  function update(data, url, replace) {
    // Serialize first; neither the URL nor the current entry changes when
    // serialization throws.
    // <https://html.spec.whatwg.org/multipage/nav-history-apis.html#shared-history-push/replace-state-steps>
    const serialized = __tbEncode(data, undefined, null).payload;
    const next = url === undefined || url === null || String(url) === ''
      ? String(globalThis.location.href)
      : new URL(String(url), globalThis.document.baseURI).href;
    const current = new URL(globalThis.location.href);
    const target = new URL(next);
    if (target.origin !== current.origin) {
      throw new DOMException('History URL must be same-origin', 'SecurityError');
    }
    length = globalThis.__tbHistoryUpdate(next, serialized, replace);
    currentState = __tbDecode(serialized, Object.create(null));
    updateLocation(next);
  }
  Object.defineProperty(globalThis.history, Symbol.toStringTag, { value: 'History' });
})();
