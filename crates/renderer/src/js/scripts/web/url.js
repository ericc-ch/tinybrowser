globalThis.__tbUSVString = value => {
  value = String(value);
  let out = '';
  for (let index = 0; index < value.length; index++) {
    const code = value.charCodeAt(index);
    if (code >= 0xd800 && code <= 0xdbff) {
      const next = value.charCodeAt(index + 1);
      if (next >= 0xdc00 && next <= 0xdfff) {
        out += value[index] + value[index + 1];
        index++;
      } else {
        out += '\uFFFD';
      }
    } else if (code >= 0xdc00 && code <= 0xdfff) {
      out += '\uFFFD';
    } else {
      out += value[index];
    }
  }
  return out;
};

// URL decomposition components; indexes match `js/url_parts.rs`
// (<https://html.spec.whatwg.org/multipage/links.html#url-decomposition-idl-attributes>).
const __tbUrlPart = (href, index) => {
  const values = globalThis.__tbUrlParts(href, href);
  return values === null || values === undefined ? null : values[index];
};
const __tbRefreshUrlParams = url => {
  if (url._searchParams !== null) {
    const fresh = new URLSearchParams(__tbUrlPart(url._href, 7) || '')._pairs;
    const pairs = url._searchParams._pairs;
    // Replace the contents without replacing the array, so live iterators
    // keep observing the same list
    // (<https://url.spec.whatwg.org/#urlsearchparams-iterate>).
    pairs.length = 0;
    for (const pair of fresh) pairs.push(pair);
  }
};
const __tbSetUrlPart = (url, index, value) => {
  const result = globalThis.__tbUrlSetPart(url._href, url._href, index, value);
  if (result !== null && result !== undefined) {
    url._href = result;
    __tbRefreshUrlParams(url);
  }
};

globalThis.URL = class URL {
  constructor(input, base) {
    // No base means no document fallback: the input must parse absolutely
    // (<https://url.spec.whatwg.org/#concept-url-parser>).
    const href = base === undefined
      ? globalThis.__tbParseUrl(globalThis.__tbUSVString(input))
      : globalThis.__tbResolveUrl(globalThis.__tbUSVString(input), globalThis.__tbUSVString(base));
    if (href == null) throw new TypeError('Invalid URL');
    this._href = href;
    this._searchParams = null;
  }
  get href() { return this._href; }
  set href(value) {
    // A value that fails to parse leaves the URL unchanged; it does not
    // throw (<https://url.spec.whatwg.org/#dom-url-href>).
    const parsed = globalThis.__tbParseUrl(globalThis.__tbUSVString(value));
    if (parsed !== null && parsed !== undefined) {
      this._href = parsed;
      __tbRefreshUrlParams(this);
    }
  }
  toString() { return this._href; }
  toJSON() { return this._href; }
  get protocol() { return __tbUrlPart(this._href, 0); }
  set protocol(value) { __tbSetUrlPart(this, 0, globalThis.__tbUSVString(value)); }
  get username() { return __tbUrlPart(this._href, 1); }
  set username(value) { __tbSetUrlPart(this, 1, globalThis.__tbUSVString(value)); }
  get password() { return __tbUrlPart(this._href, 2); }
  set password(value) { __tbSetUrlPart(this, 2, globalThis.__tbUSVString(value)); }
  get host() { return __tbUrlPart(this._href, 3); }
  set host(value) { __tbSetUrlPart(this, 3, globalThis.__tbUSVString(value)); }
  get hostname() { return __tbUrlPart(this._href, 4); }
  set hostname(value) { __tbSetUrlPart(this, 4, globalThis.__tbUSVString(value)); }
  get port() { return __tbUrlPart(this._href, 5); }
  set port(value) { __tbSetUrlPart(this, 5, globalThis.__tbUSVString(value)); }
  get pathname() { return __tbUrlPart(this._href, 6); }
  set pathname(value) { __tbSetUrlPart(this, 6, globalThis.__tbUSVString(value)); }
  get search() { return __tbUrlPart(this._href, 7); }
  set search(value) { __tbSetUrlPart(this, 7, globalThis.__tbUSVString(value)); }
  get hash() { return __tbUrlPart(this._href, 8); }
  set hash(value) { __tbSetUrlPart(this, 8, globalThis.__tbUSVString(value)); }
  get origin() { return __tbUrlPart(this._href, 9); }
  get searchParams() {
    if (this._searchParams === null) {
      const params = new URLSearchParams(this.search);
      const url = this;
      params._sync = value => {
        const result = globalThis.__tbUrlSetPart(url._href, url._href, 7, String(value));
        if (result !== null && result !== undefined) url._href = result;
      };
      this._searchParams = params;
    }
    return this._searchParams;
  }
};
globalThis.URL.createObjectURL = function(blob) {
  const data = __tbBrand(blob, __tbBlobData, 'value is not a Blob');
  const text = __tbUtf8Decode(data.bytes, false, true).text;
  const url = globalThis.__tbCreateObjectURL(text, data.type);
  if (url == null) throw new RangeError('object URL budget exceeded');
  return url;
};
// https://w3c.github.io/FileAPI/#dfn-revokeObjectURL
globalThis.URL.revokeObjectURL = function(url) {
  globalThis.__tbRevokeObjectURL(String(url));
};
Object.defineProperty(globalThis.URL.prototype, Symbol.toStringTag, { value: 'URL', writable: false, enumerable: false, configurable: true });
// `location` stringifies to its URL, which is what `new URL(input, location)`
// and other base-taking APIs expect
// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-location-href>).
if (globalThis.location !== undefined && globalThis.location !== null) {
  Object.defineProperty(globalThis.location, 'toString', {
    value: function() { return String(this.href); },
    writable: true, enumerable: false, configurable: true,
  });
}
// https://url.spec.whatwg.org/#dom-url-parse
globalThis.URL.parse = function(input, base) {
  try { return new globalThis.URL(input, base); }
  catch (error) { return null; }
};
// https://url.spec.whatwg.org/#dom-url-canparse
globalThis.URL.canParse = function(input, base) {
  return globalThis.URL.parse(input, base) !== null;
};
// https://url.spec.whatwg.org/#interface-urlsearchparams
globalThis.URLSearchParams = class URLSearchParams {
  constructor(init) {
    this._pairs = [];
    this._sync = null;
    if (init instanceof URLSearchParams) {
      this._pairs = init._pairs.map(pair => pair.slice());
      return;
    }
    if (init !== null && typeof init === 'object') {
      if (typeof init[Symbol.iterator] === 'function') {
        for (const pair of init) {
          const values = Array.from(pair);
          if (values.length !== 2) throw new TypeError('parameter pair must contain two values');
          this._pairs.push([globalThis.__tbUSVString(values[0]), globalThis.__tbUSVString(values[1])]);
        }
      } else {
        for (const name of Object.keys(init)) {
          this._pairs.push([globalThis.__tbUSVString(name), globalThis.__tbUSVString(init[name])]);
        }
      }
      return;
    }
    var input = globalThis.__tbUSVString(init === undefined ? '' : init);
    if (input.charAt(0) === '?') input = input.slice(1);
    if (!input) return;
    const decode = value => {
      value = value.replace(/\+/g, ' ');
      try { return decodeURIComponent(value); }
      catch (_) { return value.replace(/%([0-9a-f]{2})/gi, (_m, hex) => String.fromCharCode(parseInt(hex, 16))); }
    };
    for (const item of input.split('&')) {
      // The `application/x-www-form-urlencoded` parser drops empty items
      // (<https://url.spec.whatwg.org/#urlencoded-parsing>).
      if (item === '') continue;
      const separator = item.indexOf('=');
      const name = separator < 0 ? item : item.slice(0, separator);
      const value = separator < 0 ? '' : item.slice(separator + 1);
      this._pairs.push([
        decode(name),
        decode(value)
      ]);
    }
  }
  get size() { return this._pairs.length; }
  get(name) {
    name = globalThis.__tbUSVString(name);
    for (const pair of this._pairs) {
      if (pair[0] === name) return pair[1];
    }
    return null;
  }
  getAll(name) {
    name = globalThis.__tbUSVString(name);
    return this._pairs.filter(pair => pair[0] === name).map(pair => pair[1]);
  }
  has(name, value) {
    name = globalThis.__tbUSVString(name);
    if (arguments.length < 2 || value === undefined) return this._pairs.some(pair => pair[0] === name);
    value = globalThis.__tbUSVString(value);
    return this._pairs.some(pair => pair[0] === name && pair[1] === value);
  }
  append(name, value) {
    this._pairs.push([globalThis.__tbUSVString(name), globalThis.__tbUSVString(value)]);
    if (this._sync) this._sync(this.toString());
  }
  set(name, value) {
    name = globalThis.__tbUSVString(name);
    value = globalThis.__tbUSVString(value);
    let first = -1;
    for (let index = 0; index < this._pairs.length; index++) {
      if (this._pairs[index][0] !== name) continue;
      if (first < 0) {
        first = index;
        this._pairs[index][1] = value;
      } else {
        this._pairs.splice(index, 1);
        index--;
      }
    }
    if (first < 0) this._pairs.push([name, value]);
    if (this._sync) this._sync(this.toString());
  }
  delete(name, value) {
    name = globalThis.__tbUSVString(name);
    const removeValue = !(arguments.length < 2 || value === undefined);
    if (removeValue) value = globalThis.__tbUSVString(value);
    // Splice in place: an iterator over this object must observe mutations
    // (<https://url.spec.whatwg.org/#urlsearchparams-iterate>).
    for (let index = this._pairs.length - 1; index >= 0; index--) {
      const pair = this._pairs[index];
      if (pair[0] === name && (!removeValue || pair[1] === value)) this._pairs.splice(index, 1);
    }
    if (this._sync) this._sync(this.toString());
  }
  sort() {
    // Array.prototype.sort is stable, matching the spec's sort
    // (<https://url.spec.whatwg.org/#dom-urlsearchparams-sort>).
    this._pairs.sort((a, b) => a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0);
    if (this._sync) this._sync(this.toString());
  }
  entries() { return this._pairs[Symbol.iterator](); }
  keys() { return this._pairs.map(pair => pair[0])[Symbol.iterator](); }
  values() { return this._pairs.map(pair => pair[1])[Symbol.iterator](); }
  forEach(callback, thisArg) {
    for (const pair of this._pairs) callback.call(thisArg, pair[1], pair[0], this);
  }
  toString() {
    const encode = value => encodeURIComponent(value)
      .replace(/%20/g, '+')
      .replace(/[!'()~]/g, character =>
        '%' + character.charCodeAt(0).toString(16).toUpperCase());
    return this._pairs.map(pair => encode(pair[0]) + '=' + encode(pair[1])).join('&');
  }
  [Symbol.iterator]() { return this.entries(); }
};
Object.defineProperty(globalThis.URLSearchParams.prototype, Symbol.toStringTag, { value: 'URLSearchParams', writable: false, enumerable: false, configurable: true });

