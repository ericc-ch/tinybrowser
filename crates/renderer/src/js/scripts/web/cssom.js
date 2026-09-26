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
  // A computed style always resolves: an unset property reports its initial
  // value, not the empty string
  // (<https://drafts.csswg.org/cssom/#resolved-values>). The engine has no
  // full cascade in script yet, so this covers the initial values clients
  // read (visibility for actionability, boxes for geometry).
  const __tbInitialValues = {
    visibility: 'visible',
    display: 'inline',
    cursor: 'auto',
    transform: 'none',
    'transform-origin': '50% 50%',
    'border-left-width': '0px',
    'border-top-width': '0px',
    'border-right-width': '0px',
    'border-bottom-width': '0px',
    'border-left-style': 'none',
    'border-top-style': 'none',
    'border-right-style': 'none',
    'border-bottom-style': 'none',
    'padding-left': '0px',
    'padding-top': '0px',
    'padding-right': '0px',
    'padding-bottom': '0px',
    'margin-left': '0px',
    'margin-top': '0px',
    'margin-right': '0px',
    'margin-bottom': '0px',
    color: 'rgb(0, 0, 0)',
    'background-color': 'rgba(0, 0, 0, 0)',
    'font-size': '16px',
    'font-weight': '400',
    'font-family': 'serif',
    'line-height': 'normal',
    width: 'auto',
    height: 'auto',
    'box-sizing': 'content-box',
    position: 'static',
    overflow: 'visible',
    opacity: '1',
    'z-index': 'auto',
    'text-align': 'start',
    'white-space': 'normal',
  };
  Object.defineProperty(globalThis, 'getComputedStyle', {
    value: function(element) {
      const inline = element?.style ?? {};
      if (typeof inline.getPropertyValue !== 'function') {
        inline.getPropertyValue = function(name) { return this[name] ?? ''; };
      }
      return new Proxy(inline, {
        get(target, property, receiver) {
          if (typeof property !== 'string') return Reflect.get(target, property, receiver);
          if (property === 'getPropertyValue') {
            return name => {
              const value = target.getPropertyValue(name);
              if (value !== '') return value;
              return __tbInitialValues[String(name).toLowerCase()] ?? '';
            };
          }
          const value = Reflect.get(target, property, receiver);
          if (value !== undefined && value !== null && value !== '') return value;
          const kebab = property.replace(/[A-Z]/g, letter => '-' + letter.toLowerCase());
          if (kebab in __tbInitialValues) return __tbInitialValues[kebab];
          return value;
        },
      });
    },
    writable: true, configurable: true,
  });
  // Viewport and color-scheme queries used by pages and by CSSOM tests.
  // Color scheme must match Stylo's Device `PrefersColorScheme::Dark`.
  // https://drafts.csswg.org/cssom-view/#dom-window-matchmedia
  // https://drafts.csswg.org/mediaqueries-5/#mq-syntax
  const __tbColorScheme = 'dark';
  const __tbSplitMedia = (text, separator) => {
    const parts = [];
    let current = '';
    let depth = 0;
    for (let index = 0; index < text.length; index++) {
      const ch = text[index];
      if (ch === '(') depth++;
      if (ch === ')') depth = Math.max(0, depth - 1);
      if (depth === 0 && text.slice(index, index + separator.length).toLowerCase() === separator) {
        parts.push(current);
        current = '';
        index += separator.length - 1;
        continue;
      }
      current += ch;
    }
    parts.push(current);
    return parts;
  };
  const __tbPx = (raw) => {
    const value = String(raw).trim().toLowerCase();
    if (value.endsWith('px')) return Number(value.slice(0, -2));
    if (value.endsWith('em')) return Number(value.slice(0, -2)) * 16;
    return Number(value);
  };
  const __tbFeature = (raw) => {
    const inner = raw.trim().replace(/^\(/, '').replace(/\)$/, '').trim();
    if (!inner) return 'unknown';
    const colon = inner.indexOf(':');
    const name = (colon < 0 ? inner : inner.slice(0, colon)).trim().toLowerCase();
    const value = colon < 0 ? '' : inner.slice(colon + 1).trim().toLowerCase();
    if (name === 'prefers-color-scheme') {
      if (value === '') return true;
      if (value === 'dark' || value === 'light') return value === __tbColorScheme;
      if (value === 'no-preference') return false;
      return 'unknown';
    }
    const width = Number(globalThis.innerWidth);
    const height = Number(globalThis.innerHeight);
    if (name === 'width' || name === 'min-width' || name === 'max-width') {
      const px = __tbPx(value);
      if (!Number.isFinite(px)) return 'unknown';
      if (name === 'min-width') return width >= px;
      if (name === 'max-width') return width <= px;
      return width === px;
    }
    if (name === 'height' || name === 'min-height' || name === 'max-height') {
      const px = __tbPx(value);
      if (!Number.isFinite(px)) return 'unknown';
      if (name === 'min-height') return height >= px;
      if (name === 'max-height') return height <= px;
      return height === px;
    }
    if (name === 'hover') {
      if (value === '' || value === 'hover') return true;
      if (value === 'none') return false;
      return 'unknown';
    }
    if (name === 'pointer') {
      if (value === '' || value === 'fine') return true;
      if (value === 'coarse' || value === 'none') return false;
      return 'unknown';
    }
    return 'unknown';
  };
  const __tbMediaQuery = (text) => {
    text = String(text).trim().toLowerCase();
    if (!text) return true;
    let negated = false;
    if (text.startsWith('only ')) text = text.slice(5).trim();
    if (text.startsWith('not ')) {
      negated = true;
      text = text.slice(4).trim();
    }
    const parts = __tbSplitMedia(text, ' and ').map(part => part.trim()).filter(part => part);
    let known = true;
    let matches = true;
    for (const part of parts) {
      if (part.startsWith('(')) {
        const feature = __tbFeature(part);
        if (feature === 'unknown') {
          known = false;
          matches = false;
        } else if (!feature) {
          matches = false;
        }
      } else if (part !== 'all' && part !== 'screen') {
        matches = false;
      }
    }
    // Unknown features stay false under `not`
    // (<https://drafts.csswg.org/mediaqueries-5/#error-handling>).
    if (!known) return false;
    return negated ? !matches : matches;
  };
  const __tbMediaQueryList = (text) => __tbSplitMedia(String(text), ',').some(__tbMediaQuery);
  Object.defineProperty(globalThis, 'matchMedia', {
    value: function(media) {
      return {
        media: String(media), matches: __tbMediaQueryList(media), onchange: null,
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
