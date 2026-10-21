// https://drafts.csswg.org/css-font-loading/#fontfaceset
// System fonts are always available, so the document's font set is settled
// from the start. Async tests commonly await `document.fonts.ready` before
// measuring or drawing canvas text.
const __tbFontFaceSet = {
  status: 'loaded',
  ready: null,
  onloading: null,
  onloadingdone: null,
  onloadingerror: null,
  check() { return true; },
  load() { return Promise.resolve([]); },
  add() {},
  delete() { return false; },
  clear() {},
  forEach() {},
};
__tbFontFaceSet.ready = Promise.resolve(__tbFontFaceSet);

if (globalThis.document !== undefined) {
  const prototype = Object.getPrototypeOf(document);
  // A realm can exist before its document is installed; timers.js then uses
  // a plain {} placeholder. Never define `fonts` on Object.prototype.
  if (prototype !== Object.prototype) {
    Object.defineProperty(prototype, 'fonts', {
      configurable: true,
      enumerable: false,
      get() { return __tbFontFaceSet; },
    });
  }
}
