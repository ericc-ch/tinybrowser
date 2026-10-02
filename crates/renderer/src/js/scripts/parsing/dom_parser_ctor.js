// DOMParser wrapper
// (<https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-domparser-parsefromstring>).
//
// The native instance remembers its constructing realm's URL, so parsed
// documents take that URL even when the method runs in another realm. This
// wrapper only exists to keep `new.target` branding and argument validation
// on the JS side.
(function() {
  const Native = globalThis.DOMParser;
  const parsers = host.slots('DOMParser');
  class DOMParser {
    constructor() {
      parsers.set(this, new Native());
    }
    parseFromString(source, type) {
      const parser = parsers.get(this);
      if (parser === undefined) throw new TypeError('Illegal invocation');
      return parser.parseFromString(source, type);
    }
  }
  Object.defineProperty(globalThis, 'DOMParser', {
    value: DOMParser, writable: true, configurable: true,
  });
})();
