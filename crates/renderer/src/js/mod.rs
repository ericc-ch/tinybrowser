//! `QuickJS` host for one document: eval, timers, `fetch`, DOM host objects.
//!
//! Callbacks live in JS (`__tb_timeouts`, `__tb_fetchCbs`). Rust holds
//! integer ids so a `Function` never crosses the JS boundary. Invocation
//! uses `Function::call` on the renderer thread.

mod bindings;
mod events;
mod intl;
mod world;

pub(crate) use world::{DocumentStreamCommand, FrameNavigation, RealmRegistry};

use std::cell::{Cell, OnceCell, RefCell};
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rquickjs::{
    Array, Coerced, Context, Ctx, FromJs, Function, Object, Runtime, Value,
    context::EvalOptions, prelude::Func,
};

pub(crate) use world::World;

use crate::document::Stop;

const MAX_RUNTIME_MEMORY: usize = 32 * 1024 * 1024;
const MAX_RUNTIME_STACK: usize = 512 * 1024;
const DEFAULT_SCRIPT_BUDGET: Duration = Duration::from_secs(5);

const INSTALL_WEB_APIS_JS: &str = r"
globalThis.__tb_timeouts = [];
globalThis.__tb_fetchCbs = Object.create(null);
globalThis.__tb_fetchSeq = 0;
['__scheduleTimeout','__cancelTimeout','__queueFetch','__cookieGet','__cookieSet','__tbCreateObjectURL','__tbRevokeObjectURL','__tbResolveUrl','__tbParseUrl','__tb_timeouts','__tb_fetchCbs'].forEach(function(k) {
  Object.defineProperty(globalThis, k, { writable: false, configurable: false, enumerable: false });
});
globalThis.setTimeout = function(fn, ms) {
  var id = globalThis.__tb_timeouts.length;
  globalThis.__tb_timeouts.push(fn);
  globalThis.__scheduleTimeout(id, Number(ms));
  return id;
};
globalThis.clearTimeout = function(id) {
  globalThis.__tb_timeouts[id] = function() {};
  globalThis.__cancelTimeout(Number(id));
};
// The engine has no rendering pipeline; a frame callback is a 16ms timer
// (<https://html.spec.whatwg.org/multipage/imagebitmap-and-animations.html#dom-animationframeprovider-requestanimationframe>).
// Deviations: callbacks queued in the same frame do not share a
// DOMHighResTimeStamp (each gets `Date.now()`), handles share the timer
// table with the +1 offset, and `cancelAnimationFrame` of a plain
// `setTimeout` handle cancels that timer.
globalThis.requestAnimationFrame = function(fn) {
  if (typeof fn !== 'function') {
    throw new TypeError('requestAnimationFrame requires a callback');
  }
  return globalThis.setTimeout(function() {
    fn(Date.now());
  }, 16) + 1;
};
globalThis.cancelAnimationFrame = function(id) {
  globalThis.clearTimeout(Number(id) - 1);
};
if (!globalThis.document) {
  globalThis.document = {};
}
// A no-op console: enough for scripts that only log, without a logging pipe.
if (typeof globalThis.console === 'undefined') {
  const noop = function() {};
  globalThis.console = {
    log: noop, info: noop, warn: noop, error: noop, debug: noop,
    trace: noop, dir: noop, group: noop, groupEnd: noop, table: noop,
    assert: noop, time: noop, timeEnd: noop, count: noop,
  };
}
Object.defineProperty(document, 'cookie', {
  get() { return globalThis.__cookieGet(); },
  set(v) { globalThis.__cookieSet(String(v)); }
});
// https://fetch.spec.whatwg.org/#headers-class
// The host dials GET only and response bodies are text, so fetch ignores
// request bodies, headers, and credentials, and rejects other methods.
const __tbHeadersData = Symbol.for('tinybrowser.headers.data');
const __tbResponseData = Symbol.for('tinybrowser.response.data');
const __tbRequestData = Symbol.for('tinybrowser.request.data');
const __tbBrand = (value, symbol, message) => {
  const data = value === null || value === undefined ? undefined : value[symbol];
  if (data === undefined) throw new TypeError(message === undefined ? 'Illegal invocation' : message);
  return data;
};
globalThis.Headers = class Headers {
  constructor(init) {
    const entries = [];
    if (init !== undefined && init !== null) {
      if (typeof init[Symbol.iterator] === 'function') {
        for (const pair of init) {
          const values = Array.from(pair);
          if (values.length !== 2) throw new TypeError('Header pairs must contain two values');
          entries.push([String(values[0]).toLowerCase(), String(values[1]).trim()]);
        }
      } else {
        for (const name of Object.keys(init)) entries.push([name.toLowerCase(), String(init[name]).trim()]);
      }
    }
    Object.defineProperty(this, __tbHeadersData, {
      value: entries, writable: false, enumerable: false, configurable: false,
    });
  }
  append(name, value) { __tbBrand(this, __tbHeadersData).push([String(name).toLowerCase(), String(value).trim()]); }
  delete(name) {
    const entries = __tbBrand(this, __tbHeadersData);
    name = String(name).toLowerCase();
    for (let index = entries.length - 1; index >= 0; index--) {
      if (entries[index][0] === name) entries.splice(index, 1);
    }
  }
  get(name) {
    const entries = __tbBrand(this, __tbHeadersData);
    name = String(name).toLowerCase();
    for (const [header, value] of entries) {
      if (header === name) return value;
    }
    return null;
  }
  has(name) { return this.get(name) !== null; }
  set(name, value) {
    this.delete(name);
    __tbBrand(this, __tbHeadersData).push([String(name).toLowerCase(), String(value).trim()]);
  }
  forEach(callback, thisArg) {
    for (const [name, value] of __tbBrand(this, __tbHeadersData)) callback.call(thisArg, value, name, this);
  }
  entries() { return __tbBrand(this, __tbHeadersData).map(entry => entry.slice())[Symbol.iterator](); }
  keys() { return __tbBrand(this, __tbHeadersData).map(entry => entry[0])[Symbol.iterator](); }
  values() { return __tbBrand(this, __tbHeadersData).map(entry => entry[1])[Symbol.iterator](); }
  [Symbol.iterator]() { return this.entries(); }
};
Object.defineProperty(globalThis.Headers.prototype, Symbol.toStringTag, { value: 'Headers', writable: false, enumerable: false, configurable: true });
// https://fetch.spec.whatwg.org/#response-class
globalThis.Response = class Response {
  constructor(body, init) {
    const options = init === undefined ? {} : Object(init);
    const text = body === undefined || body === null ? '' : String(body);
    Object.defineProperty(this, __tbResponseData, {
      value: {
        text,
        status: options.status === undefined ? 200 : Number(options.status),
        statusText: options.statusText === undefined ? '' : String(options.statusText),
        url: options.url === undefined ? '' : String(options.url),
        headers: options.headers instanceof globalThis.Headers ? options.headers : new globalThis.Headers(options.headers),
      },
      writable: false, enumerable: false, configurable: false,
    });
  }
  get status() { return __tbBrand(this, __tbResponseData).status; }
  get statusText() { return __tbBrand(this, __tbResponseData).statusText; }
  get url() { return __tbBrand(this, __tbResponseData).url; }
  get headers() { return __tbBrand(this, __tbResponseData).headers; }
  get ok() { const status = __tbBrand(this, __tbResponseData).status; return status >= 200 && status <= 299; }
  get bodyUsed() { return false; }
  text() { return Promise.resolve(__tbBrand(this, __tbResponseData).text); }
  json() {
    try { return Promise.resolve(JSON.parse(__tbBrand(this, __tbResponseData).text)); }
    catch (error) { return Promise.reject(error); }
  }
  arrayBuffer() { return Promise.resolve(__tbUtf8Encode(__tbBrand(this, __tbResponseData).text).buffer); }
  blob() {
    const data = __tbBrand(this, __tbResponseData);
    const type = data.headers.get('content-type');
    return Promise.resolve(new Blob([data.text], { type: type === null ? '' : type }));
  }
  clone() {
    const data = __tbBrand(this, __tbResponseData);
    return new Response(data.text, { status: data.status, statusText: data.statusText, url: data.url, headers: data.headers });
  }
};
Object.defineProperty(globalThis.Response.prototype, Symbol.toStringTag, { value: 'Response', writable: false, enumerable: false, configurable: true });
const __tbLookupObjectUrl = url => {
  const key = url.split('#')[0];
  const contents = globalThis.__tbObjectUrlContents(key);
  if (contents === null || contents === undefined) return null;
  const content_type = globalThis.__tbObjectUrlType(key);
  return { contents, type: content_type === null || content_type === undefined ? '' : content_type };
};
globalThis.Request = class Request {
  constructor(input, init) {
    const options = init === undefined ? {} : Object(init);
    let url;
    let method = 'GET';
    let blob = null;
    if (input instanceof globalThis.Request) {
      const source = __tbBrand(input, __tbRequestData);
      url = source.url;
      method = source.method;
      blob = source.blob;
    } else {
      const resolved = globalThis.__tbResolveUrl(String(input), undefined);
      url = resolved === null ? String(input) : resolved;
    }
    if (options.method !== undefined) method = String(options.method).toUpperCase();
    // A Request keeps a blob URL's data alive, so fetching it still works
    // after revokeObjectURL (<https://w3c.github.io/FileAPI/#lifeTime>).
    if (blob === null && url.indexOf('blob:') === 0) blob = __tbLookupObjectUrl(url);
    Object.defineProperty(this, __tbRequestData, {
      value: { url, method, blob }, writable: false, enumerable: false, configurable: false,
    });
  }
  get url() { return __tbBrand(this, __tbRequestData).url; }
  get method() { return __tbBrand(this, __tbRequestData).method; }
  clone() {
    const data = __tbBrand(this, __tbRequestData);
    const copy = Object.create(globalThis.Request.prototype);
    Object.defineProperty(copy, __tbRequestData, {
      value: { url: data.url, method: data.method, blob: data.blob },
      writable: false, enumerable: false, configurable: false,
    });
    return copy;
  }
};
Object.defineProperty(globalThis.Request.prototype, Symbol.toStringTag, { value: 'Request', writable: false, enumerable: false, configurable: true });
const __tbMakeResponse = (body, status, url, type) => {
  const headers = new globalThis.Headers();
  if (type) headers.set('content-type', type);
  return new globalThis.Response(body, { status, url, headers });
};
globalThis.fetch = function(input, init) {
  const options = init === undefined ? {} : Object(init);
  let url;
  let method = 'GET';
  let blob = null;
  if (input instanceof globalThis.Request) {
    const source = __tbBrand(input, __tbRequestData);
    url = source.url;
    method = source.method;
    blob = source.blob;
  } else {
    const resolved = globalThis.__tbResolveUrl(String(input), undefined);
    url = resolved === null ? String(input) : resolved;
  }
  if (options.method !== undefined) method = String(options.method).toUpperCase();
  return new Promise(function(resolve, reject) {
    if (url.indexOf('blob:') === 0) {
      if (method !== 'GET') {
        reject(new TypeError('Failed to fetch'));
        return;
      }
      const entry = blob === null ? __tbLookupObjectUrl(url) : blob;
      if (entry === null) {
        reject(new TypeError('Failed to fetch'));
        return;
      }
      resolve(__tbMakeResponse(entry.contents, 200, url, entry.type));
      return;
    }
    if (method !== 'GET') {
      reject(new TypeError('Failed to fetch'));
      return;
    }
    const id = ++globalThis.__tb_fetchSeq;
    globalThis.__tb_fetchCbs[id] = function(ok, status, body) {
      delete globalThis.__tb_fetchCbs[id];
      if (ok) resolve(__tbMakeResponse(String(body), status, url, ''));
      else reject(new TypeError('Failed to fetch'));
    };
    globalThis.__queueFetch(String(url), id);
  });
};
// https://w3c.github.io/FileAPI/#blob
// Blob bytes live as a Uint8Array under a symbol key, so no IDL member is an
// own property and a method called from another realm of the same runtime can
// still recognize its receiver.
const __tbBlobData = Symbol.for('tinybrowser.blob.data');
const __tbFileData = Symbol.for('tinybrowser.file.data');
const __tbFileListData = Symbol.for('tinybrowser.filelist.data');
const __tbReaderData = Symbol.for('tinybrowser.filereader.data');
const __tbProgressData = Symbol.for('tinybrowser.progress.data');
const __tbDecoderData = Symbol.for('tinybrowser.textdecoder.data');
const __tbStreamData = Symbol.for('tinybrowser.readablestream.data');
// https://encoding.spec.whatwg.org/#utf-8-encoder, with lone surrogates
// replaced as the standard requires.
const __tbUtf8Encode = value => {
  value = String(value);
  const bytes = [];
  for (let index = 0; index < value.length; index++) {
    let code = value.charCodeAt(index);
    if (code >= 0xd800 && code <= 0xdbff && index + 1 < value.length) {
      const low = value.charCodeAt(index + 1);
      if (low >= 0xdc00 && low <= 0xdfff) {
        code = 0x10000 + ((code - 0xd800) << 10) + (low - 0xdc00);
        index++;
      }
    }
    if (code >= 0xd800 && code <= 0xdfff) code = 0xfffd;
    if (code <= 0x7f) bytes.push(code);
    else if (code <= 0x7ff) bytes.push(0xc0 | (code >> 6), 0x80 | (code & 0x3f));
    else if (code <= 0xffff) bytes.push(0xe0 | (code >> 12), 0x80 | ((code >> 6) & 0x3f), 0x80 | (code & 0x3f));
    else bytes.push(0xf0 | (code >> 18), 0x80 | ((code >> 12) & 0x3f), 0x80 | ((code >> 6) & 0x3f), 0x80 | (code & 0x3f));
  }
  return new Uint8Array(bytes);
};
// https://encoding.spec.whatwg.org/#utf-8-decoder. The trailing incomplete
// sequence is returned so a streaming TextDecoder can carry it forward.
const __tbUtf8Decode = (bytes, fatal, ignoreBOM) => {
  let text = '';
  let index = 0;
  if (!ignoreBOM && bytes.length >= 3 && bytes[0] === 0xef && bytes[1] === 0xbb && bytes[2] === 0xbf) index = 3;
  while (index < bytes.length) {
    const first = bytes[index];
    let needed = 0;
    let code = 0;
    if (first < 0x80) { text += String.fromCharCode(first); index++; continue; }
    if (first >= 0xc2 && first <= 0xdf) { needed = 1; code = first & 0x1f; }
    else if (first >= 0xe0 && first <= 0xef) { needed = 2; code = first & 0x0f; }
    else if (first >= 0xf0 && first <= 0xf4) { needed = 3; code = first & 0x07; }
    if (needed === 0) {
      if (fatal) throw new TypeError('The encoded data was not valid.');
      text += '\ufffd';
      index++;
      continue;
    }
    if (index + needed >= bytes.length) break;
    let valid = true;
    for (let offset = 1; offset <= needed; offset++) {
      const next = bytes[index + offset];
      if ((next & 0xc0) !== 0x80) { valid = false; break; }
      code = (code << 6) | (next & 0x3f);
    }
    if (valid) {
      const overlong = (needed === 1 && code < 0x80) || (needed === 2 && code < 0x800) || (needed === 3 && code < 0x10000);
      if (overlong || code > 0x10ffff || (code >= 0xd800 && code <= 0xdfff)) valid = false;
    }
    if (!valid) {
      if (fatal) throw new TypeError('The encoded data was not valid.');
      text += '\ufffd';
      index++;
      continue;
    }
    text += String.fromCodePoint(code);
    index += needed + 1;
  }
  return { text, remainder: bytes.slice(index) };
};
// https://encoding.spec.whatwg.org/#utf-16le-decoder
const __tbUtf16Decode = (bytes, littleEndian, ignoreBOM) => {
  let start = 0;
  let swap = littleEndian;
  if (!ignoreBOM && bytes.length >= 2) {
    if (bytes[0] === 0xff && bytes[1] === 0xfe) { swap = true; start = 2; }
    else if (bytes[0] === 0xfe && bytes[1] === 0xff) { swap = false; start = 2; }
  }
  let text = '';
  for (let index = start; index + 1 < bytes.length; index += 2) {
    const code = swap ? bytes[index] | (bytes[index + 1] << 8) : (bytes[index] << 8) | bytes[index + 1];
    text += String.fromCharCode(code);
  }
  const consumed = bytes.length - ((bytes.length - start) & 1);
  return { text, remainder: bytes.slice(consumed) };
};
// https://encoding.spec.whatwg.org/#windows-1252
const __tbWindows1252 = bytes => {
  const table = '\u20ac\u0081\u201a\u0192\u201e\u2026\u2020\u2021\u02c6\u2030\u0160\u2039\u0152\u008d\u017d\u008f\u0090\u2018\u2019\u201c\u201d\u2022\u2013\u2014\u02dc\u2122\u0161\u203a\u0153\u009d\u017e\u0178';
  let text = '';
  for (const byte of bytes) {
    text += byte >= 0x80 && byte <= 0x9f ? table[byte - 0x80] : String.fromCharCode(byte);
  }
  return text;
};
const __tbEncoding = label => {
  label = String(label).trim().toLowerCase();
  if (label === 'utf-8' || label === 'utf8' || label === 'unicode-1-1-utf-8') return 'utf-8';
  if (label === 'utf-16' || label === 'utf-16le') return 'utf-16le';
  if (label === 'utf-16be') return 'utf-16be';
  if (label === 'windows-1252' || label === 'cp1252' || label === 'x-cp1252') return 'windows-1252';
  return null;
};
const __tbDecodeBytes = (bytes, encoding, fatal, ignoreBOM) => {
  if (encoding === 'utf-16le' || encoding === 'utf-16be') {
    return __tbUtf16Decode(bytes, encoding === 'utf-16le', ignoreBOM).text;
  }
  if (encoding === 'windows-1252') return __tbWindows1252(bytes);
  const decoded = __tbUtf8Decode(bytes, fatal, ignoreBOM);
  return decoded.text + (decoded.remainder.length ? '\ufffd' : '');
};
// https://w3c.github.io/FileAPI/#convert-line-endings-to-native: LF is the
// native line ending on this platform, so CR and CRLF collapse to LF.
const __tbNativeEndings = value => String(value).replace(/\r\n?|\n/g, '\n');
// https://webidl.spec.whatwg.org/#Clamp: round to the nearest integer with
// ties going to the even integer.
const __tbClampRound = value => {
  value = Number(value);
  if (Number.isNaN(value) || value === 0) return 0;
  if (value === Infinity) return Number.MAX_SAFE_INTEGER;
  if (value === -Infinity) return Number.MIN_SAFE_INTEGER;
  const floor = Math.floor(value);
  const fraction = value - floor;
  if (fraction < 0.5) return floor;
  if (fraction > 0.5) return floor + 1;
  return floor % 2 === 0 ? floor : floor + 1;
};
globalThis.TextEncoder = class TextEncoder {
  constructor() {
    Object.defineProperty(this, 'encoding', { value: 'utf-8', enumerable: true, configurable: true });
  }
  encode(input) {
    return __tbUtf8Encode(input === undefined ? '' : input);
  }
  encodeInto(source, destination) {
    if (!ArrayBuffer.isView(destination) || destination instanceof DataView) {
      throw new TypeError('The destination argument must be a Uint8Array');
    }
    source = String(source === undefined ? '' : source);
    // Encode code point by code point so `read` can stop at the last code
    // unit whose bytes fit in the destination
    // (<https://encoding.spec.whatwg.org/#dom-textencoder-encodeinto>).
    let read = 0;
    let written = 0;
    while (read < source.length) {
      let code = source.charCodeAt(read);
      let units = 1;
      if (code >= 0xd800 && code <= 0xdbff && read + 1 < source.length) {
        const low = source.charCodeAt(read + 1);
        if (low >= 0xdc00 && low <= 0xdfff) {
          code = 0x10000 + ((code - 0xd800) << 10) + (low - 0xdc00);
          units = 2;
        }
      }
      if (code >= 0xd800 && code <= 0xdfff) code = 0xfffd;
      const size = code <= 0x7f ? 1 : code <= 0x7ff ? 2 : code <= 0xffff ? 3 : 4;
      if (written + size > destination.length) break;
      if (size === 1) {
        destination[written++] = code;
      } else if (size === 2) {
        destination[written++] = 0xc0 | (code >> 6);
        destination[written++] = 0x80 | (code & 0x3f);
      } else if (size === 3) {
        destination[written++] = 0xe0 | (code >> 12);
        destination[written++] = 0x80 | ((code >> 6) & 0x3f);
        destination[written++] = 0x80 | (code & 0x3f);
      } else {
        destination[written++] = 0xf0 | (code >> 18);
        destination[written++] = 0x80 | ((code >> 12) & 0x3f);
        destination[written++] = 0x80 | ((code >> 6) & 0x3f);
        destination[written++] = 0x80 | (code & 0x3f);
      }
      read += units;
    }
    return { read, written };
  }
};
Object.defineProperty(globalThis.TextEncoder.prototype, Symbol.toStringTag, { value: 'TextEncoder', writable: false, enumerable: false, configurable: true });
globalThis.TextDecoder = class TextDecoder {
  constructor(label, options) {
    const optionsObject = options === undefined ? {} : Object(options);
    const encoding = label === undefined ? 'utf-8' : __tbEncoding(String(label));
    if (encoding === null) throw new RangeError('The encoding label is not supported');
    Object.defineProperty(this, __tbDecoderData, {
      value: {
        encoding,
        fatal: optionsObject.fatal !== undefined && Boolean(optionsObject.fatal),
        ignoreBOM: optionsObject.ignoreBOM !== undefined && Boolean(optionsObject.ignoreBOM),
        pending: new Uint8Array(0),
      },
      writable: false, enumerable: false, configurable: false,
    });
  }
  get encoding() { return __tbBrand(this, __tbDecoderData).encoding; }
  get fatal() { return __tbBrand(this, __tbDecoderData).fatal; }
  get ignoreBOM() { return __tbBrand(this, __tbDecoderData).ignoreBOM; }
  decode(input, options) {
    const data = __tbBrand(this, __tbDecoderData);
    if (input !== undefined && input !== null) {
      let bytes;
      if (ArrayBuffer.isView(input)) bytes = new Uint8Array(input.buffer, input.byteOffset, input.byteLength);
      else if (input instanceof ArrayBuffer) bytes = new Uint8Array(input);
      else throw new TypeError('The input argument must be an ArrayBuffer or ArrayBufferView');
      if (bytes.length > 0) {
        const combined = new Uint8Array(data.pending.length + bytes.length);
        combined.set(data.pending);
        combined.set(bytes, data.pending.length);
        data.pending = combined;
      }
    }
    const stream = options !== undefined && options.stream === true;
    let text;
    if (data.encoding === 'utf-8') {
      const decoded = __tbUtf8Decode(data.pending, data.fatal, data.ignoreBOM);
      data.pending = stream ? decoded.remainder : new Uint8Array(0);
      if (!stream && decoded.remainder.length) {
        // A truncated tail is a decode error; fatal turns it into a throw,
        // otherwise it is one replacement character
        // (<https://encoding.spec.whatwg.org/#utf-8-decoder>).
        if (data.fatal) throw new TypeError('The encoded data was not valid.');
        text = decoded.text + '\ufffd';
      } else {
        text = decoded.text;
      }
    } else if (data.encoding === 'windows-1252') {
      text = __tbWindows1252(data.pending);
      data.pending = new Uint8Array(0);
    } else {
      const decoded = __tbUtf16Decode(data.pending, data.encoding === 'utf-16le', data.ignoreBOM);
      data.pending = stream ? decoded.remainder : new Uint8Array(0);
      text = decoded.text;
    }
    return text;
  }
};
Object.defineProperty(globalThis.TextDecoder.prototype, Symbol.toStringTag, { value: 'TextDecoder', writable: false, enumerable: false, configurable: true });
// https://html.spec.whatwg.org/multipage/webappapis.html#atob
const __tbBase64Table = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
const __tbBase64Encode = bytes => {
  let text = '';
  for (let index = 0; index < bytes.length; index += 3) {
    const first = bytes[index];
    const second = index + 1 < bytes.length ? bytes[index + 1] : null;
    const third = index + 2 < bytes.length ? bytes[index + 2] : null;
    text += __tbBase64Table[first >> 2];
    text += __tbBase64Table[((first & 3) << 4) | (second === null ? 0 : second >> 4)];
    text += second === null ? '=' : __tbBase64Table[((second & 15) << 2) | (third === null ? 0 : third >> 6)];
    text += third === null ? '=' : __tbBase64Table[third & 63];
  }
  return text;
};
globalThis.btoa = function(input) {
  input = String(input);
  for (let index = 0; index < input.length; index++) {
    if (input.charCodeAt(index) > 0xff) {
      throw new DOMException('The string to be encoded contains characters outside of the Latin1 range.', 'InvalidCharacterError');
    }
  }
  const bytes = new Uint8Array(input.length);
  for (let index = 0; index < input.length; index++) bytes[index] = input.charCodeAt(index);
  return __tbBase64Encode(bytes);
};
globalThis.atob = function(input) {
  const cleaned = String(input).replace(/[\t\n\f\r ]/g, '');
  let body = cleaned;
  if (body.endsWith('==')) body = body.slice(0, -2);
  else if (body.endsWith('=')) body = body.slice(0, -1);
  if (/[^A-Za-z0-9+/]/.test(body) || body.length % 4 === 1) {
    throw new DOMException('The string to be decoded is not correctly encoded.', 'InvalidCharacterError');
  }
  let text = '';
  let buffer = 0;
  let bits = 0;
  for (const character of body) {
    buffer = (buffer << 6) | __tbBase64Table.indexOf(character);
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      text += String.fromCharCode((buffer >> bits) & 0xff);
    }
  }
  return text;
};
// A ReadableStream with a synchronous chunk queue: enough for Blob.stream()
// and other producers that push all their chunks up front.
const __tbStreamBrand = value => __tbBrand(value, __tbStreamData);
globalThis.ReadableStream = class ReadableStream {
  constructor(source, strategy) {
    const sourceObject = source === undefined ? {} : Object(source);
    const data = {
      chunks: [], closed: false, errored: null, canceled: false, reader: null,
      pull: typeof sourceObject.pull === 'function' ? sourceObject.pull : null,
      cancel: typeof sourceObject.cancel === 'function' ? sourceObject.cancel : null,
      controller: null,
    };
    Object.defineProperty(this, __tbStreamData, { value: data, writable: false, enumerable: false, configurable: false });
    data.controller = {
      enqueue(chunk) { if (!data.closed && data.errored === null && !data.canceled) data.chunks.push(chunk); },
      close() { data.closed = true; },
      error(error) { data.errored = error; },
      get desiredSize() { return data.closed ? null : 1; },
    };
    if (typeof sourceObject.start === 'function') sourceObject.start(data.controller);
  }
  get locked() { return __tbStreamBrand(this).reader !== null; }
  getReader() {
    const data = __tbStreamBrand(this);
    if (data.reader !== null) throw new TypeError('The stream is locked to a reader');
    const reader = {
      read: () => {
        const step = () => {
          if (data.errored !== null) return Promise.reject(data.errored);
          if (data.chunks.length > 0) return Promise.resolve({ value: data.chunks.shift(), done: false });
          if (data.closed || data.canceled) return Promise.resolve({ value: undefined, done: true });
          if (data.pull !== null) return Promise.resolve(data.pull(data.controller)).then(step);
          return Promise.resolve({ value: undefined, done: true });
        };
        return step();
      },
      cancel: reason => this.cancel(reason),
      releaseLock: () => { data.reader = null; },
      closed: Promise.resolve(),
    };
    data.reader = reader;
    return reader;
  }
  cancel(reason) {
    const data = __tbStreamBrand(this);
    data.canceled = true;
    if (data.cancel !== null) return Promise.resolve(data.cancel(reason));
    return Promise.resolve();
  }
};
Object.defineProperty(globalThis.ReadableStream.prototype, Symbol.toStringTag, { value: 'ReadableStream', writable: false, enumerable: false, configurable: true });
// https://xhr.spec.whatwg.org/#interface-progressevent
globalThis.ProgressEvent = class ProgressEvent extends Event {
  constructor(type, init) {
    const eventInit = init === undefined ? {} : Object(init);
    super(String(type), eventInit);
    Object.defineProperty(this, __tbProgressData, {
      value: {
        lengthComputable: eventInit.lengthComputable === undefined ? false : Boolean(eventInit.lengthComputable),
        loaded: eventInit.loaded === undefined ? 0 : Number(eventInit.loaded),
        total: eventInit.total === undefined ? 0 : Number(eventInit.total),
      },
      writable: false, enumerable: false, configurable: false,
    });
  }
  get lengthComputable() { return __tbBrand(this, __tbProgressData).lengthComputable; }
  get loaded() { return __tbBrand(this, __tbProgressData).loaded; }
  get total() { return __tbBrand(this, __tbProgressData).total; }
};
Object.defineProperty(globalThis.ProgressEvent.prototype, Symbol.toStringTag, { value: 'ProgressEvent', writable: false, enumerable: false, configurable: true });
// Web IDL sequence conversion for blob parts: the sequence must be an object,
// elements convert left to right, and platform objects with indexed
// properties convert through `length` and the index getters
// (<https://webidl.spec.whatwg.org/#es-sequence>).
const __tbConvertBlobParts = blobParts => {
  const parts = blobParts === undefined ? [] : blobParts;
  if (parts === null || (typeof parts !== 'object' && typeof parts !== 'function')) {
    throw new TypeError('The blobParts argument must be a sequence');
  }
  const iteratorMethod = parts[Symbol.iterator];
  const converted = [];
  const convertPart = part => {
    const fromBlob = part !== null && typeof part === 'object' ? part[__tbBlobData] : undefined;
    if (fromBlob !== undefined) {
      converted.push(fromBlob.bytes);
    } else if (part instanceof ArrayBuffer || (typeof SharedArrayBuffer !== 'undefined' && part instanceof SharedArrayBuffer)) {
      converted.push(new Uint8Array(part).slice());
    } else if (ArrayBuffer.isView(part)) {
      converted.push(new Uint8Array(part.buffer, part.byteOffset, part.byteLength).slice());
    } else {
      converted.push(String(part));
    }
  };
  if (iteratorMethod !== undefined && iteratorMethod !== null) {
    if (typeof iteratorMethod !== 'function') {
      throw new TypeError('The blobParts argument must be iterable');
    }
    const iterator = iteratorMethod.call(parts);
    while (true) {
      const step = iterator.next();
      if (step.done) break;
      convertPart(step.value);
    }
  } else if (typeof parts.item === 'function' && typeof parts.length === 'number') {
    const length = Number(parts.length);
    for (let index = 0; index < length; index++) convertPart(parts[index]);
  } else {
    throw new TypeError('The blobParts argument must be iterable');
  }
  return converted;
};
// Dictionary members evaluate in lexicographic order, so each member gets a
// helper the constructors call in IDL order
// (<https://webidl.spec.whatwg.org/#es-dictionary>).
const __tbEndings = value => {
  if (value === undefined) return 'transparent';
  value = String(value);
  if (value !== 'transparent' && value !== 'native') {
    throw new TypeError('The endings option must be transparent or native');
  }
  return value;
};
const __tbBlobType = value => {
  if (value === undefined) return '';
  value = String(value);
  return [...value].some(character => character < ' ' || character > '~') ? '' : value;
};
const __tbBlobBytes = (converted, endings) => {
  const chunks = converted.map(part => typeof part === 'string'
    ? __tbUtf8Encode(endings === 'native' ? __tbNativeEndings(part) : part)
    : part);
  let length = 0;
  for (const chunk of chunks) length += chunk.length;
  const bytes = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.length;
  }
  return bytes;
};
globalThis.Blob = class Blob {
  constructor(blobParts, options) {
    const converted = __tbConvertBlobParts(blobParts);
    let endings = 'transparent';
    let type = '';
    if (options !== undefined && options !== null) {
      if (typeof options !== 'object' && typeof options !== 'function') {
        throw new TypeError('The options argument must be a property bag');
      }
      endings = __tbEndings(options.endings);
      type = __tbBlobType(options.type);
    }
    const bytes = __tbBlobBytes(converted, endings);
    Object.defineProperty(this, __tbBlobData, {
      value: { bytes, type: type.toLowerCase() },
      writable: false, enumerable: false, configurable: false,
    });
  }
  get size() { return __tbBrand(this, __tbBlobData).bytes.length; }
  get type() { return __tbBrand(this, __tbBlobData).type; }
  // https://w3c.github.io/FileAPI/#dom-blob-text
  text() {
    const bytes = __tbBrand(this, __tbBlobData).bytes;
    return Promise.resolve(__tbDecodeBytes(bytes, 'utf-8', false, false));
  }
  // https://w3c.github.io/FileAPI/#dom-blob-arraybuffer
  arrayBuffer() {
    const bytes = __tbBrand(this, __tbBlobData).bytes;
    return Promise.resolve(bytes.slice().buffer);
  }
  // https://w3c.github.io/FileAPI/#dom-blob-bytes
  bytes() {
    const bytes = __tbBrand(this, __tbBlobData).bytes;
    return Promise.resolve(bytes.slice());
  }
  // https://w3c.github.io/FileAPI/#dom-blob-slice
  slice(start, end, contentType) {
    const data = __tbBrand(this, __tbBlobData);
    const size = data.bytes.length;
    const hasStart = arguments.length > 0 && start !== undefined;
    const hasEnd = arguments.length > 1 && end !== undefined;
    const hasContentType = arguments.length > 2 && contentType !== undefined;
    let relativeStart = 0;
    if (hasStart) {
      relativeStart = __tbClampRound(start);
      relativeStart = relativeStart < 0 ? Math.max(size + relativeStart, 0) : Math.min(relativeStart, size);
    }
    let relativeEnd = size;
    if (hasEnd) {
      relativeEnd = __tbClampRound(end);
      relativeEnd = relativeEnd < 0 ? Math.max(size + relativeEnd, 0) : Math.min(relativeEnd, size);
    }
    const span = Math.max(relativeEnd - relativeStart, 0);
    let type = '';
    if (hasContentType) {
      type = String(contentType);
      if ([...type].some(character => character < ' ' || character > '~')) type = '';
    }
    return new Blob([data.bytes.subarray(relativeStart, relativeStart + span)], { type });
  }
  // https://w3c.github.io/FileAPI/#dom-blob-stream
  stream() {
    const bytes = __tbBrand(this, __tbBlobData).bytes.slice();
    return new ReadableStream({
      start(controller) {
        if (bytes.length > 0) controller.enqueue(bytes);
        controller.close();
      },
    });
  }
  // https://w3c.github.io/FileAPI/#dom-blob-textstream
  textStream() {
    const text = __tbDecodeBytes(__tbBrand(this, __tbBlobData).bytes, 'utf-8', false, false);
    return new ReadableStream({
      start(controller) {
        if (text.length > 0) controller.enqueue(text);
        controller.close();
      },
    });
  }
};
Object.defineProperty(globalThis.Blob.prototype, Symbol.toStringTag, { value: 'Blob', writable: false, enumerable: false, configurable: true });
// Web IDL constructor objects: `length` is the number of required arguments
// (<https://webidl.spec.whatwg.org/#es-interface>).
Object.defineProperty(globalThis.Blob, 'length', { value: 0, writable: false, enumerable: false, configurable: true });
// https://w3c.github.io/FileAPI/#file-section
globalThis.File = class File extends Blob {
  constructor(fileBits, fileName, options) {
    if (arguments.length < 2) throw new TypeError('The File constructor requires 2 arguments');
    // fileBits converts before the options dictionary, and FilePropertyBag
    // members evaluate in lexicographic order: endings, lastModified, type.
    const converted = __tbConvertBlobParts(fileBits);
    const name = String(fileName);
    let endings = 'transparent';
    let type = '';
    let lastModified = Date.now();
    if (options !== undefined && options !== null) {
      if (typeof options !== 'object' && typeof options !== 'function') {
        throw new TypeError('The options argument must be a property bag');
      }
      endings = __tbEndings(options.endings);
      if (options.lastModified !== undefined) {
        // A long long: ToInt64 maps NaN and infinities to 0
        // (<https://webidl.spec.whatwg.org/#abstract-opdef-converttoint>).
        lastModified = Number(options.lastModified);
        lastModified = Number.isFinite(lastModified) ? Math.trunc(lastModified) : 0;
      }
      type = __tbBlobType(options.type);
    }
    super([__tbBlobBytes(converted, endings)], { endings, type });
    Object.defineProperty(this, __tbFileData, {
      value: { name, lastModified },
      writable: false, enumerable: false, configurable: false,
    });
  }
  get name() { return __tbBrand(this, __tbFileData).name; }
  get lastModified() { return __tbBrand(this, __tbFileData).lastModified; }
  get webkitRelativePath() { __tbBrand(this, __tbFileData); return ''; }
};
Object.defineProperty(globalThis.File.prototype, Symbol.toStringTag, { value: 'File', writable: false, enumerable: false, configurable: true });
Object.defineProperty(globalThis.File, 'length', { value: 2, writable: false, enumerable: false, configurable: true });
// https://w3c.github.io/FileAPI/#filelist-section
globalThis.FileList = class FileList {
  constructor() { throw new TypeError('Illegal constructor'); }
  get length() { return __tbBrand(this, __tbFileListData).files.length; }
  item(index) {
    const files = __tbBrand(this, __tbFileListData).files;
    index = Number(index);
    return index >= 0 && index < files.length ? files[index] : null;
  }
  [Symbol.iterator]() {
    return __tbBrand(this, __tbFileListData).files[Symbol.iterator]();
  }
};
Object.defineProperty(globalThis.FileList.prototype, Symbol.toStringTag, { value: 'FileList', writable: false, enumerable: false, configurable: true });
globalThis.__tbCreateFileList = files => {
  const list = Object.create(globalThis.FileList.prototype);
  Object.defineProperty(list, __tbFileListData, {
    value: { files: Array.from(files) },
    writable: false, enumerable: false, configurable: false,
  });
  return list;
};
// https://w3c.github.io/FileAPI/#APIASynch
globalThis.FileReader = class FileReader extends EventTarget {
  constructor() {
    super();
    Object.defineProperty(this, __tbReaderData, {
      value: { state: 0, result: null, error: null, generation: 0, handlers: {} },
      writable: false, enumerable: false, configurable: false,
    });
  }
  get readyState() { return __tbBrand(this, __tbReaderData).state; }
  get result() { return __tbBrand(this, __tbReaderData).result; }
  get error() { return __tbBrand(this, __tbReaderData).error; }
  // https://w3c.github.io/FileAPI/#dfn-abort
  abort() {
    const data = __tbBrand(this, __tbReaderData);
    if (data.state !== 1) return;
    data.state = 2;
    data.result = null;
    const generation = ++data.generation;
    this.dispatchEvent(new ProgressEvent('abort'));
    // A read started by the abort handler suppresses the old loadend
    // (<https://w3c.github.io/FileAPI/#fileReaderAbort>).
    if (data.generation === generation) {
      this.dispatchEvent(new ProgressEvent('loadend'));
    }
  }
  readAsText(blob, encoding) { __tbFileReaderRead(this, blob, 'text', encoding); }
  readAsArrayBuffer(blob) { __tbFileReaderRead(this, blob, 'arraybuffer', undefined); }
  readAsDataURL(blob) { __tbFileReaderRead(this, blob, 'dataurl', undefined); }
  readAsBinaryString(blob) { __tbFileReaderRead(this, blob, 'binarystring', undefined); }
};
for (const [name, value] of [['EMPTY', 0], ['LOADING', 1], ['DONE', 2]]) {
  for (const target of [globalThis.FileReader, globalThis.FileReader.prototype]) {
    Object.defineProperty(target, name, { value, writable: false, enumerable: true, configurable: false });
  }
}
for (const name of ['onloadstart', 'onprogress', 'onload', 'onabort', 'onerror', 'onloadend']) {
  Object.defineProperty(globalThis.FileReader.prototype, name, {
    get() {
      const handler = __tbBrand(this, __tbReaderData).handlers[name];
      return handler === undefined ? null : handler;
    },
    set(value) { __tbBrand(this, __tbReaderData).handlers[name] = value; },
    enumerable: true, configurable: true,
  });
}
const __tbSniffBOM = bytes => {
  if (bytes.length >= 3 && bytes[0] === 0xef && bytes[1] === 0xbb && bytes[2] === 0xbf) return 'utf-8';
  if (bytes.length >= 2 && bytes[0] === 0xfe && bytes[1] === 0xff) return 'utf-16be';
  if (bytes.length >= 2 && bytes[0] === 0xff && bytes[1] === 0xfe) return 'utf-16le';
  return null;
};
const __tbFileReaderRead = (reader, blob, kind, argument) => {
  const data = __tbBrand(reader, __tbReaderData);
  if (data.state === 1) {
    throw new DOMException('The FileReader is already reading.', 'InvalidStateError');
  }
  const source = __tbBrand(blob, __tbBlobData);
  data.state = 1;
  data.result = null;
  data.error = null;
  const generation = ++data.generation;
  const total = source.bytes.length;
  const fire = (type, loaded, lengthComputable) => reader.dispatchEvent(new ProgressEvent(type, {
    lengthComputable, loaded, total,
  }));
  const stale = () => data.generation !== generation;
  // Every event runs in its own task, so awaiting tests observe the order the
  // spec queues and abort() can land between steps
  // (<https://w3c.github.io/FileAPI/#readOperation>).
  setTimeout(function() {
    if (stale()) return;
    fire('loadstart', 0, true);
    setTimeout(function() {
      if (stale()) return;
      if (total > 0) fire('progress', total, true);
      setTimeout(function() {
        if (stale()) return;
        let result = null;
        let error = null;
        try {
          if (kind === 'text') {
            // Encoding: explicit argument, else the blob type's charset, else
            // UTF-8; a BOM overrides (<https://w3c.github.io/FileAPI/#readAsDataText>).
            let encoding;
            if (argument !== undefined) {
              const label = __tbEncoding(String(argument));
              if (label !== null) encoding = label;
            }
            if (encoding === undefined) {
              const charset = /charset=['\u0022]?([^;\u0022\s]+)/i.exec(source.type);
              if (charset) {
                const label = __tbEncoding(charset[1]);
                if (label !== null) encoding = label;
              }
            }
            if (encoding === undefined) encoding = 'utf-8';
            const bom = __tbSniffBOM(source.bytes);
            result = __tbDecodeBytes(source.bytes, bom === null ? encoding : bom, false, false);
          } else if (kind === 'dataurl') {
            const type = source.type === '' ? 'application/octet-stream' : source.type;
            result = 'data:' + type + ';base64,' + __tbBase64Encode(source.bytes);
          } else if (kind === 'arraybuffer') {
            result = source.bytes.slice().buffer;
          } else {
            result = '';
            for (let index = 0; index < source.bytes.length; index++) {
              result += String.fromCharCode(source.bytes[index]);
            }
          }
        } catch (exception) {
          error = exception;
        }
        if (stale()) return;
        data.state = 2;
        if (error !== null) {
          data.error = error;
          fire('error', 0, false);
        } else {
          data.result = result;
          fire('load', total, true);
        }
        setTimeout(function() {
          if (stale()) return;
          fire('loadend', total, true);
        }, 0);
      }, 0);
    }, 0);
  }, 0);
};
Object.defineProperty(globalThis.FileReader.prototype, Symbol.toStringTag, { value: 'FileReader', writable: false, enumerable: false, configurable: true });
// https://w3c.github.io/FileAPI/#dfn-createObjectURL
globalThis.URL = class URL {
  constructor(input, base) {
    // No base means no document fallback: the input must parse absolutely
    // (<https://url.spec.whatwg.org/#concept-url-parser>).
    const href = base === undefined
      ? globalThis.__tbParseUrl(String(input))
      : globalThis.__tbResolveUrl(String(input), String(base));
    if (href === null) throw new TypeError('Invalid URL');
    this.href = href;
    this._searchParams = new URLSearchParams(this.search);
    this._searchParams._sync = value => {
      const hash = this.hash;
      const base = this.href.split(/[?#]/, 1)[0];
      this.href = base + (value ? '?' + value : '') + hash;
    };
  }
  toString() { return this.href; }
  toJSON() { return this.href; }
  get search() {
    const match = this.href.match(/\?[^#]*/);
    return match ? match[0] : '';
  }
  get hash() {
    const index = this.href.indexOf('#');
    return index < 0 ? '' : this.href.slice(index);
  }
  get origin() {
    const match = this.href.match(/^([a-z][a-z0-9+.-]*:\/\/[^/]+)/i);
    if (!match) return 'null';
    const prefix = match[1];
    const authority = prefix.slice(prefix.indexOf('//') + 2);
    return prefix.slice(0, prefix.indexOf('//') + 2) + authority.slice(authority.lastIndexOf('@') + 1);
  }
  get protocol() { return this.href.slice(0, this.href.indexOf(':') + 1); }
  get searchParams() { return this._searchParams; }
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
          this._pairs.push([String(values[0]), String(values[1])]);
        }
      } else {
        for (const name of Object.keys(init)) this._pairs.push([name, String(init[name])]);
      }
      return;
    }
    var input = String(init === undefined ? '' : init);
    if (input.charAt(0) === '?') input = input.slice(1);
    if (!input) return;
    const decode = value => {
      value = value.replace(/\+/g, ' ');
      try { return decodeURIComponent(value); }
      catch (_) { return value.replace(/%([0-9a-f]{2})/gi, (_m, hex) => String.fromCharCode(parseInt(hex, 16))); }
    };
    for (const item of input.split('&')) {
      const separator = item.indexOf('=');
      const name = separator < 0 ? item : item.slice(0, separator);
      const value = separator < 0 ? '' : item.slice(separator + 1);
      this._pairs.push([
        decode(name),
        decode(value)
      ]);
    }
  }
  get(name) {
    name = String(name);
    for (const pair of this._pairs) {
      if (pair[0] === name) return pair[1];
    }
    return null;
  }
  getAll(name) {
    name = String(name);
    return this._pairs.filter(pair => pair[0] === name).map(pair => pair[1]);
  }
  has(name, value) {
    name = String(name);
    if (arguments.length < 2) return this._pairs.some(pair => pair[0] === name);
    value = String(value);
    return this._pairs.some(pair => pair[0] === name && pair[1] === value);
  }
  append(name, value) { this._pairs.push([String(name), String(value)]); if (this._sync) this._sync(this.toString()); }
  set(name, value) {
    name = String(name);
    value = String(value);
    const index = this._pairs.findIndex(pair => pair[0] === name);
    if (index < 0) {
      this._pairs.push([name, value]);
      if (this._sync) this._sync(this.toString());
      return;
    }
    this._pairs[index][1] = value;
    this._pairs = this._pairs.filter((pair, current) => pair[0] !== name || current === index);
    if (this._sync) this._sync(this.toString());
  }
  delete(name, value) {
    name = String(name);
    if (arguments.length < 2) {
      this._pairs = this._pairs.filter(pair => pair[0] !== name);
    } else {
      value = String(value);
      this._pairs = this._pairs.filter(pair => pair[0] !== name || pair[1] !== value);
    }
    if (this._sync) this._sync(this.toString());
  }
  sort() {
    this._pairs = this._pairs.map((pair, index) => ({ pair, index }))
      .sort((a, b) => a.pair[0] < b.pair[0] ? -1 : a.pair[0] > b.pair[0] ? 1 : a.index - b.index)
      .map(entry => entry.pair);
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

globalThis.__tbMakeDataset = element => new Proxy(Object.create(null), {
  get(_target, property) {
    if (property === Symbol.toStringTag) return 'DOMStringMap';
    if (typeof property !== 'string') return undefined;
    const name = 'data-' + property.replace(/[A-Z]/g, letter => '-' + letter.toLowerCase());
    const value = element.getAttribute(name);
    return value === null ? undefined : value;
  },
  set(_target, property, value) {
    property = String(property);
    if (/-[a-z]/.test(property)) throw new DOMException('invalid dataset property', 'SyntaxError');
    const name = 'data-' + property.replace(/[A-Z]/g, letter => '-' + letter.toLowerCase());
    element.setAttribute(name, String(value));
    return true;
  },
  deleteProperty(_target, property) {
    property = String(property);
    const name = 'data-' + property.replace(/[A-Z]/g, letter => '-' + letter.toLowerCase());
    element.removeAttribute(name);
    return true;
  },
  ownKeys() {
    return element.getAttributeNames().filter(name =>
      name.startsWith('data-') && !/[A-Z]/.test(name.slice(5))
    ).map(name => name.slice(5).replace(/-([a-z])/g, (_match, letter) => letter.toUpperCase()));
  },
  getOwnPropertyDescriptor(_target, property) {
    const value = this.get(_target, property);
    if (value === undefined) return undefined;
    return { configurable: true, enumerable: true, writable: true, value };
  }
});

globalThis.__tbMakeStyle = element => {
  const splitDeclarations = value => {
    const parts = []; let start = 0; let quote = '';
    for (let i = 0; i < value.length; i++) {
      const character = value[i];
      if (quote) { if (character === quote && value[i - 1] !== '\\') quote = ''; }
      else if (character === String.fromCharCode(34) || character === String.fromCharCode(39)) quote = character;
      else if (character === ';') { parts.push(value.slice(start, i)); start = i + 1; }
    }
    parts.push(value.slice(start));
    return parts;
  };
  const read = () => {
    const declarations = [];
    for (const part of splitDeclarations(element.getAttribute('style') || '')) {
      const separator = part.indexOf(':');
      if (separator < 0) continue;
      const name = part.slice(0, separator).trim().toLowerCase();
      if (name) declarations.push([name, part.slice(separator + 1).trim()]);
    }
    return declarations;
  };
  const write = declarations => {
    const value = declarations.map(pair => pair[0] + ': ' + pair[1] + ';').join(' ');
    if (value) element.setAttribute('style', value);
    else element.removeAttribute('style');
  };
  const propertyName = property =>
    String(property).replace(/[A-Z]/g, letter => '-' + letter.toLowerCase());
  const target = {
    get cssText() { return element.getAttribute('style') || ''; },
    set cssText(value) { element.setAttribute('style', String(value)); },
    get length() { return read().length; },
    item(index) {
      const pair = read()[Number(index)];
      return pair ? pair[0] : '';
    },
    getPropertyValue(name) {
      const pair = read().find(item => item[0] === String(name).toLowerCase());
      return pair ? pair[1] : '';
    },
    setProperty(name, value) {
      name = String(name).toLowerCase();
      value = String(value);
      const declarations = read().filter(pair => pair[0] !== name);
      if (value) declarations.push([name, value]);
      write(declarations);
    },
    removeProperty(name) {
      name = String(name).toLowerCase();
      const old = this.getPropertyValue(name);
      write(read().filter(pair => pair[0] !== name));
      return old;
    }
  };
  return new Proxy(target, {
    get(target, property, receiver) {
      if (Reflect.has(target, property)) return Reflect.get(target, property, receiver);
      if (typeof property !== 'string') return undefined;
      return target.getPropertyValue(propertyName(property));
    },
    set(target, property, value, receiver) {
      if (Reflect.has(target, property)) return Reflect.set(target, property, value, receiver);
      target.setProperty(propertyName(property), value);
      return true;
    }
  });
};
// https://html.spec.whatwg.org/multipage/web-messaging.html#messageevent
const __tbMessageEventData = Symbol.for('tinybrowser.messageevent.data');
globalThis.MessageEvent = class MessageEvent extends Event {
  constructor(type, init) {
    const eventInit = init === undefined ? {} : Object(init);
    super(String(type), eventInit);
    Object.defineProperty(this, __tbMessageEventData, {
      value: {
        data: Object.prototype.hasOwnProperty.call(eventInit, 'data') ? eventInit.data : null,
        origin: eventInit.origin === undefined ? '' : String(eventInit.origin),
        lastEventId: eventInit.lastEventId === undefined ? '' : String(eventInit.lastEventId),
        source: eventInit.source === undefined ? null : eventInit.source,
        ports: eventInit.ports === undefined ? Object.freeze([]) : Object.freeze(Array.from(eventInit.ports)),
      },
      writable: false, enumerable: false, configurable: false,
    });
  }
  get data() { return __tbBrand(this, __tbMessageEventData).data; }
  get origin() { return __tbBrand(this, __tbMessageEventData).origin; }
  get lastEventId() { return __tbBrand(this, __tbMessageEventData).lastEventId; }
  get source() { return __tbBrand(this, __tbMessageEventData).source; }
  get ports() { return __tbBrand(this, __tbMessageEventData).ports; }
};
Object.defineProperty(globalThis.MessageEvent.prototype, Symbol.toStringTag, { value: 'MessageEvent', writable: false, enumerable: false, configurable: true });
// https://html.spec.whatwg.org/multipage/structured-data.html#structuredserializeinternal
// A same-realm structured clone. Cross-thread workers will move this seam to
// Rust; every messaging API already routes through it.
const __tbTransferList = transfer => {
  if (transfer === undefined) return [];
  // `sequence<object>` conversion: an object with @@iterator, else TypeError
  // (<https://webidl.spec.whatwg.org/#es-sequence>).
  if (transfer === null || typeof transfer !== 'object' || typeof transfer[Symbol.iterator] !== 'function') {
    throw new TypeError('The transfer list must be an iterable object');
  }
  return Array.from(transfer);
};
const __tbStructuredClone = (value, transfer, sourcePort) => {
  const buffers = new Map();
  const ports = new Map();
  const portList = [];
  for (const item of __tbTransferList(transfer)) {
    if (item instanceof ArrayBuffer) {
      if (buffers.has(item)) throw new DOMException('Transfer list contains duplicate buffers', 'DataCloneError');
      if (typeof item.transfer !== 'function') throw new DOMException('The buffer is not transferable', 'DataCloneError');
      let moved;
      try {
        moved = item.transfer();
      } catch (error) {
        throw new DOMException('The buffer is already detached', 'DataCloneError');
      }
      buffers.set(item, moved);
    } else if (item instanceof globalThis.MessagePort) {
      if (item === sourcePort) {
        throw new DOMException('Cannot transfer the source port', 'DataCloneError');
      }
      const itemData = __tbBrand(item, __tbPortData);
      if (itemData.closed) throw new DOMException('Cannot transfer a detached MessagePort', 'DataCloneError');
      if (ports.has(item)) throw new DOMException('Transfer list contains duplicate ports', 'DataCloneError');
      const moved = __tbNewPort();
      const movedData = __tbBrand(moved, __tbPortData);
      movedData.peer = itemData.peer;
      if (movedData.peer !== null) __tbBrand(movedData.peer, __tbPortData).peer = moved;
      // The message queue moves with the port identity; the new port starts
      // disabled and flushes when enabled
      // (<https://html.spec.whatwg.org/multipage/web-messaging.html#message-ports>).
      movedData.pending = itemData.pending;
      itemData.pending = [];
      itemData.peer = null;
      itemData.closed = true;
      itemData.started = false;
      ports.set(item, moved);
      portList.push(moved);
    } else {
      throw new DOMException('Value not transferable', 'DataCloneError');
    }
  }
  const seen = new Map();
  const clone = input => {
    if (input === null || input === undefined) return input;
    const kind = typeof input;
    if (kind === 'function' || kind === 'symbol') {
      throw new DOMException('The object could not be cloned.', 'DataCloneError');
    }
    if (kind !== 'object') return input;
    if (buffers.has(input)) return buffers.get(input);
    if (ports.has(input)) return ports.get(input);
    if (seen.has(input)) return seen.get(input);
    if (input === globalThis || input === globalThis.window) {
      throw new DOMException('The object could not be cloned.', 'DataCloneError');
    }
    if (input instanceof ArrayBuffer) {
      const copy = input.slice(0);
      seen.set(input, copy);
      return copy;
    }
    if (typeof SharedArrayBuffer !== 'undefined' && input instanceof SharedArrayBuffer) return input;
    if (ArrayBuffer.isView(input)) {
      const buffer = clone(input.buffer);
      const copy = input instanceof DataView
        ? new DataView(buffer, input.byteOffset, input.byteLength)
        : new input.constructor(buffer, input.byteOffset, input.length);
      seen.set(input, copy);
      return copy;
    }
    // Boxed primitives clone to boxed copies with the same value
    // (<https://html.spec.whatwg.org/multipage/structured-data.html#structuredserializeinternal>).
    if (input instanceof Boolean || input instanceof Number || input instanceof String) {
      const copy = new input.constructor(input.valueOf());
      seen.set(input, copy);
      return copy;
    }
    if (input instanceof Blob) {
      const blob = input[__tbBlobData];
      let copy;
      if (input instanceof globalThis.File) {
        const file = input[__tbFileData];
        copy = new globalThis.File([blob.bytes], file.name, { type: blob.type, lastModified: file.lastModified });
      } else {
        copy = new Blob([blob.bytes], { type: blob.type });
      }
      seen.set(input, copy);
      return copy;
    }
    if (input instanceof Date) {
      const copy = new Date(input.getTime());
      seen.set(input, copy);
      return copy;
    }
    if (input instanceof RegExp) {
      const copy = new RegExp(input.source, input.flags);
      seen.set(input, copy);
      return copy;
    }
    if (input instanceof Error) {
      const copy = new Error(input.message);
      copy.name = input.name;
      seen.set(input, copy);
      return copy;
    }
    if (input instanceof Map) {
      const copy = new Map();
      seen.set(input, copy);
      for (const [key, entry] of input) copy.set(clone(key), clone(entry));
      return copy;
    }
    if (input instanceof Set) {
      const copy = new Set();
      seen.set(input, copy);
      for (const entry of input) copy.add(clone(entry));
      return copy;
    }
    if (Array.isArray(input)) {
      // Arrays carry their length and only their index properties
      // (<https://html.spec.whatwg.org/multipage/structured-data.html#structuredserializeinternal>).
      const copy = new Array(input.length);
      seen.set(input, copy);
      for (const key of Object.keys(input)) {
        const index = Number(key);
        if (Number.isInteger(index) && index >= 0 && index < input.length && String(index) === key) {
          copy[index] = clone(input[key]);
        }
      }
      return copy;
    }
    // Anything left is a host object unless it is a plain object: Rust class
    // instances (events, nodes, URL, ...) carry a platform prototype and are
    // not serializable
    // (<https://html.spec.whatwg.org/multipage/structured-data.html#structuredserializeinternal>).
    const proto = Object.getPrototypeOf(input);
    if (proto !== Object.prototype && proto !== null) {
      throw new DOMException('The object could not be cloned.', 'DataCloneError');
    }
    const copy = {};
    seen.set(input, copy);
    for (const key of Object.keys(input)) copy[key] = clone(input[key]);
    return copy;
  };
  return { data: clone(value), ports: portList };
};
// https://html.spec.whatwg.org/multipage/structured-data.html#dom-structuredclone
globalThis.structuredClone = function(value, options) {
  const transfer = options === undefined || options === null ? undefined : options.transfer;
  return __tbStructuredClone(value, transfer, null).data;
};
// https://html.spec.whatwg.org/multipage/web-messaging.html#messageport
const __tbPortData = Symbol.for('tinybrowser.messageport.data');
const __tbNewPort = () => {
  // Construct through the host EventTarget so the port carries the listener
  // storage its addEventListener/dispatchEvent require.
  const port = Reflect.construct(globalThis.EventTarget, [], globalThis.MessagePort);
  Object.defineProperty(port, __tbPortData, {
    value: { peer: null, started: false, closed: false, onmessage: null, onmessageerror: null, pending: [] },
    writable: false, enumerable: false, configurable: false,
  });
  return port;
};
const __tbPortDeliver = (port, cloned) => {
  const data = __tbBrand(port, __tbPortData);
  if (data.closed || !data.started) return;
  port.__tbDispatchTrusted(new globalThis.MessageEvent('message', { data: cloned.data, ports: cloned.ports }));
};
const __tbPortFlush = port => {
  const data = __tbBrand(port, __tbPortData);
  while (data.pending.length > 0) {
    const cloned = data.pending.shift();
    setTimeout(function() { __tbPortDeliver(port, cloned); }, 0);
  }
};
const __tbPortEnqueue = (port, cloned) => {
  const peer = __tbBrand(port, __tbPortData).peer;
  if (peer === null) return;
  const peerData = __tbBrand(peer, __tbPortData);
  if (peerData.closed) return;
  if (peerData.started) setTimeout(function() { __tbPortDeliver(peer, cloned); }, 0);
  else peerData.pending.push(cloned);
};
globalThis.MessagePort = class MessagePort extends EventTarget {
  constructor() { throw new TypeError('Illegal constructor'); }
  postMessage(message, transfer) {
    const data = __tbBrand(this, __tbPortData);
    if (data.closed) return;
    // `postMessage(message, options)` dictionary overload
    // (<https://html.spec.whatwg.org/multipage/web-messaging.html#dom-messageport-postmessage>).
    if (transfer !== null && typeof transfer === 'object' && !Array.isArray(transfer)
        && typeof transfer[Symbol.iterator] !== 'function' && 'transfer' in transfer) {
      transfer = transfer.transfer;
    }
    __tbPortEnqueue(this, __tbStructuredClone(message, transfer, this));
  }
  start() {
    const data = __tbBrand(this, __tbPortData);
    if (data.started) return;
    data.started = true;
    __tbPortFlush(this);
  }
  close() {
    const data = __tbBrand(this, __tbPortData);
    data.closed = true;
    data.started = false;
    data.pending.length = 0;
  }
  get onmessage() { return __tbBrand(this, __tbPortData).onmessage; }
  set onmessage(value) {
    __tbBrand(this, __tbPortData).onmessage = value;
    if (value !== null && value !== undefined) this.start();
  }
  get onmessageerror() { return __tbBrand(this, __tbPortData).onmessageerror; }
  set onmessageerror(value) { __tbBrand(this, __tbPortData).onmessageerror = value; }
};
Object.defineProperty(globalThis.MessagePort.prototype, Symbol.toStringTag, { value: 'MessagePort', writable: false, enumerable: false, configurable: true });
// https://html.spec.whatwg.org/multipage/web-messaging.html#messagechannel
globalThis.MessageChannel = class MessageChannel {
  constructor() {
    const port1 = __tbNewPort();
    const port2 = __tbNewPort();
    __tbBrand(port1, __tbPortData).peer = port2;
    __tbBrand(port2, __tbPortData).peer = port1;
    Object.defineProperty(this, 'port1', { value: port1, writable: false, enumerable: true, configurable: true });
    Object.defineProperty(this, 'port2', { value: port2, writable: false, enumerable: true, configurable: true });
  }
};
Object.defineProperty(globalThis.MessageChannel.prototype, Symbol.toStringTag, { value: 'MessageChannel', writable: false, enumerable: false, configurable: true });
// https://html.spec.whatwg.org/multipage/web-messaging.html#dom-window-postmessage
// Same-window delivery: `source` is this window. The spec order is kept:
// resolve targetOrigin, then structured-serialize, then queue the task, and
// the origin check runs inside the task. Deviation: the task is queued on the
// timer task source; there is no posted-message task source yet.
globalThis.postMessage = function(message, targetOrigin, transfer) {
  if (arguments.length === 0) {
    throw new TypeError(`Failed to execute 'postMessage' on 'Window': 1 argument required, but only 0 present.`);
  }
  // `postMessage(message, options)` dictionary overload
  // (<https://html.spec.whatwg.org/multipage/web-messaging.html#dom-window-postmessage>).
  if (targetOrigin !== null && typeof targetOrigin === 'object') {
    transfer = targetOrigin.transfer;
    targetOrigin = targetOrigin.targetOrigin;
  }
  const sourceOrigin = String(location.origin);
  let checkedOrigin = targetOrigin === undefined ? '/' : String(targetOrigin);
  if (checkedOrigin === '/') {
    checkedOrigin = sourceOrigin;
  } else if (checkedOrigin !== '*') {
    let parsed;
    try {
      parsed = new URL(checkedOrigin);
    } catch (error) {
      throw new DOMException('Invalid target origin', 'SyntaxError');
    }
    checkedOrigin = parsed.origin;
  }
  const cloned = __tbStructuredClone(message, transfer, null);
  const ports = cloned.ports;
  setTimeout(function() {
    if (checkedOrigin !== '*' && checkedOrigin !== sourceOrigin) return;
    globalThis.__tbDispatchTrusted(new globalThis.MessageEvent('message', {
      data: cloned.data,
      origin: sourceOrigin,
      source: globalThis,
      ports: ports,
    }));
  }, 0);
};
";

/// A value produced by script evaluation.
#[derive(Clone, Debug, PartialEq)]
pub enum ScriptValue {
    /// JS `undefined`.
    Undefined,
    /// JS `null`.
    Null,
    /// JS boolean.
    Bool(bool),
    /// JS number.
    Number(f64),
    /// JS string.
    String(String),
    /// A DOM node handle for `WebDriver` element encoding.
    Node(dom::NodeId),
    /// A JS array, for `WebDriver` JSON.
    List(Vec<ScriptValue>),
    /// A JS object, for `WebDriver` JSON.
    Map(Vec<(String, ScriptValue)>),
}

/// A classic `<script>` in document order.
///
/// [HTML prepare the script element](https://html.spec.whatwg.org/multipage/webappapis.html#prepare-the-script-element)
#[derive(Clone)]
pub(crate) enum ClassicScript {
    Inline(String),
    Src(String),
}

#[derive(Debug)]
pub(crate) enum JsError {
    Engine(Box<str>),
    Interrupted,
    BadTimerId,
}

impl JsError {
    fn engine(err: impl fmt::Display) -> Self {
        Self::Engine(err.to_string().into_boxed_str())
    }
}

impl fmt::Display for JsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Engine(message) => f.write_str(message),
            Self::Interrupted => f.write_str("interrupted"),
            Self::BadTimerId => f.write_str("bad timer id"),
        }
    }
}

impl std::error::Error for JsError {}

pub(crate) struct PendingTimeout {
    pub delay: Duration,
    pub js_id: i32,
}

pub(crate) struct PendingJsFetch {
    pub url: String,
    pub js_id: i32,
}

/// One renderer process's `QuickJS` heap, created on first use and shared by
/// every frame in the process.
///
/// Several `QuickJS` contexts may share one runtime and its objects, "similar to
/// frames of the same origin sharing JavaScript objects in a web browser"
/// (<https://bellard.org/quickjs/quickjs.html>, JSRuntime): the same-site-frame
/// model.
/// The creation result is cached so a heap that cannot start fails every realm
/// the same way instead of retrying.
#[derive(Clone, Default)]
pub(crate) struct SharedJsRuntime(Rc<OnceCell<Result<Runtime, Box<str>>>>);

impl SharedJsRuntime {
    pub(crate) fn get(&self) -> Result<&Runtime, JsError> {
        let slot = self
            .0
            .get_or_init(|| Runtime::new().map_err(|err| err.to_string().into_boxed_str()));
        slot.as_ref()
            .map_err(|message| JsError::Engine(message.clone()))
    }
}

pub(crate) struct JsRealm {
    runtime: Runtime,
    context: Context,
    world: Rc<RefCell<World>>,
    stop: Arc<Stop>,
    pending_timeouts: Rc<RefCell<Vec<PendingTimeout>>>,
    pending_fetches: Rc<RefCell<Vec<PendingJsFetch>>>,
}

impl JsRealm {
    pub(crate) fn new(
        shared: &SharedJsRuntime,
        world: Rc<RefCell<World>>,
        stop: Arc<Stop>,
    ) -> Result<Self, JsError> {
        let runtime = shared.get()?.clone();
        runtime.set_memory_limit(MAX_RUNTIME_MEMORY);
        runtime.set_max_stack_size(MAX_RUNTIME_STACK);
        let context = Context::full(&runtime).map_err(JsError::engine)?;
        let host = Self {
            runtime,
            context,
            world,
            stop,
            pending_timeouts: Rc::new(RefCell::new(Vec::new())),
            pending_fetches: Rc::new(RefCell::new(Vec::new())),
        };
        host.install()?;
        Ok(host)
    }

    pub(crate) fn eval(&self, source: &str) -> Result<String, JsError> {
        self.with_budget(None, || {
            let rendered: Result<String, JsError> = self.context.with(|ctx| {
                let value: Value = eval_classic(&ctx, source)?;
                render_eval_result(&ctx, value)
            });
            // https://html.spec.whatwg.org/multipage/webappapis.html#clean-up-after-running-script
            let jobs = self.run_jobs();
            rendered.and_then(|out| jobs.map(|()| out))
        })
    }

    pub(crate) fn eval_value_deadline(
        &self,
        source: &str,
        deadline: Option<Instant>,
    ) -> Result<crate::js::ScriptValue, JsError> {
        self.with_budget(deadline, || {
            let decoded: Result<ScriptValue, JsError> = self.context.with(|ctx| {
                let value: Value = eval_classic(&ctx, source)?;
                decode_value(&ctx, value)
            });
            let jobs = self.run_jobs();
            decoded.and_then(|value| jobs.map(|()| value))
        })
    }

    pub(crate) fn take_pending_timeouts(&self) -> Vec<PendingTimeout> {
        std::mem::take(&mut *self.pending_timeouts.borrow_mut())
    }

    pub(crate) fn take_pending_fetches(&self) -> Vec<PendingJsFetch> {
        std::mem::take(&mut *self.pending_fetches.borrow_mut())
    }

    pub(crate) fn take_pending_cancels(&self) -> Vec<i32> {
        std::mem::take(&mut self.world.borrow_mut().pending_cancels)
    }

    pub(crate) fn fire_timer(&self, js_id: i32) -> Result<(), JsError> {
        self.with_budget(None, || {
            let called: Result<(), JsError> = self.context.with(|ctx| {
                let timeouts: Array = ctx
                    .globals()
                    .get("__tb_timeouts")
                    .map_err(JsError::engine)?;
                let idx = usize::try_from(js_id).map_err(|_| JsError::BadTimerId)?;
                let func: Function = timeouts.get(idx).map_err(JsError::engine)?;
                timeouts
                    .as_object()
                    .remove(js_id)
                    .map_err(JsError::engine)?;
                func.call(()).map_err(JsError::engine)
            });
            let jobs = self.run_jobs();
            called.and(jobs)
        })
    }

    pub(crate) fn finish_js_fetch(
        &self,
        js_id: i32,
        ok: bool,
        status: i32,
        body: &str,
    ) -> Result<(), JsError> {
        self.with_budget(None, || {
            let body = body.to_owned();
            let called: Result<(), JsError> = self.context.with(|ctx| {
                let cbs: Object = ctx
                    .globals()
                    .get("__tb_fetchCbs")
                    .map_err(JsError::engine)?;
                let func: Function = cbs.get(js_id).map_err(JsError::engine)?;
                func.call((ok, status, body)).map_err(JsError::engine)
            });
            let jobs = self.run_jobs();
            called.and(jobs)
        })
    }

    pub(crate) fn fire_load(&self) -> Result<(), JsError> {
        self.with_budget(None, || {
            let fired: Result<(), JsError> = self
                .context
                .with(|ctx| bindings::fire_window_load(&ctx).map_err(JsError::engine));
            let jobs = self.run_jobs();
            fired.and(jobs)
        })
    }

    /// Fires `DOMContentLoaded` at the document
    /// (<https://html.spec.whatwg.org/multipage/parsing.html#the-end>).
    pub(crate) fn fire_dom_content_loaded(&self) -> Result<(), JsError> {
        self.with_budget(None, || {
            let fired: Result<(), JsError> = self
                .context
                .with(|ctx| bindings::fire_dom_content_loaded(&ctx).map_err(JsError::engine));
            let jobs = self.run_jobs();
            fired.and(jobs)
        })
    }

    /// Fires `readystatechange` after a document readiness change.
    pub(crate) fn fire_ready_state_change(&self) -> Result<(), JsError> {
        self.with_budget(None, || {
            let fired: Result<(), JsError> = self
                .context
                .with(|ctx| bindings::fire_ready_state_change(&ctx).map_err(JsError::engine));
            let jobs = self.run_jobs();
            fired.and(jobs)
        })
    }

    pub(crate) fn fire_node_load(&self, id: dom::NodeId) -> Result<(), JsError> {
        self.with_budget(None, || {
            let fired: Result<(), JsError> = self
                .context
                .with(|ctx| bindings::fire_node_load(&ctx, id).map_err(JsError::engine));
            let jobs = self.run_jobs();
            fired.and(jobs)
        })
    }

    /// Microtask checkpoint for parser-driven mutations: schedules the
    /// delivery microtask when records are pending and runs the job queue.
    ///
    /// Parser insertions record mutations without entering a JS binding, so
    /// nothing else schedules delivery. Called between parser scripts, where
    /// the spec drains microtasks before the next script runs.
    pub(crate) fn deliver_mutations(&self) -> Result<(), JsError> {
        self.with_budget(None, || {
            let scheduled: Result<(), JsError> = self
                .context
                .with(|ctx| bindings::schedule_mutation_delivery(&ctx).map_err(JsError::engine));
            let jobs = self.run_jobs();
            scheduled.and(jobs)
        })
    }

    fn with_budget<T>(
        &self,
        deadline: Option<Instant>,
        operation: impl FnOnce() -> Result<T, JsError>,
    ) -> Result<T, JsError> {
        let deadline = deadline.unwrap_or_else(|| Instant::now() + DEFAULT_SCRIPT_BUDGET);
        let interrupted = Rc::new(Cell::new(false));
        let flag = Rc::clone(&interrupted);
        let stop = Arc::clone(&self.stop);
        self.runtime.set_interrupt_handler(Some(Box::new(move || {
            let should_interrupt = stop.is_set() || Instant::now() >= deadline;
            flag.set(should_interrupt);
            should_interrupt
        })));
        let _clear = ClearInterrupt {
            runtime: &self.runtime,
        };
        let result = operation();
        if interrupted.get() {
            Err(JsError::Interrupted)
        } else {
            result
        }
    }

    fn run_jobs(&self) -> Result<(), JsError> {
        loop {
            match self.runtime.execute_pending_job() {
                Ok(true) => {}
                Ok(false) => return Ok(()),
                Err(err) => return Err(JsError::engine(err)),
            }
        }
    }

    fn install(&self) -> Result<(), JsError> {
        let world = self.world.clone();
        self.context.with(|ctx| {
            bindings::install(&ctx, &world).map_err(JsError::engine)?;
            intl::install(&ctx).map_err(JsError::engine)?;
            self.install_task_host_functions(&ctx, &world)?;
            Self::install_document_host_functions(&ctx, &world)?;
            ctx.eval::<(), _>(INSTALL_WEB_APIS_JS)
                .map_err(JsError::engine)?;
            Ok(())
        })
    }

    /// Timer and fetch submission hooks the JS shim calls.
    fn install_task_host_functions(
        &self,
        ctx: &Ctx<'_>,
        world: &Rc<RefCell<World>>,
    ) -> Result<(), JsError> {
        let timeouts = self.pending_timeouts.clone();
        let fetches = self.pending_fetches.clone();
        let cancel_world = world.clone();

        ctx.globals()
            .set(
                "__scheduleTimeout",
                Func::from(move |js_id: i32, delay: f64| {
                    timeouts.borrow_mut().push(PendingTimeout {
                        delay: Duration::from_millis(u64::from(millis(delay))),
                        js_id,
                    });
                }),
            )
            .map_err(JsError::engine)?;

        ctx.globals()
            .set(
                "__cancelTimeout",
                Func::from(move |js_id: i32| {
                    cancel_world.borrow_mut().pending_cancels.push(js_id);
                }),
            )
            .map_err(JsError::engine)?;

        ctx.globals()
            .set(
                "__queueFetch",
                Func::from(move |url: String, js_id: i32| {
                    fetches.borrow_mut().push(PendingJsFetch { url, js_id });
                }),
            )
            .map_err(JsError::engine)?;
        Ok(())
    }

    /// Cookie, object URL, and URL resolution hooks the JS shim calls.
    fn install_document_host_functions(
        ctx: &Ctx<'_>,
        world: &Rc<RefCell<World>>,
    ) -> Result<(), JsError> {
        let cookie_get = world.clone();
        let cookie_set = world.clone();
        let object_url_create = world.clone();
        let object_url_revoke = world.clone();
        let object_url_contents = world.clone();
        let object_url_type = world.clone();
        let url_resolve = world.clone();

        ctx.globals()
            .set(
                "__cookieGet",
                Func::from(move || {
                    let world = cookie_get.borrow();
                    world.services.cookies_for(&world.document_url)
                }),
            )
            .map_err(JsError::engine)?;

        ctx.globals()
            .set(
                "__cookieSet",
                Func::from(move |value: String| {
                    let world = cookie_set.borrow();
                    world.services.set_cookie(&value, &world.document_url);
                }),
            )
            .map_err(JsError::engine)?;

        ctx.globals()
            .set(
                "__tbCreateObjectURL",
                Func::from(move |contents: String, content_type: String| {
                    object_url_create
                        .borrow_mut()
                        .create_object_url(contents, content_type)
                }),
            )
            .map_err(JsError::engine)?;

        ctx.globals()
            .set(
                "__tbRevokeObjectURL",
                Func::from(move |url: String| {
                    object_url_revoke.borrow_mut().revoke_object_url(&url);
                }),
            )
            .map_err(JsError::engine)?;

        ctx.globals()
            .set(
                "__tbObjectUrlContents",
                Func::from(move |url: String| {
                    object_url_contents
                        .borrow()
                        .object_url_contents(&url)
                        .map(|contents| contents.to_string())
                }),
            )
            .map_err(JsError::engine)?;

        ctx.globals()
            .set(
                "__tbObjectUrlType",
                Func::from(move |url: String| {
                    object_url_type
                        .borrow()
                        .object_url_type(&url)
                        .map(|content_type| content_type.to_string())
                }),
            )
            .map_err(JsError::engine)?;

        ctx.globals()
            .set(
                "__tbParseUrl",
                Func::from(|input: String| {
                    url::Url::parse(&input).ok().map(|url| url.to_string())
                }),
            )
            .map_err(JsError::engine)?;

        ctx.globals()
            .set(
                "__tbResolveUrl",
                Func::from(move |input: String, base: Option<String>| {
                    let fallback = url_resolve.borrow().document_url.clone();
                    let resolved = match base {
                        Some(base) => url::Url::parse(&base)
                            .ok()
                            .and_then(|base| base.join(&input).ok()),
                        None => url::Url::parse(&input)
                            .ok()
                            .or_else(|| fallback.join(&input).ok()),
                    };
                    resolved.map(|url| url.to_string())
                }),
            )
            .map_err(JsError::engine)?;
        Ok(())
    }
}

impl Drop for JsRealm {
    fn drop(&mut self) {
        bindings::forget_world(&self.context);
        let world = self.world.clone();
        let mut world = world.borrow_mut();
        // Release this realm's cached wrappers and document associations
        // before its QuickJS context goes away; sibling realms keep theirs.
        world.forget_owned_documents();
        world.clear_listeners();
    }
}

pub(crate) fn classic_script_at(world: &World, id: dom::NodeId) -> Option<ClassicScript> {
    let parsed = world.document(id)?;
    if !is_classic_script(&parsed.dom, id) {
        return None;
    }
    match parsed.dom.attribute(id, "src") {
        Some(src) if !src.trim().is_empty() => Some(ClassicScript::Src(src)),
        _ => Some(ClassicScript::Inline(element_text(&parsed.dom, id))),
    }
}

fn is_classic_script(tree: &dom::Dom, id: dom::NodeId) -> bool {
    match tree.get(id).map(|node| node.kind()) {
        Some(dom::NodeKind::Element { name, .. })
            if name.ns == dom::html_namespace()
                && name.local.as_ref().eq_ignore_ascii_case("script") =>
        {
            javascript_mime(tree.attribute(id, "type").as_deref())
        }
        _ => false,
    }
}

fn javascript_mime(typ: Option<&str>) -> bool {
    let Some(typ) = typ.map(str::trim).filter(|typ| !typ.is_empty()) else {
        return true;
    };
    let essence = typ
        .split(';')
        .next()
        .unwrap_or(typ)
        .trim()
        .to_ascii_lowercase();
    // https://mimesniff.spec.whatwg.org/#javascript-mime-type
    matches!(
        essence.as_str(),
        "application/ecmascript"
            | "application/javascript"
            | "application/x-ecmascript"
            | "application/x-javascript"
            | "text/ecmascript"
            | "text/javascript"
            | "text/javascript1.0"
            | "text/javascript1.1"
            | "text/javascript1.2"
            | "text/javascript1.3"
            | "text/javascript1.4"
            | "text/javascript1.5"
            | "text/jscript"
            | "text/livescript"
            | "text/x-ecmascript"
            | "text/x-javascript"
    )
}

fn element_text(tree: &dom::Dom, id: dom::NodeId) -> String {
    let mut text = String::new();
    let mut stack: Vec<_> = tree
        .children(id)
        .map(|kids| kids.copied().collect())
        .unwrap_or_default();
    stack.reverse();
    while let Some(child) = stack.pop() {
        match tree.get(child).map(|node| node.kind()) {
            Some(dom::NodeKind::Text { data }) => text.push_str(data),
            Some(dom::NodeKind::Element { .. }) => {
                if let Some(kids) = tree.children(child) {
                    let mut kids: Vec<_> = kids.copied().collect();
                    kids.reverse();
                    stack.extend(kids);
                }
            }
            _ => {}
        }
    }
    text
}

struct ClearInterrupt<'a> {
    runtime: &'a Runtime,
}

impl Drop for ClearInterrupt<'_> {
    fn drop(&mut self) {
        self.runtime.set_interrupt_handler(None);
    }
}

fn eval_classic<'js, V: FromJs<'js>>(ctx: &rquickjs::Ctx<'js>, source: &str) -> Result<V, JsError> {
    let mut options = EvalOptions::default();
    options.strict = false;
    ctx.eval_with_options(source, options)
        .map_err(|error| match error {
            rquickjs::Error::Exception => {
                let caught = ctx.catch();
                JsError::engine(match caught.into_exception() {
                    Some(exception) => format!(
                        "{} | {}",
                        exception.message().unwrap_or_default(),
                        exception.stack().unwrap_or_default()
                    ),
                    None => String::from("uncaught exception"),
                })
            }
            other => JsError::engine(other),
        })
}

fn decode_value<'js>(ctx: &rquickjs::Ctx<'js>, value: Value<'js>) -> Result<ScriptValue, JsError> {
    decode_value_inner(ctx, value, 0)
}

fn decode_value_inner<'js>(
    ctx: &rquickjs::Ctx<'js>,
    value: Value<'js>,
    depth: u8,
) -> Result<ScriptValue, JsError> {
    if depth > 32 {
        return Ok(ScriptValue::Null);
    }
    if value.is_undefined() {
        return Ok(ScriptValue::Undefined);
    }
    if value.is_null() {
        return Ok(ScriptValue::Null);
    }
    if let Some(flag) = value.as_bool() {
        return Ok(ScriptValue::Bool(flag));
    }
    if let Some(number) = value.as_number() {
        return Ok(ScriptValue::Number(number));
    }
    if let Ok(node) = rquickjs::Class::<bindings::JsNode>::from_js(ctx, value.clone()) {
        return Ok(ScriptValue::Node(node.borrow().node_id()));
    }
    if let Some(id) = bindings::host_node_id(ctx, &value) {
        return Ok(ScriptValue::Node(id));
    }
    if let Some(array) = value.as_array() {
        let mut items = Vec::with_capacity(array.len());
        for index in 0..array.len() {
            let item: Value = array.get(index).map_err(JsError::engine)?;
            items.push(decode_value_inner(ctx, item, depth.saturating_add(1))?);
        }
        return Ok(ScriptValue::List(items));
    }
    if value.as_function().is_some() {
        return Ok(ScriptValue::Null);
    }
    if let Some(object) = value.as_object() {
        let mut map = Vec::new();
        for key in object.keys::<String>() {
            let key = key.map_err(JsError::engine)?;
            let nested: Value = object.get(key.as_str()).map_err(JsError::engine)?;
            map.push((
                key,
                decode_value_inner(ctx, nested, depth.saturating_add(1))?,
            ));
        }
        return Ok(ScriptValue::Map(map));
    }
    Coerced::<String>::from_js(ctx, value)
        .map(|coerced| ScriptValue::String(coerced.0))
        .map_err(JsError::engine)
}

fn render_eval_result<'js>(ctx: &rquickjs::Ctx<'js>, value: Value<'js>) -> Result<String, JsError> {
    if value.is_undefined() || value.is_null() {
        return Ok(String::new());
    }
    Coerced::<String>::from_js(ctx, value)
        .map(|coerced| coerced.0)
        .map_err(JsError::engine)
}

fn millis(delay: f64) -> u32 {
    if !delay.is_finite() || delay <= 0.0 {
        return 0;
    }
    let duration = Duration::from_secs_f64((delay / 1000.0).clamp(0.0, 86_400.0));
    u32::try_from(duration.as_millis()).unwrap_or(u32::MAX)
}
