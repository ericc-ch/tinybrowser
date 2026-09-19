// Constructable stylesheet surface used by component libraries.
// https://drafts.csswg.org/cssom/#the-cssstylesheet-interface
{
  class CSSStyleSheet {
    constructor() {
      this.cssRules = [];
      this._text = '';
    }
    replaceSync(text) {
      this._text = String(text);
      this.cssRules = this._text ? [{ cssText: this._text }] : [];
    }
    replace(text) {
      this.replaceSync(text);
      return Promise.resolve(this);
    }
  }
  Object.defineProperty(globalThis, 'CSSStyleSheet', {
    value: CSSStyleSheet, writable: true, configurable: true,
  });
  Object.defineProperty(globalThis, 'CSS', {
    value: Object.freeze({
      escape(value) {
        return String(value).replace(/[^a-zA-Z0-9_-]/g, character =>
          `\\${character.codePointAt(0).toString(16)} `);
      },
      supports() { return false; },
    }),
    writable: true, configurable: true,
  });
  Object.defineProperty(globalThis, 'getComputedStyle', {
    value: function(element) {
      const style = element?.style ?? {};
      if (typeof style.getPropertyValue !== 'function') {
        style.getPropertyValue = function(name) { return this[name] ?? ''; };
      }
      return style;
    },
    writable: true, configurable: true,
  });
  Object.defineProperty(globalThis, 'matchMedia', {
    value: function(media) {
      return {
        media: String(media), matches: false, onchange: null,
        addEventListener() {}, removeEventListener() {},
        addListener() {}, removeListener() {}, dispatchEvent() { return true; },
      };
    },
    writable: true, configurable: true,
  });
  Object.defineProperty(HTMLElement.prototype, 'scrollTo', {
    value: function() {}, writable: true, configurable: true, enumerable: true,
  });
}
