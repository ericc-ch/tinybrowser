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

// Constructable stylesheet surface used by component libraries, plus the
// associated CSS style sheet for `<style>` and `<link rel=stylesheet>`.
// https://drafts.csswg.org/cssom/#the-cssstylesheet-interface
{
  // Owner element of an associated sheet. Constructable sheets have none;
  // their rule list stays the script-side array `replaceSync` writes.
  const __tbSheetOwners = new WeakMap();
  const __tbRulePath = path => path.join(',');
  function __tbRuleInfo(owner, path) {
    return host.__tb_sheet_rule_info(owner, __tbRulePath(path));
  }
  // Live `CSSRuleList` over the style engine's parsed rules
  // (<https://drafts.csswg.org/cssom/#the-cssrulelist-interface>).
  function __tbLiveRuleList(owner, path, sheet, parentRule) {
    const list = {
      get length() {
        const count = host.__tb_sheet_rule_count(owner, __tbRulePath(path));
        return count < 0 ? 0 : count;
      },
      item(index) {
        index = Number(index);
        if (!(index >= 0 && index < this.length)) return null;
        return __tbLiveRule(owner, path.concat(index), sheet, parentRule);
      },
    };
    list[Symbol.iterator] = function* () {
      for (let index = 0; index < this.length; index++) yield this.item(index);
    };
    return new Proxy(list, {
      get(target, property, receiver) {
        if (typeof property === 'string' && /^\d+$/.test(property)) {
          const item = target.item(Number(property));
          return item === null ? undefined : item;
        }
        const value = Reflect.get(target, property, receiver);
        return typeof value === 'function' ? value.bind(target) : value;
      },
    });
  }
  function __tbLiveRule(owner, path, sheet, parentRule) {
    const read = () => __tbRuleInfo(owner, path) || { cssText: '', type: 0, hasChildRules: false, hasStyle: false };
    const rule = {
      get type() { return read().type; },
      get cssText() { return read().cssText; },
      get parentRule() { return parentRule; },
      get parentStyleSheet() { return sheet; },
      get selectorText() { return read().selectorText || ''; },
      get conditionText() { return read().conditionText || ''; },
      get cssRules() {
        return __tbLiveRuleList(owner, path, sheet, rule);
      },
      get style() {
        if (!read().hasStyle) return null;
        return {
          get cssText() { return host.__tb_sheet_style_css_text(owner, __tbRulePath(path)); },
          getPropertyValue(name) {
            const text = this.cssText;
            const wanted = String(name).toLowerCase();
            for (const part of text.split(';')) {
              const separator = part.indexOf(':');
              if (separator < 0) continue;
              if (part.slice(0, separator).trim().toLowerCase() === wanted) {
                return part.slice(separator + 1).trim();
              }
            }
            return '';
          },
        };
      },
    };
    return rule;
  }
  class CSSStyleSheet {
    constructor() {
      this.cssRules = [];
    }
    replaceSync(text) {
      if (__tbApply(__tbWeakMapGet, __tbSheetOwners, [this]) !== undefined) {
        throw new DOMException('associated sheet is not constructable', 'NotSupportedError');
      }
      const cssText = String(text);
      this.cssRules = cssText ? [{ cssText }] : [];
    }
    replace(text) {
      this.replaceSync(text);
      return Promise.resolve(this);
    }
    insertRule(rule, index) {
      const owner = __tbApply(__tbWeakMapGet, __tbSheetOwners, [this]);
      if (owner === undefined) {
        throw new DOMException('constructable sheet has no parsed rule list', 'NotSupportedError');
      }
      if (index === undefined) index = 0;
      return host.__tb_sheet_insert_rule(owner, '', String(rule), Number(index));
    }
    deleteRule(index) {
      const owner = __tbApply(__tbWeakMapGet, __tbSheetOwners, [this]);
      if (owner === undefined) {
        throw new DOMException('constructable sheet has no parsed rule list', 'NotSupportedError');
      }
      host.__tb_sheet_delete_rule(owner, '', Number(index));
    }
  }
  Object.defineProperty(globalThis, 'CSSStyleSheet', {
    value: CSSStyleSheet, writable: true, configurable: true,
  });
  // Associated CSS style sheet for `<style>` and `<link rel=stylesheet>`
  // (<https://drafts.csswg.org/cssom/#associated-css-style-sheet>,
  // <https://html.spec.whatwg.org/multipage/semantics.html#the-style-element:create-a-css-style-sheet>,
  // <https://html.spec.whatwg.org/multipage/semantics.html#the-link-element:create-a-css-style-sheet>).
  // Created while the element is connected so an earlier script in the same
  // insert can observe `sheet` after the style/link node's insertion steps
  // and before that script's post-connection steps. `cssRules` reads the
  // style engine's parsed list for that owner node.
  const __tbSheets = new WeakMap();
  // Captured at realm init: the sheet getters brand-check against it, and
  // pages may replace the global afterwards.
  const __tbCssomBrand = value => {
    // Realm-agnostic: cross-realm elements fail `instanceof` against this
    // realm's `Element`, so check the node kind instead. A forged
    // `{nodeType: 1}` passes here but the Rust `require_interface("Element")`
    // rejects it authoritatively (page prototypes never grant brand).
    if ((value === null || (typeof value !== 'object' && typeof value !== 'function'))
      || value.nodeType !== 1) throw new TypeError('Illegal invocation');
    return value;
  };
  function __tbAssociatedSheet(element) {
    __tbCssomBrand(element);
    let sheet = __tbApply(__tbWeakMapGet, __tbSheets, [element]);
    if (sheet !== undefined) return sheet;
    sheet = new CSSStyleSheet();
    __tbApply(__tbWeakMapSet, __tbSheetOwners, [sheet, element]);
    // The constructor's own array is the constructable-sheet list. An
    // associated sheet's `cssRules` is the engine's list instead
    // (<https://drafts.csswg.org/cssom/#dom-cssstylesheet-cssrules>).
    delete sheet.cssRules;
    let rules = null;
    __tbDefineProperty(sheet, 'cssRules', {
      get() {
        if (rules === null) rules = __tbLiveRuleList(element, [], sheet, null);
        return rules;
      },
      enumerable: true,
      configurable: true,
    });
    __tbApply(__tbWeakMapSet, __tbSheets, [element, sheet]);
    return sheet;
  }
  Object.defineProperty(globalThis.HTMLStyleElement.prototype, 'sheet', {
    get() {
      __tbCssomBrand(this);
      if (!this.isConnected) return null;
      return __tbAssociatedSheet(this);
    },
    enumerable: true,
    configurable: true,
  });
  Object.defineProperty(globalThis.HTMLLinkElement.prototype, 'sheet', {
    get() {
      __tbCssomBrand(this);
      if (!this.isConnected) return null;
      const rel = __tbApply(__tbStringToLowerCase, __tbIDLString(this.rel ?? this.getAttribute('rel') ?? ''), []);
      const tokens = __tbApply(__tbStringSplit, rel, [/\s+/]);
      if (__tbArray.indexOf(tokens, 'stylesheet') === -1) return null;
      return __tbAssociatedSheet(this);
    },
    enumerable: true,
    configurable: true,
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
  // (<https://drafts.csswg.org/cssom/#resolved-values>). The style engine's
  // resolved value is authoritative when it has one; the inline style and then
  // the initial-value table below cover what it does not resolve (custom
  // properties, properties outside the supported set).
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
      const resolved = name => {
        if (!element || typeof element.nodeType !== 'number') return '';
        return host.__tb_resolved_style(element, name);
      };
      const read = name => {
        // The resolved value comes first: it carries the cascade, `!important`
        // declarations, and layout-dependent used values. The inline
        // declaration is only a fallback for elements the engine has no style
        // for (a detached element, or a property outside its tables).
        // https://drafts.csswg.org/cssom/#dom-window-getcomputedstyle
        const engine = resolved(name);
        if (engine !== '') return engine;
        const value = inline.getPropertyValue(name);
        if (value !== '') return value;
        return __tbInitialValues[String(name).toLowerCase()] ?? '';
      };
      return new Proxy(inline, {
        get(target, property, receiver) {
          if (typeof property !== 'string') return Reflect.get(target, property, receiver);
          if (property === 'getPropertyValue') return read;
          const kebab = property.replace(/[A-Z]/g, letter => '-' + letter.toLowerCase());
          const engine = resolved(kebab);
          if (engine !== '') return engine;
          const value = Reflect.get(target, property, receiver);
          if (value !== undefined && value !== null && value !== '') return value;
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
