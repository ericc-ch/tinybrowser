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
// WebIDL USVString conversion: unpaired surrogates become U+FFFD
// (<https://webidl.spec.whatwg.org/#idl-USVString>). Values that cross into
// Rust have to be valid UTF-8, and a URL parser sees replacement characters
// anyway.
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
// ── cross-realm structured serialization ───────────────────────────────
// Values cannot cross realms, so the sender's realm encodes a message into a
// versioned payload string and the receiver's realm decodes it
// (<https://html.spec.whatwg.org/multipage/structured-data.html#structuredserializeinternal>).
// Rust carries payloads between frames, owns the frame tree, and owns the
// channel endpoints, so a port survives being transferred to another realm.
const __tbPayloadVersion = 'tb1:';
const __tbPlatform = Symbol.for('tinybrowser.platform');
const __tbWindowProxyData = Symbol.for('tinybrowser.windowproxy.data');
const __tbPortData = Symbol.for('tinybrowser.messageport.data');
const __tbFrameId = globalThis.__tb_frameId;
const __tbFrameProxies = Object.create(null);
// Writes made through a proxy whose target realm does not exist yet: a
// browsing context registered inside a script gets its realm at the next
// non-JS turn, and the engine then flushes these through
// `__tbFlushFrameSets` (<https://html.spec.whatwg.org/multipage/window-object.html#windowproxy-set>).
const __tbFramePendingSets = Object.create(null);
const __tbFramePending = frame => {
  let pending = __tbFramePendingSets[frame];
  if (pending === undefined) {
    pending = Object.create(null);
    __tbFramePendingSets[frame] = pending;
  }
  return pending;
};
globalThis.__tbFlushFrameSets = function(frame) {
  const pending = __tbFramePendingSets[frame];
  if (pending === undefined) return;
  delete __tbFramePendingSets[frame];
  const target = __tbFrameGlobal(frame);
  if (target == null) return;
  for (const key of Reflect.ownKeys(pending)) {
    if (key === '__proto__' || typeof key === 'symbol') {
      // `CreateDataProperty`: a `__proto__` write must not walk the prototype
      // setter, and symbol keys need `defineProperty` to become own data
      // properties.
      Object.defineProperty(target, key, {
        value: pending[key], writable: true, enumerable: true, configurable: true,
      });
    } else {
      target[key] = pending[key];
    }
  }
};

const __tbCloneFailure = () => new globalThis.DOMException('The object could not be cloned.', 'DataCloneError');
const __tbBase64Bytes = text => {
  const binary = globalThis.atob(text);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index++) bytes[index] = binary.charCodeAt(index);
  return bytes;
};
// Cross-realm brand probes: an intrinsic from this realm accepts an object
// from any realm, and throws when the internal slot is missing.
const __tbProbe = (read, value) => {
  try { read(value); return true; } catch (error) { return false; }
};
const __tbIsDate = value => __tbProbe(candidate => Date.prototype.getTime.call(candidate), value);
const __tbIsRegExp = value => __tbProbe(candidate => Object.getOwnPropertyDescriptor(RegExp.prototype, 'source').get.call(candidate), value);
const __tbIsMap = value => __tbProbe(candidate => Object.getOwnPropertyDescriptor(Map.prototype, 'size').get.call(candidate), value);
const __tbIsSet = value => __tbProbe(candidate => Object.getOwnPropertyDescriptor(Set.prototype, 'size').get.call(candidate), value);
const __tbIsArrayBuffer = value => __tbProbe(candidate => Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, 'byteLength').get.call(candidate), value);
const __tbIsDataView = value => __tbProbe(candidate => Object.getOwnPropertyDescriptor(DataView.prototype, 'byteLength').get.call(candidate), value);
const __tbTypedArrayPrototype = Object.getPrototypeOf(Uint8Array.prototype);
const __tbIsTypedArray = value => __tbProbe(candidate => Object.getOwnPropertyDescriptor(__tbTypedArrayPrototype, 'length').get.call(candidate), value);
const __tbIsBoxedBoolean = value => __tbProbe(candidate => Boolean.prototype.valueOf.call(candidate), value);
const __tbIsBoxedNumber = value => __tbProbe(candidate => Number.prototype.valueOf.call(candidate), value);
const __tbIsBoxedString = value => __tbProbe(candidate => String.prototype.valueOf.call(candidate), value);
// A real error (from any realm) answers `[object Error]` through its internal
// slot; a plain object can only do so by defining `Symbol.toStringTag`, which
// real errors never do
// (<https://html.spec.whatwg.org/multipage/structured-data.html#structuredserializeinternal>).
const __tbIsError = value => Object.prototype.toString.call(value) === '[object Error]'
  && value[Symbol.toStringTag] === undefined;
// Types with internal slots the serializer cannot reproduce. Weak maps and
// sets answer their own brand probes; promises and weak refs expose a tag and
// have no non-destructive brand probe.
const __tbIsWeakMap = value => __tbProbe(candidate => WeakMap.prototype.has.call(candidate, candidate), value);
const __tbIsWeakSet = value => __tbProbe(candidate => WeakSet.prototype.has.call(candidate, candidate), value);
const __tbIsPromise = value => typeof Promise === 'function' && Object.prototype.toString.call(value) === '[object Promise]';
const __tbIsWeakRef = value => typeof WeakRef === 'function' && Object.prototype.toString.call(value) === '[object WeakRef]';
const __tbHasInternalSlots = value => __tbIsPromise(value) || __tbIsWeakMap(value)
  || __tbIsWeakSet(value) || __tbIsWeakRef(value);

// `[[NumberData]]` specials have no JSON spelling.
const __tbNumberWire = value => {
  if (value !== value) return 'NaN';
  if (value === Infinity) return 'Infinity';
  if (value === -Infinity) return '-Infinity';
  if (value === 0 && 1 / value === -Infinity) return '-0';
  return value;
};
const __tbNumberValue = wire => wire === 'NaN' ? NaN
  : wire === 'Infinity' ? Infinity
  : wire === '-Infinity' ? -Infinity
  : wire === '-0' ? -0
  : wire;

// https://webidl.spec.whatwg.org/#es-sequence
const __tbTransferList = transfer => {
  if (transfer === undefined) return [];
  if (transfer === null || typeof transfer !== 'object' || typeof transfer[Symbol.iterator] !== 'function') {
    throw new TypeError('The transfer list must be an iterable object');
  }
  return Array.from(transfer);
};

// Serializes `value` into { payload, ports }, detaching every transferred
// buffer and endpoint as it does so
// (<https://html.spec.whatwg.org/multipage/structured-data.html#structuredserializewithtransfer>).
const __tbEncode = (value, transfer, sourcePort) => {
  // Validate the transfer list first: nothing is transferred until the value
  // graph serializes, because transferring has side effects and
  // StructuredSerializeInternal must be able to throw first
  // (<https://html.spec.whatwg.org/multipage/structured-data.html#structuredserializewithtransfer>).
  const buffers = new Map();     // ArrayBuffer -> node slot, or -1 once listed
  const transferred = new Map(); // MessagePort -> endpoint id
  for (const item of __tbTransferList(transfer)) {
    if (item instanceof ArrayBuffer) {
      if (buffers.has(item)) throw new globalThis.DOMException('Transfer list contains duplicate buffers', 'DataCloneError');
      if (typeof item.transfer !== 'function') throw new globalThis.DOMException('The buffer is not transferable', 'DataCloneError');
      buffers.set(item, -1);
    } else if (item instanceof globalThis.MessagePort) {
      if (item === sourcePort) throw new globalThis.DOMException('Cannot transfer the source port', 'DataCloneError');
      const itemData = __tbBrand(item, __tbPortData);
      if (itemData.closed) throw new globalThis.DOMException('Cannot transfer a detached MessagePort', 'DataCloneError');
      if (transferred.has(item)) throw new globalThis.DOMException('Transfer list contains duplicate ports', 'DataCloneError');
      transferred.set(item, itemData.id);
    } else {
      throw new globalThis.DOMException('Value not transferable', 'DataCloneError');
    }
  }
  const nodes = [];
  const seen = new Map();
  const slot = descriptor => {
    const index = nodes.length;
    nodes.push(descriptor);
    return index;
  };
  const encode = input => {
    if (input === null) return slot(['null']);
    const kind = typeof input;
    if (kind === 'undefined') return slot(['undefined']);
    if (kind === 'boolean') return slot(['boolean', input]);
    if (kind === 'number') return slot(['number', __tbNumberWire(input)]);
    if (kind === 'string') return slot(['string', input]);
    if (kind === 'bigint') return slot(['bigint', String(input)]);
    if (kind === 'function' || kind === 'symbol') throw __tbCloneFailure();
    if (seen.has(input)) return seen.get(input);
    // Window proxies are never serializable
    // (<https://html.spec.whatwg.org/multipage/structured-data.html#structuredserializeinternal>).
    if (input[__tbWindowProxyData] !== undefined) throw __tbCloneFailure();
    if (transferred.has(input)) {
      const index = slot(['port', transferred.get(input)]);
      seen.set(input, index);
      return index;
    }
    if (buffers.has(input)) {
      // The bytes are captured by the transfer step, after the graph is done.
      const index = slot(null);
      seen.set(input, index);
      buffers.set(input, index);
      return index;
    }
    if (input[__tbBlobData] !== undefined) {
      const blob = input[__tbBlobData];
      const bytes = __tbBase64Encode(new Uint8Array(blob.bytes));
      let descriptor;
      if (input[__tbFileData] !== undefined) {
        const file = input[__tbFileData];
        descriptor = ['file', file.name, file.lastModified, blob.type, bytes];
      } else {
        descriptor = ['blob', blob.type, bytes];
      }
      const index = slot(descriptor);
      seen.set(input, index);
      return index;
    }
    if (__tbIsArrayBuffer(input)) {
      let view;
      try {
        view = new Uint8Array(input);
      } catch (error) {
        // A detached buffer cannot be copied; the spec reports it as a clone
        // failure, not a raw TypeError.
        throw __tbCloneFailure();
      }
      const index = slot(['buffer', __tbBase64Encode(view)]);
      seen.set(input, index);
      return index;
    }
    if (typeof SharedArrayBuffer === 'function' && input instanceof SharedArrayBuffer) {
      // Shared memory would need to stay shared across realms; the engine
      // has no shared-memory transport, so refuse instead of corrupting.
      throw __tbCloneFailure();
    }
    if (__tbIsTypedArray(input) || __tbIsDataView(input)) {
      const buffer = encode(input.buffer);
      const descriptor = __tbIsDataView(input)
        ? ['view', buffer, 'DataView', input.byteOffset, input.byteLength]
        : ['view', buffer, input.constructor.name, input.byteOffset, input.length];
      const index = slot(descriptor);
      seen.set(input, index);
      return index;
    }
    // DOMException is serializable (name and message survive)
    // (<https://html.spec.whatwg.org/multipage/structured-data.html#serializable-objects>).
    if (Object.prototype.toString.call(input) === '[object DOMException]') {
      return slot(['domexception', String(input.name), String(input.message)]);
    }
    // Platform objects (nodes, events, ports, ...) are not serializable.
    if (input[__tbPlatform] !== undefined || input[__tbPortData] !== undefined) throw __tbCloneFailure();
    // Objects with internal slots the serializer cannot reproduce must fail,
    // not silently clone as plain objects
    // (<https://html.spec.whatwg.org/multipage/structured-data.html#structuredserializeinternal>).
    if (__tbHasInternalSlots(input)) throw __tbCloneFailure();
    if (__tbIsDate(input)) return slot(['date', __tbNumberWire(input.getTime())]);
    if (__tbIsRegExp(input)) return slot(['regexp', input.source, input.flags]);
    if (__tbIsError(input)) {
      const index = slot(null);
      seen.set(input, index);
      nodes[index] = ['error', String(input.name), String(input.message)];
      return index;
    }
    if (__tbIsBoxedBoolean(input)) return slot(['boxed', 'Boolean', input.valueOf()]);
    if (__tbIsBoxedNumber(input)) return slot(['boxed', 'Number', __tbNumberWire(input.valueOf())]);
    if (__tbIsBoxedString(input)) return slot(['boxed', 'String', input.valueOf()]);
    if (__tbIsMap(input)) {
      const index = slot(null);
      seen.set(input, index);
      const entries = [];
      for (const [key, entry] of input) entries.push([encode(key), encode(entry)]);
      nodes[index] = ['map', entries];
      return index;
    }
    if (__tbIsSet(input)) {
      const index = slot(null);
      seen.set(input, index);
      const entries = [];
      for (const entry of input) entries.push(encode(entry));
      nodes[index] = ['set', entries];
      return index;
    }
    if (Array.isArray(input)) {
      const index = slot(null);
      seen.set(input, index);
      const items = [];
      for (let item = 0; item < input.length; item++) items.push(encode(input[item]));
      nodes[index] = ['array', items];
      return index;
    }
    // Anything else clones as an object with its own enumerable properties,
    // whatever its prototype chain says
    // (<https://html.spec.whatwg.org/multipage/structured-data.html#structuredserializeinternal>).
    const index = slot(null);
    seen.set(input, index);
    const entries = [];
    for (const key of Object.keys(input)) entries.push([key, encode(input[key])]);
    nodes[index] = ['object', entries];
    return index;
  };
  const root = encode(value);
  // Serialization succeeded, so the transfer steps run now
  // (<https://html.spec.whatwg.org/multipage/structured-data.html#structuredserializewithtransfer>).
  for (const [buffer, index] of buffers) {
    let moved;
    try {
      moved = buffer.transfer();
    } catch (error) {
      throw new globalThis.DOMException('The buffer is already detached', 'DataCloneError');
    }
    if (index !== -1) {
      nodes[index] = ['buffer', __tbBase64Encode(new Uint8Array(moved))];
    }
  }
  const ports = [];
  for (const [port, id] of transferred) {
    if (!__tbPortDetach(id)) throw new globalThis.DOMException('Cannot transfer a detached MessagePort', 'DataCloneError');
    __tbBrand(port, __tbPortData).closed = true;
    // The received port is a new object; the sender's is detached.
    delete __tbPorts[id];
    ports.push(id);
  }
  return { payload: __tbPayloadVersion + JSON.stringify({ root: root, nodes: nodes }), ports: ports };
};

// Deserializes a payload in this realm; `ports` maps transferred endpoint ids
// to the port objects that were already materialized for them.
const __tbDecode = (payload, ports) => {
  const text = String(payload);
  if (text.indexOf(__tbPayloadVersion) !== 0) throw new Error('structured clone payload mismatch');
  const wire = JSON.parse(text.slice(__tbPayloadVersion.length));
  const nodes = wire.nodes;
  const values = new Array(nodes.length);
  for (let index = 0; index < nodes.length; index++) {
    switch (nodes[index][0]) {
      case 'buffer': values[index] = __tbBase64Bytes(nodes[index][1]).buffer; break;
      case 'object': values[index] = {}; break;
      case 'array': values[index] = new Array(nodes[index][1].length); break;
      case 'map': values[index] = new Map(); break;
      case 'set': values[index] = new Set(); break;
      default: values[index] = undefined; break;
    }
  }
  for (let index = 0; index < nodes.length; index++) {
    const descriptor = nodes[index];
    switch (descriptor[0]) {
      case 'view': {
        const buffer = values[descriptor[1]];
        values[index] = descriptor[2] === 'DataView'
          ? new DataView(buffer, descriptor[3], descriptor[4])
          : new globalThis[descriptor[2]](buffer, descriptor[3], descriptor[4]);
        break;
      }
      case 'undefined': values[index] = undefined; break;
      case 'null': values[index] = null; break;
      case 'boolean': values[index] = descriptor[1]; break;
      case 'number': values[index] = __tbNumberValue(descriptor[1]); break;
      case 'string': values[index] = descriptor[1]; break;
      case 'bigint': values[index] = BigInt(descriptor[1]); break;
      case 'date': values[index] = new Date(__tbNumberValue(descriptor[1])); break;
      case 'regexp': values[index] = new RegExp(descriptor[1], descriptor[2]); break;
      case 'domexception': values[index] = new globalThis.DOMException(descriptor[2], descriptor[1]); break;
      case 'error': {
        const name = descriptor[1];
        const ctor = globalThis[name];
        values[index] = typeof ctor === 'function' && ctor.prototype instanceof Error
          ? new ctor(descriptor[2])
          : new Error(descriptor[2]);
        values[index].name = name;
        break;
      }
      case 'boxed': {
        const ctor = globalThis[descriptor[1]];
        values[index] = new ctor(__tbNumberValue(descriptor[2]));
        break;
      }
      case 'blob': values[index] = new Blob([__tbBase64Bytes(descriptor[2])], { type: descriptor[1] }); break;
      case 'file': values[index] = new File([__tbBase64Bytes(descriptor[4])], descriptor[1], { type: descriptor[2], lastModified: descriptor[3] }); break;
      case 'port': {
        const port = ports[descriptor[1]];
        if (port === undefined) throw new Error('missing transferred MessagePort');
        values[index] = port;
        break;
      }
    }
  }
  for (let index = 0; index < nodes.length; index++) {
    const descriptor = nodes[index];
    switch (descriptor[0]) {
      case 'object': {
        const target = values[index];
        for (const [key, slot] of descriptor[1]) {
          // `CreateDataProperty`, not `Set`: a `__proto__` key must become an
          // own property instead of walking the prototype setter
          // (<https://html.spec.whatwg.org/multipage/structured-data.html#structureddeserialize>).
          Object.defineProperty(target, key, {
            value: values[slot], writable: true, enumerable: true, configurable: true,
          });
        }
        break;
      }
      case 'array': {
        const target = values[index];
        for (let item = 0; item < descriptor[1].length; item++) target[item] = values[descriptor[1][item]];
        break;
      }
      case 'map': {
        const target = values[index];
        for (const [key, entry] of descriptor[1]) target.set(values[key], values[entry]);
        break;
      }
      case 'set': {
        const target = values[index];
        for (const slot of descriptor[1]) target.add(values[slot]);
        break;
      }
    }
  }
  return values[wire.root];
};
// https://html.spec.whatwg.org/multipage/structured-data.html#dom-structuredclone
globalThis.structuredClone = function(value, options) {
  if (options != null && typeof options !== 'object') {
    throw new TypeError('The provided value is not of type StructuredSerializeOptions');
  }
  const transfer = options == null ? undefined : options.transfer;
  const encoded = __tbEncode(value, transfer, null);
  const materialized = __tbMaterializePorts(encoded.ports);
  const clone = __tbDecode(encoded.payload, materialized.ports);
  __tbFirePortCloses(materialized.closes);
  return clone;
};

// ── message ports ──────────────────────────────────────────────────────
// The endpoint (id, queue, entanglement) lives in Rust; the JS object is the
// realm's handle on it, registered in `__tbPorts` so deliveries can find it.
const __tbPorts = Object.create(null);

// Materializes every transferred endpoint. Closes are deferred: a port whose
// peer disentangled while it was in transit fires `close` once the carrying
// message has been dispatched, so the message handler can install
// `onclose` first
// (<https://html.spec.whatwg.org/multipage/web-messaging.html#disentangle>).
const __tbMaterializePorts = ids => {
  const ports = {};
  const closes = [];
  for (const id of ids) {
    const pendingClose = __tbPortAdopt(id);
    if (pendingClose == null) throw new Error('missing transferred MessagePort');
    ports[id] = __tbMaterializePort(id);
    if (pendingClose) closes.push(id);
  }
  return { ports: ports, closes: closes };
};
const __tbFirePortCloses = ids => {
  for (const id of ids) {
    const port = __tbPortLookup(id);
    if (port === null) continue;
    const data = __tbBrand(port, __tbPortData);
    data.closed = true;
    __tbPortClose(id);
    port.__tbDispatchTrusted(new globalThis.Event('close'));
  }
};
// Drops ports a delivery could not decode; the spec loses them with the
// failed message.
const __tbDiscardPorts = ids => {
  for (const id of ids) {
    const port = __tbPortLookup(id);
    if (port === null) continue;
    __tbBrand(port, __tbPortData).closed = true;
    __tbPortClose(id);
  }
};
const __tbMaterializePort = id => {
  if (__tbPorts[id] !== undefined) return __tbPorts[id];
  const port = Reflect.construct(globalThis.EventTarget, [], globalThis.MessagePort);
  Object.defineProperty(port, __tbPortData, {
    value: { id: id, closed: false, onmessage: null, onclose: null },
    writable: false, enumerable: false, configurable: false,
  });
  __tbPorts[id] = port;
  return port;
};
const __tbPortLookup = id => {
  const port = __tbPorts[id];
  return port === undefined ? null : port;
};
// https://html.spec.whatwg.org/multipage/web-messaging.html#messageport
globalThis.MessagePort = class MessagePort extends EventTarget {
  constructor() { throw new TypeError('Illegal constructor'); }
  postMessage(message, transfer) {
    const data = __tbBrand(this, __tbPortData);
    // Overload resolution: an iterable second argument is the transfer
    // sequence; any other object is the options dictionary, whose unknown
    // members are ignored
    // (<https://html.spec.whatwg.org/multipage/web-messaging.html#dom-messageport-postmessage>).
    if (transfer !== null && typeof transfer === 'object'
        && typeof transfer[Symbol.iterator] !== 'function') {
      transfer = transfer.transfer;
    }
    // The transfer list is consumed even when the port is detached or the
    // message is doomed
    // (<https://html.spec.whatwg.org/multipage/web-messaging.html#message-port-post-message-steps>).
    const encoded = __tbEncode(message, transfer, this);
    if (data.closed) return;
    // A port posted to its own entangled port loses the channel
    // (<https://html.spec.whatwg.org/multipage/web-messaging.html#message-port-post-message-steps>).
    const peer = __tbPortPeer(data.id);
    if (peer != null && encoded.ports.indexOf(peer) !== -1) return;
    __tbPortPost(data.id, encoded.payload, encoded.ports);
  }
  start() {
    const data = __tbBrand(this, __tbPortData);
    if (data.closed) return;
    __tbPortStart(data.id);
  }
  close() {
    const data = __tbBrand(this, __tbPortData);
    if (data.closed) return;
    data.closed = true;
    __tbPortClose(data.id);
  }
  get onmessage() { return __tbBrand(this, __tbPortData).onmessage; }
  set onmessage(value) {
    __tbBrand(this, __tbPortData).onmessage = value;
    // The first set enables the queue, whatever the value
    // (<https://html.spec.whatwg.org/multipage/web-messaging.html#message-ports>).
    this.start();
  }
  get onmessageerror() { return __tbBrand(this, __tbPortData).onmessageerror; }
  set onmessageerror(value) { __tbBrand(this, __tbPortData).onmessageerror = value; }
  get onclose() { return __tbBrand(this, __tbPortData).onclose; }
  set onclose(value) { __tbBrand(this, __tbPortData).onclose = value; }
};
Object.defineProperty(globalThis.MessagePort.prototype, Symbol.toStringTag, { value: 'MessagePort', writable: false, enumerable: false, configurable: true });
// https://html.spec.whatwg.org/multipage/web-messaging.html#messagechannel
globalThis.MessageChannel = class MessageChannel {
  constructor() {
    const pair = __tbPortNew();
    const port1 = __tbMaterializePort(pair[0]);
    const port2 = __tbMaterializePort(pair[1]);
    Object.defineProperty(this, 'port1', { value: port1, writable: false, enumerable: true, configurable: true });
    Object.defineProperty(this, 'port2', { value: port2, writable: false, enumerable: true, configurable: true });
  }
};
Object.defineProperty(globalThis.MessageChannel.prototype, Symbol.toStringTag, { value: 'MessageChannel', writable: false, enumerable: false, configurable: true });

// ── window proxies ─────────────────────────────────────────────────────
// One proxy per frame per realm, stable across the frame's navigations. It
// carries the cross-origin whitelist; same-origin members forward through the
// target realm's window
// (<https://html.spec.whatwg.org/multipage/window-object.html#the-windowproxy-exotic-object>).
// The cross-origin member set follows Blink's `[CrossOrigin]` IDL attributes
// and Firefox's `sCrossOriginProperties`: postMessage, window, self, frames,
// length, top, parent, closed, and the indexed getter.
const __tbFrameProxy = frame => {
  if (frame == null) return undefined;
  if (frame === __tbFrameId) return globalThis;
  if (__tbFrameProxies[frame] !== undefined) return __tbFrameProxies[frame];
  let proxy;
  const crossOrigin = () => {
    throw new globalThis.DOMException('Blocked a frame from accessing a cross-origin frame.', 'SecurityError');
  };
  // One function per proxy: a shipped engine keeps
  // `contentWindow.postMessage` identity stable, and so do we.
  const postMessage = function(message, targetOrigin, transfer) {
    try {
      return __tbPostMessage(frame, arguments.length, message, targetOrigin, transfer);
    } catch (error) {
      // Exceptions from a proxy's postMessage come from the target window's
      // realm, the way a shipped engine throws them.
      if (error instanceof globalThis.DOMException) {
        const global = __tbFrameGlobal(frame);
        const constructor = global == null ? undefined : global.DOMException;
        if (typeof constructor === 'function' && error.constructor !== constructor) {
          throw new constructor(error.message, error.name);
        }
      }
      throw error;
    }
  };
  const handler = {
    get(target, property) {
      if (property === __tbWindowProxyData) return { frame: frame };
      switch (property) {
        case 'postMessage':
          return postMessage;
        case 'parent': {
          const parent = __tbFrameParent(frame);
          return parent == null ? proxy : __tbFrameProxy(parent);
        }
        case 'top': {
          const top = __tbFrameTop(frame);
          return top == null || top === frame ? proxy : __tbFrameProxy(top);
        }
        case 'window': case 'self': case 'frames': return proxy;
        case 'length': return __tbFrameChildCount(frame);
        case 'closed': return !__tbFrameRegistered(frame);
        case Symbol.toStringTag: return 'Window';
        case 'document': return __tbFrameDocument(frame);
      }
      const sameOrigin = __tbFrameGlobal(frame);
      if (sameOrigin == null) {
        if (__tbFrameRegistered(frame)) {
          const pending = __tbFramePendingSets[frame];
          if (pending !== undefined && Object.prototype.hasOwnProperty.call(pending, property)) {
            return pending[property];
          }
          return undefined;
        }
        crossOrigin();
      }
      return sameOrigin[property];
    },
    set(target, property, value) {
      const sameOrigin = __tbFrameGlobal(frame);
      if (sameOrigin == null) {
        if (__tbFrameRegistered(frame)) {
          __tbFramePending(frame)[property] = value;
          return true;
        }
        crossOrigin();
      }
      sameOrigin[property] = value;
      return true;
    },
    has(target, property) {
      switch (property) {
        case 'postMessage': case 'parent': case 'top': case 'window': case 'self':
        case 'frames': case 'length': case 'closed': case 'document':
          return true;
      }
      const sameOrigin = __tbFrameGlobal(frame);
      if (sameOrigin != null) return property in sameOrigin;
      const pending = __tbFramePendingSets[frame];
      return pending !== undefined && property in pending;
    },
    getPrototypeOf() {
      // The spec forwards to the target; cross-origin callers cannot reach
      // it, so the plain object prototype stands in
      // (<https://html.spec.whatwg.org/multipage/window-object.html#windowproxy-getprototypeof>).
      const sameOrigin = __tbFrameGlobal(frame);
      return sameOrigin == null ? globalThis.Object.prototype : globalThis.Object.getPrototypeOf(sameOrigin);
    },
    ownKeys() {
      const sameOrigin = __tbFrameGlobal(frame);
      return sameOrigin == null ? [] : globalThis.Reflect.ownKeys(sameOrigin);
    },
    getOwnPropertyDescriptor() { return undefined; },
  };
  proxy = new Proxy({}, handler);
  __tbFrameProxies[frame] = proxy;
  return proxy;
};
globalThis.__tbFrameProxy = __tbFrameProxy;

// ── posting and delivery ───────────────────────────────────────────────
// https://html.spec.whatwg.org/multipage/web-messaging.html#dom-window-postmessage
function __tbPostMessage(targetFrame, argumentCount, message, targetOrigin, transfer) {
  if (argumentCount === 0) {
    throw new TypeError(`Failed to execute 'postMessage' on 'Window': 1 argument required, but only 0 present.`);
  }
  if (targetFrame == null) {
    throw new TypeError(`Failed to execute 'postMessage' on 'Window': the target window is missing.`);
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
      parsed = new globalThis.URL(checkedOrigin);
    } catch (error) {
      throw new globalThis.DOMException('Invalid target origin', 'SyntaxError');
    }
    checkedOrigin = parsed.origin;
  }
  const encoded = __tbEncode(message, transfer, null);
  __tbPostWindowMessage(targetFrame, checkedOrigin, encoded.payload, encoded.ports);
}
globalThis.postMessage = function(message, targetOrigin, transfer) {
  return __tbPostMessage(__tbFrameId, arguments.length, message, targetOrigin, transfer);
};
// Runs in the target realm: decode, then dispatch a trusted message event, or
// report the failure so the engine dispatches `messageerror`
// (<https://html.spec.whatwg.org/multipage/web-messaging.html#window-post-message-steps>).
globalThis.__tbDeliverMessage = function(payload, sourceFrame, origin, portIds) {
  if (String(payload).indexOf(__tbPayloadVersion) !== 0) return false;
  const materialized = __tbMaterializePorts(portIds);
  let data;
  try {
    data = __tbDecode(payload, materialized.ports);
  } catch (error) {
    // The ports arrived with a message that cannot be decoded; they are lost
    // with it.
    __tbDiscardPorts(portIds);
    return false;
  }
  const portArray = portIds.map(id => materialized.ports[id]);
  globalThis.__tbDispatchTrusted(new globalThis.MessageEvent('message', {
    data: data, origin: origin, source: __tbFrameProxy(sourceFrame), ports: Object.freeze(portArray),
  }));
  __tbFirePortCloses(materialized.closes);
  return true;
};
globalThis.__tbDeliverMessageError = function(sourceFrame, origin) {
  globalThis.__tbDispatchTrusted(new globalThis.MessageEvent('messageerror', {
    data: null, origin: origin, source: __tbFrameProxy(sourceFrame),
  }));
};
// A message from another tab cannot name a frame in this renderer, so its
// source is `null`
// (<https://html.spec.whatwg.org/multipage/web-messaging.html#window-post-message-steps>).
globalThis.__tbDeliverRemoteMessage = function(payload) {
  if (String(payload).indexOf(__tbPayloadVersion) !== 0) return;
  let data;
  try {
    data = __tbDecode(payload, []);
  } catch (error) {
    return;
  }
  globalThis.__tbDispatchTrusted(new globalThis.MessageEvent('message', {
    data: data, origin: '', source: null, ports: Object.freeze([]),
  }));
};

// ── broadcast channels ─────────────────────────────────────────────────
// https://html.spec.whatwg.org/multipage/web-messaging.html#broadcastchannel
const __tbBroadcastData = Symbol.for('tinybrowser.broadcastchannel.data');
const __tbBroadcastChannels = new Set();
let __tbNextBroadcastChannel = 1;

globalThis.BroadcastChannel = class BroadcastChannel extends EventTarget {
  constructor(name) {
    if (arguments.length < 1) {
      throw new TypeError("Failed to construct 'BroadcastChannel': 1 argument required, but only 0 present.");
    }
    super();
    Object.defineProperty(this, __tbBroadcastData, {
      value: {
        id: __tbNextBroadcastChannel++, name: String(name), closed: false,
        onmessage: null, onmessageerror: null,
      },
      writable: false, enumerable: false, configurable: false,
    });
    __tbBroadcastChannels.add(this);
  }
  get name() { return __tbBrand(this, __tbBroadcastData).name; }
  postMessage(message) {
    if (arguments.length < 1) {
      throw new TypeError("Failed to execute 'postMessage' on 'BroadcastChannel': 1 argument required, but only 0 present.");
    }
    const data = __tbBrand(this, __tbBroadcastData);
    if (data.closed) {
      throw new globalThis.DOMException('The channel is closed.', 'InvalidStateError');
    }
    const encoded = __tbEncode(message, [], null);
    const origin = __tbStorageOrigin();
    if (origin === null || origin === undefined) return;
    __tbBroadcastPost(origin, data.name, encoded.payload, data.id);
  }
  close() {
    const data = __tbBrand(this, __tbBroadcastData);
    if (data.closed) return;
    data.closed = true;
    __tbBroadcastChannels.delete(this);
  }
  get onmessage() { return __tbBrand(this, __tbBroadcastData).onmessage; }
  set onmessage(value) { __tbBrand(this, __tbBroadcastData).onmessage = value; }
  get onmessageerror() { return __tbBrand(this, __tbBroadcastData).onmessageerror; }
  set onmessageerror(value) { __tbBrand(this, __tbBroadcastData).onmessageerror = value; }
};
Object.defineProperty(globalThis.BroadcastChannel.prototype, Symbol.toStringTag, {
  value: 'BroadcastChannel', writable: false, enumerable: false, configurable: true,
});

globalThis.__tbDeliverBroadcast = function(name, payload, origin, sourceChannel) {
  for (const channel of __tbBroadcastChannels) {
    const data = channel[__tbBroadcastData];
    if (data === undefined || data.closed || data.name !== name) continue;
    if (sourceChannel !== null && sourceChannel !== undefined && data.id === sourceChannel) continue;
    let event;
    try {
      event = new globalThis.MessageEvent('message', {
        data: __tbDecode(payload, []), origin: origin, source: null, ports: Object.freeze([]),
      });
    } catch (error) {
      event = new globalThis.MessageEvent('messageerror', {
        data: null, origin: origin, source: null, ports: Object.freeze([]),
      });
    }
    channel.__tbDispatchTrusted(event);
  }
};
globalThis.__tbDeliverPortMessage = function(endpoint, payload, portIds) {
  const port = __tbPortLookup(endpoint);
  if (port === null) return true;
  const data = __tbBrand(port, __tbPortData);
  if (data.closed) return true;
  if (String(payload).indexOf(__tbPayloadVersion) !== 0) return false;
  const materialized = __tbMaterializePorts(portIds);
  let value;
  try {
    value = __tbDecode(payload, materialized.ports);
  } catch (error) {
    __tbDiscardPorts(portIds);
    return false;
  }
  const portArray = portIds.map(id => materialized.ports[id]);
  port.__tbDispatchTrusted(new globalThis.MessageEvent('message', {
    data: value, ports: Object.freeze(portArray),
  }));
  __tbFirePortCloses(materialized.closes);
  return true;
};
globalThis.__tbDeliverPortMessageError = function(endpoint) {
  const port = __tbPortLookup(endpoint);
  if (port === null) return;
  port.__tbDispatchTrusted(new globalThis.MessageEvent('messageerror'));
};
globalThis.__tbDeliverPortClose = function(endpoint) {
  const port = __tbPortLookup(endpoint);
  if (port === null) return;
  const data = __tbBrand(port, __tbPortData);
  if (data.closed) return;
  port.__tbDispatchTrusted(new globalThis.Event('close'));
};

// ── the window's own indexed and browsing-context members ──────────────
// https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-length
Object.defineProperty(globalThis, 'length', {
  get() { return __tbFrameChildCount(__tbFrameId); },
  configurable: true, enumerable: false,
});
// https://html.spec.whatwg.org/multipage/window-object.html#dom-origin
Object.defineProperty(globalThis, 'origin', {
  get() {
    const value = __tbStorageOrigin();
    return value === null || value === undefined ? 'null' : value;
  },
  configurable: true, enumerable: true,
});
// https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-window-item
for (let index = 0; index < __tb_maxFrames; index++) {
  Object.defineProperty(globalThis, String(index), {
    get() {
      const child = __tbFrameChild(__tbFrameId, index);
      return child == null ? undefined : __tbFrameProxy(child);
    },
    configurable: true, enumerable: false,
  });
}
// https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-parent
Object.defineProperty(globalThis, 'parent', {
  get() {
    const parent = __tbFrameParent(__tbFrameId);
    return parent == null ? globalThis : __tbFrameProxy(parent);
  },
  configurable: true, enumerable: true,
});
// https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-top
Object.defineProperty(globalThis, 'top', {
  get() {
    const top = __tbFrameTop(__tbFrameId);
    return top === __tbFrameId ? globalThis : __tbFrameProxy(top);
  },
  configurable: true, enumerable: true,
});

// Event handler properties live in the world rather than on the wrapper, so
// a collected wrapper cannot lose `element.onload`
// (<https://html.spec.whatwg.org/multipage/webappapis.html#event-handlers>).
(function() {
  const names = globalThis.__tb_handlerNames;
  if (names === undefined) return;
  for (const name of names) {
    Object.defineProperty(globalThis, name, {
      get() { return globalThis.__tbGetWindowHandler(name); },
      set(value) { globalThis.__tbSetWindowHandler(name, value); },
      enumerable: true, configurable: true,
    });
    for (const proto of [globalThis.HTMLElement.prototype, globalThis.SVGElement.prototype]) {
      Object.defineProperty(proto, name, {
        get() { return globalThis.__tbGetNodeHandler(this, name); },
        set(value) { globalThis.__tbSetNodeHandler(this, name, value); },
        enumerable: true, configurable: true,
      });
    }
  }
})();

// ── auxiliary windows ──────────────────────────────────────────────────
// `window.open` creates a browser tab; the returned remote-window object
// exposes `close`, `postMessage`, and the same-origin storage areas. A named
// window is one browsing context, so reopening a live name returns the same
// object without a session copy
// (<https://html.spec.whatwg.org/multipage/window-object.html#dom-open>).
(function() {
  const remoteWindows = new Map();
  const namedWindows = new Map();
  const remoteSessions = new Map();

  function remoteWindow(tab) {
    const existing = remoteWindows.get(tab);
    if (existing !== undefined) return existing;
    const handler = {
      get(target, property) {
        switch (property) {
          case 'close': return () => { __tbWindowClose(tab); };
          case 'closed': return false;
          case 'postMessage': return (message, targetOrigin, transfer) => {
            const encoded = __tbEncode(message, transfer ?? [], null);
            __tbWindowPostMessage(tab, encoded.payload);
          };
          case 'localStorage': return globalThis.__tbStorageArea('local');
          case 'sessionStorage': return remoteSession(tab);
          case Symbol.toStringTag: return 'Window';
          default: return undefined;
        }
      },
      has(target, property) {
        switch (property) {
          case 'close': case 'closed': case 'postMessage':
          case 'localStorage': case 'sessionStorage':
            return true;
        }
        return false;
      },
      getOwnPropertyDescriptor() { return undefined; },
      ownKeys() { return []; },
    };
    const proxy = new Proxy({}, handler);
    remoteWindows.set(tab, proxy);
    return proxy;
  }

  // A live read of another window's session area, for same-origin openers and
  // opened windows. The response carries the storage seam's encoded strings.
  function remoteSession(tab) {
    const existing = remoteSessions.get(tab);
    if (existing !== undefined) return existing;
    const session = {
      getItem(key) {
        if (arguments.length < 1) {
          throw new TypeError("Failed to execute 'getItem' on 'Storage': 1 argument required, but only 0 present.");
        }
        const value = __tbRemoteSessionGet(tab, globalThis.__tbStorageEncode(String(key)));
        return value === undefined ? null : globalThis.__tbStorageDecode(value);
      },
    };
    remoteSessions.set(tab, session);
    return session;
  }

  function requestsNoOpener(features) {
    return features
      .split(/[\s,]+/)
      .some(token => token.toLowerCase() === 'noopener' || token.toLowerCase() === 'noreferrer');
  }

  // A new auxiliary browsing context gets a copy of this window's session
  // area (<https://html.spec.whatwg.org/multipage/document-sequences.html#copy-session-storage>).
  // The storage seam stores JSON-escaped strings, so the seed carries the
  // encoded keys and values unchanged.
  function sessionSeed() {
    const origin = __tbStorageOrigin();
    if (origin === null || origin === undefined) return null;
    const entries = [];
    for (const key of __tbStorageKeys('session')) {
      entries.push(key, __tbStorageGet('session', key));
    }
    return { origin: origin, entries: entries };
  }

  globalThis.open = function(url, target, features) {
    if (arguments.length < 1 || url === undefined || url === null) url = '';
    const spec = url === '' ? '' : __tbResolveUrl(String(url), undefined);
    if (spec === null || spec === undefined) return null;
    const name = target === undefined || target === null ? '' : String(target);
    const featureString = features === undefined || features === null ? '' : String(features);
    if (name !== '' && namedWindows.has(name)) return namedWindows.get(name);
    const seed = requestsNoOpener(featureString) ? null : sessionSeed();
    const tab = __tbWindowOpen(
      spec, name, featureString,
      seed === null ? '' : seed.origin,
      seed === null ? [] : seed.entries);
    if (tab === null || tab === undefined) return null;
    const proxy = remoteWindow(tab);
    if (name !== '') namedWindows.set(name, proxy);
    return proxy;
  };

  // https://html.spec.whatwg.org/multipage/window-object.html#dom-opener
  Object.defineProperty(globalThis, 'opener', {
    get() {
      const tab = __tbWindowOpener();
      return tab === null || tab === undefined ? null : remoteWindow(tab);
    },
    configurable: true,
  });
})();

// ── web storage ─────────────────────────────────────────────────────────
// `localStorage` and `sessionStorage` are one interface over two areas: the
// browser process owns the local area (shared by every tab, persisted with
// the profile), the engine owns the session area (one per top-level browsing
// context). Items are named properties per the legacy platform-object rules:
// a stored item is hidden by a member of the prototype chain, and every
// string-keyed write stores
// (<https://html.spec.whatwg.org/multipage/webstorage.html#the-storage-interface>,
// <https://webidl.spec.whatwg.org/#legacy-platform-object>).
(function() {
  const kindSlot = Symbol('storageKind');
  const holders = new Map();

  function kindOf(storage) {
    const kind = storage == null ? undefined : storage[kindSlot];
    if (kind !== 'local' && kind !== 'session') {
      throw new TypeError('Illegal invocation');
    }
    return kind;
  }

  const quotaError = key => new globalThis.QuotaExceededError(
    "Failed to execute 'setItem' on 'Storage': Setting the value of '" + key +
    "' exceeded the quota.");

  // JavaScript strings may hold lone surrogates; DOMString preserves them but
  // the UTF-8 host seam cannot. JSON escaping round-trips them exactly
  // (<https://webidl.spec.whatwg.org/#idl-DOMString>).
  const encode = value => JSON.stringify(value);
  const decode = value => (value === null || value === undefined ? null : JSON.parse(value));

  // https://storage.spec.whatwg.org/#quotaexceedederror
  globalThis.QuotaExceededError = class QuotaExceededError extends globalThis.DOMException {
    constructor(message = '') {
      super(message, 'QuotaExceededError');
    }
  };
  Object.defineProperty(globalThis.QuotaExceededError.prototype, Symbol.toStringTag, {
    value: 'QuotaExceededError', writable: false, enumerable: false, configurable: true,
  });

  class Storage {
    constructor(kind) {
      Object.defineProperty(this, kindSlot, {
        value: kind, writable: false, enumerable: false, configurable: false,
      });
    }
    get length() {
      return __tbStorageKeys(kindOf(this)).length;
    }
    key(index) {
      if (arguments.length < 1) {
        throw new TypeError("Failed to execute 'key' on 'Storage': 1 argument required, but only 0 present.");
      }
      const keys = __tbStorageKeys(kindOf(this));
      const n = Number(index) >>> 0;
      return n < keys.length ? decode(keys[n]) : null;
    }
    getItem(key) {
      if (arguments.length < 1) {
        throw new TypeError("Failed to execute 'getItem' on 'Storage': 1 argument required, but only 0 present.");
      }
      const item = __tbStorageGet(kindOf(this), encode(String(key)));
      return item === undefined ? null : decode(item);
    }
    setItem(key, value) {
      if (arguments.length < 2) {
        throw new TypeError("Failed to execute 'setItem' on 'Storage': 2 arguments required.");
      }
      key = String(key);
      if (!__tbStorageSet(kindOf(this), encode(key), encode(String(value)))) {
        throw quotaError(key);
      }
    }
    removeItem(key) {
      if (arguments.length < 1) {
        throw new TypeError("Failed to execute 'removeItem' on 'Storage': 1 argument required, but only 0 present.");
      }
      __tbStorageRemove(kindOf(this), encode(String(key)));
    }
    clear() {
      __tbStorageClear(kindOf(this));
    }
  }
  globalThis.Storage = Storage;
  Object.defineProperty(Storage.prototype, Symbol.toStringTag, {
    value: 'Storage', writable: false, enumerable: false, configurable: true,
  });

  function area(kind) {
    const existing = holders.get(kind);
    if (existing !== undefined) return existing;
    const origin = __tbStorageOrigin();
    if (origin === null || origin === undefined) {
      throw new globalThis.DOMException(
        "Failed to read the '" + (kind === 'session' ? 'sessionStorage' : 'localStorage') +
        "' property from 'Window': Storage is unavailable for opaque origins.", 'SecurityError');
    }
    const target = new Storage(kind);
    const handler = {
      get(t, property, receiver) {
        if (typeof property === 'symbol' || property in t) {
          return Reflect.get(t, property, receiver);
        }
        const item = __tbStorageGet(kind, encode(property));
        return item === null || item === undefined ? undefined : decode(item);
      },
      set(t, property, value) {
        if (typeof property === 'symbol') return Reflect.set(t, property, value);
        if (!__tbStorageSet(kind, encode(property), encode(String(value)))) {
          throw quotaError(property);
        }
        return true;
      },
      has(t, property) {
        if (typeof property === 'symbol' || property in t) return true;
        const item = __tbStorageGet(kind, encode(property));
        return item !== null && item !== undefined;
      },
      deleteProperty(t, property) {
        if (typeof property === 'symbol') return Reflect.deleteProperty(t, property);
        __tbStorageRemove(kind, encode(property));
        return true;
      },
      defineProperty(t, property, descriptor) {
        if (typeof property === 'symbol') return Reflect.defineProperty(t, property, descriptor);
        const value = 'value' in descriptor ? String(descriptor.value) : 'undefined';
        if (!__tbStorageSet(kind, encode(property), encode(value))) throw quotaError(property);
        return true;
      },
      getOwnPropertyDescriptor(t, property) {
        if (typeof property === 'symbol') return Reflect.getOwnPropertyDescriptor(t, property);
        if (property in t) return undefined;
        const item = __tbStorageGet(kind, encode(property));
        if (item === null || item === undefined) return undefined;
        return { value: decode(item), writable: true, enumerable: true, configurable: true };
      },
      ownKeys(t) {
        const names = __tbStorageKeys(kind).map(decode).filter(key => !(key in t));
        return Reflect.ownKeys(t).concat(names);
      },
    };
    const proxy = new Proxy(target, handler);
    holders.set(kind, proxy);
    return proxy;
  }

  // https://html.spec.whatwg.org/multipage/webstorage.html#the-storageevent-interface
  const eventData = Symbol.for('tinybrowser.storageevent.data');
  function dataOf(event) {
    const data = event == null ? undefined : event[eventData];
    if (data === undefined) throw new TypeError('Illegal invocation');
    return data;
  }
  function nullableString(value) {
    return value === undefined || value === null ? null : String(value);
  }
  globalThis.StorageEvent = class StorageEvent extends Event {
    constructor(type, init = undefined) {
      if (arguments.length < 1) {
        throw new TypeError("Failed to construct 'StorageEvent': 1 argument required, but only 0 present.");
      }
      const eventInit = init === undefined ? {} : Object(init);
      super(String(type), eventInit);
      Object.defineProperty(this, eventData, {
        value: {
          key: nullableString(eventInit.key),
          oldValue: nullableString(eventInit.oldValue),
          newValue: nullableString(eventInit.newValue),
          url: eventInit.url === undefined ? '' : String(eventInit.url),
          storageArea: eventInit.storageArea === undefined ? null : eventInit.storageArea,
        },
        writable: false, enumerable: false, configurable: false,
      });
    }
    get key() { return dataOf(this).key; }
    get oldValue() { return dataOf(this).oldValue; }
    get newValue() { return dataOf(this).newValue; }
    get url() { return dataOf(this).url; }
    get storageArea() { return dataOf(this).storageArea; }
    initStorageEvent(type, bubbles = false, cancelable = false, key = null, oldValue = null, newValue = null, url = '', storageArea = null) {
      if (arguments.length < 1) {
        throw new TypeError("Failed to execute 'initStorageEvent' on 'StorageEvent': 1 argument required, but only 0 present.");
      }
      Event.prototype.initEvent.call(this, String(type), Boolean(bubbles), Boolean(cancelable));
      const data = dataOf(this);
      data.key = key === null ? null : String(key);
      data.oldValue = oldValue === null ? null : String(oldValue);
      data.newValue = newValue === null ? null : String(newValue);
      data.url = String(url);
      data.storageArea = storageArea;
    }
  };
  Object.defineProperty(globalThis.StorageEvent.prototype, Symbol.toStringTag, {
    value: 'StorageEvent', writable: false, enumerable: false, configurable: true,
  });

  /// Fires one storage event in this realm, with this realm's area as
  /// `storageArea`
  /// (<https://html.spec.whatwg.org/multipage/webstorage.html#concept-storage-broadcast>).
  globalThis.__tbFireStorageEvent = function(kind, key, oldValue, newValue, url) {
    globalThis.__tbDispatchTrusted(new globalThis.StorageEvent('storage', {
      key: decode(key), oldValue: decode(oldValue), newValue: decode(newValue),
      url: url, storageArea: area(kind),
    }));
  };

  Object.defineProperty(globalThis, 'localStorage', {
    get() { return area('local'); }, configurable: true,
  });
  Object.defineProperty(globalThis, 'sessionStorage', {
    get() { return area('session'); }, configurable: true,
  });
  // Remote-window proxies share these areas for same-origin openers.
  globalThis.__tbStorageArea = area;
  globalThis.__tbStorageEncode = encode;
  globalThis.__tbStorageDecode = decode;
})();

// Platform objects and globals are not serializable; the marker travels with
// the prototype, so it identifies an object from another realm too
// (<https://html.spec.whatwg.org/multipage/structured-data.html#serializable-objects>).
(function() {
  const names = [
    'Event', 'EventTarget', 'Node', 'DOMException', 'Attr', 'NamedNodeMap',
    'TokenList', 'Implementation', 'DOMParser', 'XMLSerializer', 'MutationObserver',
    'MutationRecord', 'MessageEvent', 'MessagePort', 'MessageChannel', 'Headers',
    'Request', 'Response', 'Blob', 'File', 'FileList', 'FileReader', 'ProgressEvent',
    'ReadableStream', 'TextDecoder', 'TextEncoder', 'URL', 'URLSearchParams',
    'AbortController', 'AbortSignal', 'CustomEvent', 'Document',
    'Storage', 'StorageEvent', 'QuotaExceededError',
  ];
  for (const name of names) {
    const ctor = globalThis[name];
    if (typeof ctor === 'function' && ctor.prototype !== undefined) {
      try {
        Object.defineProperty(ctor.prototype, __tbPlatform, {
          value: true, writable: false, enumerable: false, configurable: true,
        });
      } catch (error) {}
    }
  }
  Object.defineProperty(globalThis, __tbPlatform, {
    value: true, writable: false, enumerable: false, configurable: true,
  });
})();

// ── uievents and the WebDriver actions performer ───────────────────────
// Event interfaces for input dispatch, plus the page-side half of
// `POST /session/{id}/actions`
// (<https://w3c.github.io/webdriver/#perform-actions>). The engine fires its
// own input events from Rust; these classes exist so pages and the performer
// can construct events with the spec's properties.

const __tbUIEventData = Symbol.for('tinybrowser.uievent.data');
globalThis.UIEvent = class UIEvent extends Event {
  constructor(type, init) {
    if (arguments.length < 1) {
      throw new TypeError("Failed to construct 'UIEvent': 1 argument required, but only 0 present.");
    }
    init = init || {};
    super(type, init);
    Object.defineProperty(this, __tbUIEventData, {
      value: { view: init.view || null, detail: init.detail || 0 },
      writable: false, enumerable: false, configurable: false,
    });
  }
  get view() { return __tbBrand(this, __tbUIEventData).view; }
  get detail() { return __tbBrand(this, __tbUIEventData).detail; }
};
Object.defineProperty(globalThis.UIEvent.prototype, Symbol.toStringTag, { value: 'UIEvent', writable: false, enumerable: false, configurable: true });

const __tbMouseEventData = Symbol.for('tinybrowser.mouseevent.data');
globalThis.MouseEvent = class MouseEvent extends UIEvent {
  constructor(type, init) {
    if (arguments.length < 1) {
      throw new TypeError("Failed to construct 'MouseEvent': 1 argument required, but only 0 present.");
    }
    init = init || {};
    super(type, init);
    const button = init.button === undefined ? 0 : init.button;
    Object.defineProperty(this, __tbMouseEventData, {
      value: {
        screenX: init.screenX || 0, screenY: init.screenY || 0,
        clientX: init.clientX || 0, clientY: init.clientY || 0,
        ctrlKey: !!init.ctrlKey, shiftKey: !!init.shiftKey,
        altKey: !!init.altKey, metaKey: !!init.metaKey,
        button: button, buttons: init.buttons === undefined ? 0 : init.buttons,
        relatedTarget: init.relatedTarget || null,
      },
      writable: false, enumerable: false, configurable: false,
    });
  }
  get screenX() { return __tbBrand(this, __tbMouseEventData).screenX; }
  get screenY() { return __tbBrand(this, __tbMouseEventData).screenY; }
  get clientX() { return __tbBrand(this, __tbMouseEventData).clientX; }
  get clientY() { return __tbBrand(this, __tbMouseEventData).clientY; }
  get ctrlKey() { return __tbBrand(this, __tbMouseEventData).ctrlKey; }
  get shiftKey() { return __tbBrand(this, __tbMouseEventData).shiftKey; }
  get altKey() { return __tbBrand(this, __tbMouseEventData).altKey; }
  get metaKey() { return __tbBrand(this, __tbMouseEventData).metaKey; }
  get button() { return __tbBrand(this, __tbMouseEventData).button; }
  get buttons() { return __tbBrand(this, __tbMouseEventData).buttons; }
  get relatedTarget() { return __tbBrand(this, __tbMouseEventData).relatedTarget; }
  getModifierState(key) {
    return { Alt: !!this.altKey, Control: !!this.ctrlKey, Meta: !!this.metaKey, Shift: !!this.shiftKey }[String(key)] || false;
  }
};
Object.defineProperty(globalThis.MouseEvent.prototype, Symbol.toStringTag, { value: 'MouseEvent', writable: false, enumerable: false, configurable: true });

const __tbPointerEventData = Symbol.for('tinybrowser.pointerevent.data');
globalThis.PointerEvent = class PointerEvent extends MouseEvent {
  constructor(type, init) {
    if (arguments.length < 1) {
      throw new TypeError("Failed to construct 'PointerEvent': 1 argument required, but only 0 present.");
    }
    init = init || {};
    super(type, init);
    Object.defineProperty(this, __tbPointerEventData, {
      value: {
        pointerId: init.pointerId === undefined ? 1 : init.pointerId,
        width: init.width || 1, height: init.height || 1,
        pressure: init.pressure === undefined ? 0 : init.pressure,
        tangentialPressure: init.tangentialPressure || 0,
        tiltX: init.tiltX || 0, tiltY: init.tiltY || 0, twist: init.twist || 0,
        pointerType: init.pointerType || 'mouse',
        isPrimary: init.isPrimary === undefined ? true : !!init.isPrimary,
      },
      writable: false, enumerable: false, configurable: false,
    });
  }
  get pointerId() { return __tbBrand(this, __tbPointerEventData).pointerId; }
  get width() { return __tbBrand(this, __tbPointerEventData).width; }
  get height() { return __tbBrand(this, __tbPointerEventData).height; }
  get pressure() { return __tbBrand(this, __tbPointerEventData).pressure; }
  get tangentialPressure() { return __tbBrand(this, __tbPointerEventData).tangentialPressure; }
  get tiltX() { return __tbBrand(this, __tbPointerEventData).tiltX; }
  get tiltY() { return __tbBrand(this, __tbPointerEventData).tiltY; }
  get twist() { return __tbBrand(this, __tbPointerEventData).twist; }
  get pointerType() { return __tbBrand(this, __tbPointerEventData).pointerType; }
  get isPrimary() { return __tbBrand(this, __tbPointerEventData).isPrimary; }
};
Object.defineProperty(globalThis.PointerEvent.prototype, Symbol.toStringTag, { value: 'PointerEvent', writable: false, enumerable: false, configurable: true });

const __tbWheelEventData = Symbol.for('tinybrowser.wheelevent.data');
globalThis.WheelEvent = class WheelEvent extends MouseEvent {
  constructor(type, init) {
    if (arguments.length < 1) {
      throw new TypeError("Failed to construct 'WheelEvent': 1 argument required, but only 0 present.");
    }
    init = init || {};
    super(type, init);
    Object.defineProperty(this, __tbWheelEventData, {
      value: {
        deltaX: init.deltaX || 0, deltaY: init.deltaY || 0, deltaZ: init.deltaZ || 0,
        deltaMode: init.deltaMode || 0,
      },
      writable: false, enumerable: false, configurable: false,
    });
  }
  get deltaX() { return __tbBrand(this, __tbWheelEventData).deltaX; }
  get deltaY() { return __tbBrand(this, __tbWheelEventData).deltaY; }
  get deltaZ() { return __tbBrand(this, __tbWheelEventData).deltaZ; }
  get deltaMode() { return __tbBrand(this, __tbWheelEventData).deltaMode; }
};
Object.defineProperty(globalThis.WheelEvent.prototype, Symbol.toStringTag, { value: 'WheelEvent', writable: false, enumerable: false, configurable: true });

const __tbKeyboardEventData = Symbol.for('tinybrowser.keyboardevent.data');
globalThis.KeyboardEvent = class KeyboardEvent extends UIEvent {
  constructor(type, init) {
    if (arguments.length < 1) {
      throw new TypeError("Failed to construct 'KeyboardEvent': 1 argument required, but only 0 present.");
    }
    init = init || {};
    super(type, init);
    const key = init.key === undefined ? '' : String(init.key);
    Object.defineProperty(this, __tbKeyboardEventData, {
      value: {
        key: key,
        code: init.code === undefined ? '' : String(init.code),
        location: init.location || 0,
        ctrlKey: !!init.ctrlKey, shiftKey: !!init.shiftKey,
        altKey: !!init.altKey, metaKey: !!init.metaKey,
        repeat: !!init.repeat, isComposing: !!init.isComposing,
        keyCode: init.keyCode === undefined ? key.charCodeAt(0) : init.keyCode,
        charCode: init.charCode || 0,
      },
      writable: false, enumerable: false, configurable: false,
    });
  }
  get key() { return __tbBrand(this, __tbKeyboardEventData).key; }
  get code() { return __tbBrand(this, __tbKeyboardEventData).code; }
  get location() { return __tbBrand(this, __tbKeyboardEventData).location; }
  get ctrlKey() { return __tbBrand(this, __tbKeyboardEventData).ctrlKey; }
  get shiftKey() { return __tbBrand(this, __tbKeyboardEventData).shiftKey; }
  get altKey() { return __tbBrand(this, __tbKeyboardEventData).altKey; }
  get metaKey() { return __tbBrand(this, __tbKeyboardEventData).metaKey; }
  get repeat() { return __tbBrand(this, __tbKeyboardEventData).repeat; }
  get isComposing() { return __tbBrand(this, __tbKeyboardEventData).isComposing; }
  get keyCode() { return __tbBrand(this, __tbKeyboardEventData).keyCode; }
  get charCode() { return __tbBrand(this, __tbKeyboardEventData).charCode; }
  getModifierState(key) {
    return { Alt: !!this.altKey, Control: !!this.ctrlKey, Meta: !!this.metaKey, Shift: !!this.shiftKey }[String(key)] || false;
  }
};
Object.defineProperty(globalThis.KeyboardEvent.prototype, Symbol.toStringTag, { value: 'KeyboardEvent', writable: false, enumerable: false, configurable: true });

const __tbInputEventData = Symbol.for('tinybrowser.inputevent.data');
globalThis.InputEvent = class InputEvent extends UIEvent {
  constructor(type, init) {
    if (arguments.length < 1) {
      throw new TypeError("Failed to construct 'InputEvent': 1 argument required, but only 0 present.");
    }
    init = init || {};
    super(type, init);
    Object.defineProperty(this, __tbInputEventData, {
      value: {
        data: init.data === undefined ? null : init.data,
        inputType: init.inputType === undefined ? '' : String(init.inputType),
        isComposing: !!init.isComposing,
        dataTransfer: init.dataTransfer || null,
      },
      writable: false, enumerable: false, configurable: false,
    });
  }
  get data() { return __tbBrand(this, __tbInputEventData).data; }
  get inputType() { return __tbBrand(this, __tbInputEventData).inputType; }
  get isComposing() { return __tbBrand(this, __tbInputEventData).isComposing; }
  get dataTransfer() { return __tbBrand(this, __tbInputEventData).dataTransfer; }
};
Object.defineProperty(globalThis.InputEvent.prototype, Symbol.toStringTag, { value: 'InputEvent', writable: false, enumerable: false, configurable: true });

// The WebDriver Perform Actions sequence
// (<https://w3c.github.io/webdriver/#perform-actions>). Element origins arrive
// rewritten by the WebDriver crate as `{__tbRemote: <number>}`; ticks from
// every source run in order. Durations are treated as zero: the engine has no
// per-frame interpolation yet.
(function() {
  const pointerStates = new Map();
  const pointerState = id => {
    if (!pointerStates.has(id)) pointerStates.set(id, { x: 0, y: 0, buttons: 0, target: null });
    return pointerStates.get(id);
  };
  const at = (x, y) => {
    const found = document.elementFromPoint ? document.elementFromPoint(x, y) : null;
    return found || document.documentElement || document.body;
  };
  const centerOf = origin => {
    if (!origin || typeof origin !== 'object' || typeof origin.__tbRemote !== 'number') return null;
    const element = globalThis.__tb_webdriver_element(origin.__tbRemote);
    if (!element || typeof element.getBoundingClientRect !== 'function') return null;
    const rect = element.getBoundingClientRect();
    return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
  };
  const mouseInit = (x, y, buttons, extra) => Object.assign({
    bubbles: true, cancelable: true, composed: true, view: globalThis,
    clientX: x, clientY: y, screenX: x, screenY: y, buttons: buttons || 0,
  }, extra || {});
  const fire = (node, event) => { if (node) node.dispatchEvent(event); };
  const pointerItem = (state, item) => {
    if (item.type === 'pointerMove') {
      const center = centerOf(item.origin);
      let x, y;
      if (center) { x = center.x + (item.x || 0); y = center.y + (item.y || 0); }
      else if (item.origin === 'pointer') { x = state.x + (item.x || 0); y = state.y + (item.y || 0); }
      else { x = item.x || 0; y = item.y || 0; }
      const next = at(x, y);
      if (state.target && state.target !== next) {
        fire(state.target, new PointerEvent('pointerout', mouseInit(state.x, state.y, state.buttons)));
        fire(state.target, new PointerEvent('pointerleave', mouseInit(state.x, state.y, state.buttons)));
        fire(state.target, new MouseEvent('mouseout', mouseInit(state.x, state.y, state.buttons)));
        fire(state.target, new MouseEvent('mouseleave', mouseInit(state.x, state.y, state.buttons)));
        fire(next, new PointerEvent('pointerover', mouseInit(x, y, state.buttons)));
        fire(next, new PointerEvent('pointerenter', mouseInit(x, y, state.buttons)));
        fire(next, new MouseEvent('mouseover', mouseInit(x, y, state.buttons)));
        fire(next, new MouseEvent('mouseenter', mouseInit(x, y, state.buttons)));
      }
      state.x = x; state.y = y; state.target = next;
      fire(next, new PointerEvent('pointermove', mouseInit(x, y, state.buttons)));
      fire(next, new MouseEvent('mousemove', mouseInit(x, y, state.buttons)));
    } else if (item.type === 'pointerDown') {
      const button = item.button || 0;
      state.buttons |= 1 << button;
      const node = at(state.x, state.y);
      state.target = node;
      if (node && typeof node.focus === 'function') { try { node.focus(); } catch (error) {} }
      fire(node, new PointerEvent('pointerdown', mouseInit(state.x, state.y, state.buttons, { button: button })));
      fire(node, new MouseEvent('mousedown', mouseInit(state.x, state.y, state.buttons, { button: button })));
    } else if (item.type === 'pointerUp') {
      const button = item.button || 0;
      const node = at(state.x, state.y);
      fire(node, new PointerEvent('pointerup', mouseInit(state.x, state.y, state.buttons, { button: button })));
      fire(node, new MouseEvent('mouseup', mouseInit(state.x, state.y, state.buttons, { button: button })));
      if (button === 0) fire(node, new MouseEvent('click', mouseInit(state.x, state.y, 0, { button: 0 })));
      state.buttons &= ~(1 << button);
    } else if (item.type === 'pointerCancel') {
      fire(at(state.x, state.y), new PointerEvent('pointercancel', mouseInit(state.x, state.y, state.buttons)));
      state.buttons = 0;
    }
  };
  const specialKeys = {
    '\uE003': ['Backspace', 'Backspace'], '\uE004': ['Tab', 'Tab'],
    '\uE006': ['Enter', 'Enter'], '\uE007': ['Enter', 'Enter'],
    '\uE008': ['Shift', 'ShiftLeft'], '\uE00C': ['Escape', 'Escape'],
    '\uE00D': [' ', 'Space'], '\uE00E': ['PageUp', 'PageUp'], '\uE00F': ['PageDown', 'PageDown'],
    '\uE010': ['End', 'End'], '\uE011': ['Home', 'Home'],
    '\uE012': ['ArrowLeft', 'ArrowLeft'], '\uE013': ['ArrowUp', 'ArrowUp'],
    '\uE014': ['ArrowRight', 'ArrowRight'], '\uE015': ['ArrowDown', 'ArrowDown'],
    '\uE017': ['Delete', 'Delete'],
  };
  const keyOf = value => {
    if (specialKeys[value]) return { key: specialKeys[value][0], code: specialKeys[value][1] };
    if (value.length === 1) {
      const code = value >= 'a' && value <= 'z' ? 'Key' + value.toUpperCase()
        : value >= 'A' && value <= 'Z' ? 'Key' + value
        : value >= '0' && value <= '9' ? 'Digit' + value
        : '';
      return { key: value, code: code };
    }
    return { key: value, code: value };
  };
  const insertText = text => {
    const element = document.activeElement;
    if (!element) return;
    const tag = element.tagName;
    if (tag === 'INPUT' || tag === 'TEXTAREA') {
      const start = element.selectionStart === undefined || element.selectionStart === null ? element.value.length : element.selectionStart;
      const end = element.selectionEnd === undefined || element.selectionEnd === null ? start : element.selectionEnd;
      if (typeof element.setRangeText === 'function') element.setRangeText(text, start, end, 'end');
      else element.value = element.value.slice(0, start) + text + element.value.slice(end);
      fire(element, new InputEvent('beforeinput', { bubbles: true, cancelable: true, inputType: 'insertText', data: text }));
      fire(element, new InputEvent('input', { bubbles: true, inputType: 'insertText', data: text }));
    } else if (element.isContentEditable) {
      element.textContent = (element.textContent || '') + text;
      fire(element, new InputEvent('input', { bubbles: true, inputType: 'insertText', data: text }));
    }
  };
  const keyItem = (item, down) => {
    const value = item.value === undefined ? '' : String(item.value);
    const info = keyOf(value);
    const node = document.activeElement || document.body || document.documentElement;
    if (down) {
      fire(node, new KeyboardEvent('keydown', { bubbles: true, cancelable: true, key: info.key, code: info.code }));
      if (value.length === 1 || value === '\uE00D') insertText(value === '\uE00D' ? ' ' : value);
      if (value.length === 1) fire(node, new KeyboardEvent('keypress', { bubbles: true, cancelable: true, key: info.key, code: info.code }));
    } else {
      fire(node, new KeyboardEvent('keyup', { bubbles: true, cancelable: true, key: info.key, code: info.code }));
    }
  };
  const wheelItem = item => {
    const x = item.x || 0;
    const y = item.y || 0;
    const node = at(x, y);
    const accepted = fire(node, new WheelEvent('wheel', mouseInit(x, y, 0, {
      deltaX: item.deltaX || 0, deltaY: item.deltaY || 0,
    })));
    if (typeof node.scrollBy === 'function') node.scrollBy(item.deltaX || 0, item.deltaY || 0);
  };
  globalThis.__tbWebDriverActions = function(actions) {
    let ticks = 0;
    for (const source of actions) {
      const count = source.actions ? source.actions.length : 0;
      if (count > ticks) ticks = count;
    }
    for (let tick = 0; tick < ticks; tick++) {
      for (const source of actions) {
        const item = source.actions ? source.actions[tick] : null;
        if (!item || item.type === 'pause') continue;
        if (source.type === 'pointer') pointerItem(pointerState(source.id), item);
        else if (source.type === 'key') keyItem(item, item.type === 'keyDown');
        else if (source.type === 'wheel') wheelItem(item);
      }
    }
    return true;
  };
  Object.defineProperty(globalThis, '__tbWebDriverActions', {
    writable: false, configurable: false, enumerable: false,
  });
})();
