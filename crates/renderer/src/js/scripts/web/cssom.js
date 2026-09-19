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
      // https://drafts.csswg.org/cssom/#dom-css-escape
      escape(value) {
        if (arguments.length < 1) throw new TypeError('CSS.escape requires 1 argument');
        const string = String(value);
        let result = '';
        for (let index = 0; index < string.length; index++) {
          const code = string.charCodeAt(index);
          if (code === 0) {
            result += '\uFFFD';
            continue;
          }
          if (
            (code >= 1 && code <= 0x1F) || code === 0x7F ||
            (index === 0 && code >= 0x30 && code <= 0x39) ||
            (index === 1 && code >= 0x30 && code <= 0x39 && string.charCodeAt(0) === 0x2D)
          ) {
            result += `\\${code.toString(16)} `;
            continue;
          }
          if (
            code >= 0x80 || code === 0x2D || code === 0x5F ||
            (code >= 0x30 && code <= 0x39) ||
            (code >= 0x41 && code <= 0x5A) ||
            (code >= 0x61 && code <= 0x7A)
          ) {
            result += string[index];
            continue;
          }
          result += `\\${string[index]}`;
        }
        return result;
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
