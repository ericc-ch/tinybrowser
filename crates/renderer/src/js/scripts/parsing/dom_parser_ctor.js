// DOMParser wrapper
// (<https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-domparser-parsefromstring>).
//
// The native class cannot know which realm constructed an instance once the
// method is invoked from another realm, and parsed documents must take the
// constructing realm's URL. This wrapper captures `document.URL` in the
// constructor — running in the constructor's realm — and hands it to the
// native method.
(function() {
  const Native = globalThis.DOMParser;
  class DOMParser {
    constructor() {
      Object.defineProperty(this, '__tbParser', { value: new Native() });
      Object.defineProperty(this, '__tbUrl', { value: globalThis.document.URL });
    }
    parseFromString(source, type) {
      return this.__tbParser.parseFromString(source, type, this.__tbUrl);
    }
  }
  Object.defineProperty(globalThis, 'DOMParser', {
    value: DOMParser, writable: true, configurable: true,
  });
})();
