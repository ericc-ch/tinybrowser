// DOMParser wrapper
// (<https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-domparser-parsefromstring>).
//
// The native instance remembers its constructing realm's URL, so parsed
// documents take that URL even when the method runs in another realm. This
// wrapper only exists to keep `new.target` branding and argument validation
// on the JS side.
(function() {
  const Native = globalThis.DOMParser;
  class DOMParser {
    constructor() {
      Object.defineProperty(this, '__tbParser', { value: new Native() });
    }
    parseFromString(source, type) {
      return this.__tbParser.parseFromString(source, type);
    }
  }
  Object.defineProperty(globalThis, 'DOMParser', {
    value: DOMParser, writable: true, configurable: true,
  });
})();
