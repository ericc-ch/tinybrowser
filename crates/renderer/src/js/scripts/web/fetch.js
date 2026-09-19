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
    // Duplicate values combine with ', '
    // (<https://fetch.spec.whatwg.org/#dom-headers-get>).
    const values = entries.filter(entry => entry[0] === name).map(entry => entry[1]);
    return values.length === 0 ? null : values.join(', ');
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
    const storedBody = body === undefined || body === null ? '' : body;
    Object.defineProperty(this, __tbResponseData, {
      value: {
        body: storedBody,
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
  text() { return __tbResponseBytes(__tbBrand(this, __tbResponseData).body).then(bytes => __tbDecodeBytes(bytes, 'utf-8', false, false)); }
  json() {
    return this.text().then(JSON.parse);
  }
  arrayBuffer() { return __tbResponseBytes(__tbBrand(this, __tbResponseData).body).then(bytes => bytes.buffer); }
  blob() {
    const data = __tbBrand(this, __tbResponseData);
    const type = data.headers.get('content-type');
    return __tbResponseBytes(data.body).then(bytes => new Blob([bytes], { type: type === null ? '' : type }));
  }
  clone() {
    const data = __tbBrand(this, __tbResponseData);
    return new Response(data.body, { status: data.status, statusText: data.statusText, url: data.url, headers: data.headers });
  }
};
const __tbResponseBytes = async body => {
  if (body instanceof ReadableStream) {
    const chunks = [];
    let length = 0;
    const reader = body.getReader();
    while (true) {
      const step = await reader.read();
      if (step.done) break;
      const chunk = typeof step.value === 'string' ? __tbUtf8Encode(step.value)
        : step.value instanceof Uint8Array ? step.value
        : ArrayBuffer.isView(step.value) ? new Uint8Array(step.value.buffer, step.value.byteOffset, step.value.byteLength)
        : step.value instanceof ArrayBuffer ? new Uint8Array(step.value)
        : __tbUtf8Encode(String(step.value));
      chunks.push(chunk);
      length += chunk.length;
    }
    const bytes = new Uint8Array(length);
    let offset = 0;
    for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.length; }
    return bytes;
  }
  if (body instanceof Blob) return new Uint8Array(await body.arrayBuffer());
  if (body instanceof ArrayBuffer) return new Uint8Array(body.slice(0));
  if (ArrayBuffer.isView(body)) return new Uint8Array(body.buffer, body.byteOffset, body.byteLength).slice();
  return __tbUtf8Encode(String(body));
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
