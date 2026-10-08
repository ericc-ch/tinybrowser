  // `window` viewport metrics track the live document viewport, so CDP
  // emulation changes are visible without re-installing globals
  // (<https://drafts.csswg.org/cssom-view/#dom-window-innerwidth>).
  const __tbViewportSize = () => host.__tb_viewport_size();
  Object.defineProperty(globalThis, 'innerWidth', {
    get: () => __tbViewportSize()[0], configurable: true,
  });
  Object.defineProperty(globalThis, 'innerHeight', {
    get: () => __tbViewportSize()[1], configurable: true,
  });
  // No browser chrome exists, so the outer window equals the inner viewport
  // (<https://drafts.csswg.org/cssom-view/#dom-window-outerwidth>).
  Object.defineProperty(globalThis, 'outerWidth', {
    get: () => __tbViewportSize()[0], configurable: true,
  });
  Object.defineProperty(globalThis, 'outerHeight', {
    get: () => __tbViewportSize()[1], configurable: true,
  });

// Constructable stylesheet surface used by component libraries.
// https://drafts.csswg.org/cssom/#the-cssstylesheet-interface
{
  class CSSStyleSheet {
    constructor() {
      this.cssRules = [];
    }
    replaceSync(text) {
      const cssText = String(text);
      this.cssRules = cssText ? [{ cssText }] : [];
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
  // `MediaQueryList` is an `EventTarget` with the legacy listener pair
  // (<https://drafts.csswg.org/cssom-view/#mediaquerylist>). The constructor
  // is captured here so a page-replaced `EventTarget` cannot build the list.
  const __tbMediaSlots = host.slots('MediaQueryList');
  const __tbMediaLists = [];
  const __tbEventTarget = globalThis.EventTarget;
  const __tbEvent = globalThis.Event;
  const __tbAddEventListener = __tbEventTarget.prototype.addEventListener;
  const __tbRemoveEventListener = __tbEventTarget.prototype.removeEventListener;
  const __tbDispatchEvent = __tbEventTarget.prototype.dispatchEvent;
  function MediaQueryList() {
    throw new TypeError('Illegal constructor');
  }
  MediaQueryList.prototype = Object.create(__tbEventTarget.prototype, {
    constructor: { value: MediaQueryList, writable: true, configurable: true },
  });
  Object.defineProperty(MediaQueryList.prototype, Symbol.toStringTag, {
    value: 'MediaQueryList', writable: false, enumerable: false, configurable: true,
  });
  function __tbMediaBrand(list) {
    const slot = __tbMediaSlots.get(list);
    if (!slot) throw new TypeError('Illegal invocation');
    return slot;
  }
  Object.defineProperty(MediaQueryList.prototype, 'media', {
    get() { return __tbMediaBrand(this).media; },
    enumerable: true, configurable: true,
  });
  Object.defineProperty(MediaQueryList.prototype, 'matches', {
    get() { return __tbMediaQueryList(__tbMediaBrand(this).media); },
    enumerable: true, configurable: true,
  });
  function __tbMediaAddListener(callback) {
    const list = this;
    __tbMediaBrand(list);
    if (callback == null) return;
    __tbApply(__tbAddEventListener, list, ['change', callback]);
  }
  function __tbMediaRemoveListener(callback) {
    const list = this;
    __tbMediaBrand(list);
    if (callback == null) return;
    __tbApply(__tbRemoveEventListener, list, ['change', callback]);
  }
  Object.defineProperty(MediaQueryList.prototype, 'addListener', {
    value: __tbMediaAddListener, writable: true, enumerable: true, configurable: true,
  });
  Object.defineProperty(MediaQueryList.prototype, 'removeListener', {
    value: __tbMediaRemoveListener, writable: true, enumerable: true, configurable: true,
  });
  Object.defineProperty(MediaQueryList.prototype, 'onchange', {
    get() { return __tbMediaBrand(this).onchange; },
    set(value) {
      const slot = __tbMediaBrand(this);
      if (value != null && typeof value !== 'function') throw new TypeError('not a function');
      const next = typeof value === 'function' ? value : null;
      if (slot.onchange) __tbApply(__tbRemoveEventListener, this, ['change', slot.onchange]);
      slot.onchange = next;
      if (next) __tbApply(__tbAddEventListener, this, ['change', next]);
    },
    enumerable: true, configurable: true,
  });
  function __tbReportMediaChanges() {
    for (const list of __tbMediaLists) {
      const slot = __tbMediaSlots.get(list);
      if (!slot) continue;
      const matches = __tbMediaQueryList(slot.media);
      if (matches === slot.matches) continue;
      slot.matches = matches;
      const event = new __tbEvent('change');
      Object.defineProperty(event, 'media', { value: slot.media });
      Object.defineProperty(event, 'matches', { value: matches });
      __tbApply(__tbDispatchEvent, list, [event]);
    }
  }
  host.__tbReportMediaChanges = __tbReportMediaChanges;
  Object.defineProperty(globalThis, 'MediaQueryList', {
    value: MediaQueryList, writable: true, configurable: true,
  });
  Object.defineProperty(globalThis, 'matchMedia', {
    value: function(media) {
      const query = String(media);
      const list = __tbConstruct(__tbEventTarget, [], MediaQueryList);
      __tbMediaSlots.set(list, {
        media: query,
        matches: __tbMediaQueryList(query),
        onchange: null,
      });
      __tbMediaLists.push(list);
      return list;
    },
    writable: true, configurable: true,
  });
}
