// https://fetch.spec.whatwg.org/#headers-class
// Page requests share the browser process's HTTP client with navigation.
const __tbHeadersData = host.slots('tinybrowser.headers.data');
const __tbResponseData = host.slots('tinybrowser.response.data');
const __tbRequestData = host.slots('tinybrowser.request.data');
// https://fetch.spec.whatwg.org/#concept-method-normalize
const __tbRequestMethod = value => {
  const method = String(value);
  if (!/^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/.test(method) || /^(CONNECT|TRACE|TRACK)$/i.test(method)) {
    throw new TypeError('Invalid HTTP method');
  }
  return /^(DELETE|GET|HEAD|OPTIONS|POST|PUT)$/i.test(method) ? method.toUpperCase() : method;
};
const __tbBrand = (value, slots, message) => {
  const data = slots.get(value);
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
           __tbArray.push(entries, [String(values[0]).toLowerCase(), String(values[1]).trim()]);
        }
      } else {
        for (const name of Object.keys(init)) __tbArray.push(entries, [name.toLowerCase(), String(init[name]).trim()]);
      }
    }
    __tbHeadersData.set(this, entries);
  }
  append(name, value) { __tbArray.push(__tbBrand(this, __tbHeadersData), [String(name).toLowerCase(), String(value).trim()]); }
  delete(name) {
    const entries = __tbBrand(this, __tbHeadersData);
    name = String(name).toLowerCase();
    for (let index = entries.length - 1; index >= 0; index--) {
      if (entries[index][0] === name) __tbArray.remove(entries, index);
    }
  }
  get(name) {
    const entries = __tbBrand(this, __tbHeadersData);
    name = String(name).toLowerCase();
    // Duplicate values combine with ', '
    // (<https://fetch.spec.whatwg.org/#dom-headers-get>).
    const values = [];
    for (let index = 0; index < entries.length; index++) {
      if (entries[index][0] === name) values[values.length] = entries[index][1];
    }
    return values.length === 0 ? null : __tbArray.join(values, ', ');
  }
  has(name) { return this.get(name) !== null; }
  set(name, value) {
    this.delete(name);
    __tbArray.push(__tbBrand(this, __tbHeadersData), [String(name).toLowerCase(), String(value).trim()]);
  }
  forEach(callback, thisArg) {
    const entries = __tbBrand(this, __tbHeadersData);
    for (let index = 0; index < entries.length; index++) {
      __tbApply(callback, thisArg, [entries[index][1], entries[index][0], this]);
    }
  }
  entries() { return __tbArray.iterator(__tbArray.map(__tbBrand(this, __tbHeadersData), entry => [entry[0], entry[1]])); }
  keys() { return __tbArray.iterator(__tbArray.map(__tbBrand(this, __tbHeadersData), entry => entry[0])); }
  values() { return __tbArray.iterator(__tbArray.map(__tbBrand(this, __tbHeadersData), entry => entry[1])); }
  [Symbol.iterator]() { return this.entries(); }
};
Object.defineProperty(globalThis.Headers.prototype, Symbol.toStringTag, { value: 'Headers', writable: false, enumerable: false, configurable: true });
const __tbHeadersConstructor = globalThis.Headers;
const __tbHeadersGet = __tbHeadersConstructor.prototype.get;
const __tbHeadersAppend = __tbHeadersConstructor.prototype.append;
// https://fetch.spec.whatwg.org/#response-class
globalThis.Response = class Response {
  constructor(body, init) {
    const options = init === undefined ? {} : Object(init);
    const storedBody = body === undefined || body === null ? '' : body;
    __tbResponseData.set(this, {
        body: storedBody,
        bodyUsed: false,
        status: options.status === undefined ? 200 : Number(options.status),
        statusText: options.statusText === undefined ? '' : String(options.statusText),
        url: options.url === undefined ? '' : String(options.url),
        headers: options.headers instanceof globalThis.Headers ? options.headers : new globalThis.Headers(options.headers),
    });
  }
  get status() { return __tbBrand(this, __tbResponseData).status; }
  get statusText() { return __tbBrand(this, __tbResponseData).statusText; }
  get url() { return __tbBrand(this, __tbResponseData).url; }
  get headers() { return __tbBrand(this, __tbResponseData).headers; }
  get ok() { const status = __tbBrand(this, __tbResponseData).status; return status >= 200 && status <= 299; }
  get bodyUsed() { return __tbBrand(this, __tbResponseData).bodyUsed; }
  text() { return __tbConsumeResponse(this).then(bytes => __tbDecodeBytes(bytes, 'utf-8', false, false)); }
  json() {
    return this.text().then(JSON.parse);
  }
  arrayBuffer() { return __tbConsumeResponse(this).then(bytes => bytes.buffer); }
  blob() {
    const data = __tbBrand(this, __tbResponseData);
    const type = data.headers.get('content-type');
    return __tbConsumeResponse(this).then(bytes => new Blob([bytes], { type: type === null ? '' : type }));
  }
  clone() {
    const data = __tbBrand(this, __tbResponseData);
    if (data.bodyUsed) throw new TypeError('body already used');
    return new Response(data.body, { status: data.status, statusText: data.statusText, url: data.url, headers: data.headers });
  }
};
const __tbConsumeResponse = response => {
  const data = __tbBrand(response, __tbResponseData);
  if (data.bodyUsed) return Promise.reject(new TypeError('body already used'));
  data.bodyUsed = true;
  return __tbResponseBytes(data.body);
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
  const contents = host.__tbObjectUrlContents(key);
  if (contents === null || contents === undefined) return null;
  const content_type = host.__tbObjectUrlType(key);
  return { contents, type: content_type === null || content_type === undefined ? '' : content_type };
};
globalThis.Request = class Request {
  constructor(input, init) {
    const options = init === undefined ? {} : Object(init);
    let url;
    let method = 'GET';
    let blob = null;
    let headers = new globalThis.Headers();
    let body = null;
    if (input instanceof globalThis.Request) {
      const source = __tbBrand(input, __tbRequestData);
      url = source.url;
      method = source.method;
      blob = source.blob;
      headers = new globalThis.Headers(source.headers);
      body = source.body;
    } else {
      const resolved = host.__tbResolveUrl(String(input), undefined);
      url = resolved === null ? String(input) : resolved;
    }
    if (options.method !== undefined) method = __tbRequestMethod(options.method);
    if (options.headers !== undefined) headers = new globalThis.Headers(options.headers);
    if (options.body !== undefined) body = options.body;
    // A Request keeps a blob URL's data alive, so fetching it still works
    // after revokeObjectURL (<https://w3c.github.io/FileAPI/#lifeTime>).
    if (blob === null && url.indexOf('blob:') === 0) blob = __tbLookupObjectUrl(url);
    __tbRequestData.set(this, { url, method, blob, headers, body });
  }
  get url() { return __tbBrand(this, __tbRequestData).url; }
  get method() { return __tbBrand(this, __tbRequestData).method; }
  get headers() { return __tbBrand(this, __tbRequestData).headers; }
  clone() {
    const data = __tbBrand(this, __tbRequestData);
    const copy = Object.create(globalThis.Request.prototype);
    __tbRequestData.set(copy, { url: data.url, method: data.method, blob: data.blob, headers: new globalThis.Headers(data.headers), body: data.body });
    return copy;
  }
};
Object.defineProperty(globalThis.Request.prototype, Symbol.toStringTag, { value: 'Request', writable: false, enumerable: false, configurable: true });
const __tbMakeResponse = (body, status, url, type, fields) => {
  const headers = new globalThis.Headers(fields);
  if (type && !headers.has('content-type')) headers.set('content-type', type);
  return new globalThis.Response(body, { status, url, headers });
};
// <https://fetch.spec.whatwg.org/#bodyinit-safely-extract>
const __tbPageRequestBody = async body => {
  if (body === null || body === undefined) return { bytes: [], type: null };
  if (body instanceof Blob) return { bytes: Array.from(new Uint8Array(await body.arrayBuffer())), type: body.type || null };
  if (body instanceof ArrayBuffer) return { bytes: Array.from(new Uint8Array(body)), type: null };
  if (ArrayBuffer.isView(body)) return { bytes: Array.from(new Uint8Array(body.buffer, body.byteOffset, body.byteLength)), type: null };
  if (body instanceof URLSearchParams) {
    return { bytes: Array.from(__tbUtf8Encode(String(body))), type: 'application/x-www-form-urlencoded;charset=UTF-8' };
  }
  return { bytes: Array.from(__tbUtf8Encode(String(body))), type: 'text/plain;charset=UTF-8' };
};
const __tbQueuePageRequest = (url, method, body, headers, callback) => {
  const id = ++host.__tb_fetchSeq;
  // Snapshot the document referrer at `fetch()` time. Body extraction is
  // async, so a later `meta name=referrer` insertion would otherwise apply
  // to this request. The request URL goes along: the policy (same-origin,
  // downgrade) is decided against it
  // (<https://w3c.github.io/webappsec-referrer-policy/#determine-requests-referrer>).
  // Edge case: `new Request(url)` stores no referrer, so `fetch(req)` uses
  // fetch-time (not construction-time) policy; construction-time state is a
  // known gap.
  const referrer = host.__tbReferrer(url);
  host.__tb_fetchCbs[id] = function(status, bytes, finalUrl, contentType, responseHeaders) {
    delete host.__tb_fetchCbs[id];
    callback(status, bytes, finalUrl, contentType, responseHeaders);
  };
  __tbPageRequestBody(body).then(payload => {
    if (!host.__tb_fetchCbs[id]) return;
    const type = __tbApply(__tbHeadersGet, headers, ['content-type']) === null ? payload.type : null;
    const fields = __tbArray.map(__tbBrand(headers, __tbHeadersData), entry => [entry[0], entry[1]]);
    host.__queueFetch(url, id, method, payload.bytes, type, fields, referrer);
  }, () => {
    const done = host.__tb_fetchCbs[id];
    if (done) done(0, new Uint8Array(), '', '', []);
  });
  return id;
};
// https://fetch.spec.whatwg.org/#fetch-controller
const __tbCancelPageRequest = id => {
  if (id === null) return;
  delete host.__tb_fetchCbs[id];
  host.__cancelFetch(id);
};
globalThis.fetch = function(input, init) {
  const options = init === undefined ? {} : Object(init);
  let url;
  let method = 'GET';
  let blob = null;
  let headers = new globalThis.Headers();
  let body = null;
  if (input instanceof globalThis.Request) {
    const source = __tbBrand(input, __tbRequestData);
    url = source.url;
    method = source.method;
    blob = source.blob;
    headers = new globalThis.Headers(source.headers);
    body = source.body;
  } else {
    const resolved = host.__tbResolveUrl(String(input), undefined);
    url = resolved === null ? String(input) : resolved;
  }
  if (options.method !== undefined) {
    try { method = __tbRequestMethod(options.method); }
    catch (error) { return Promise.reject(error); }
  }
  if (options.headers !== undefined) headers = new globalThis.Headers(options.headers);
  if (options.body !== undefined) body = options.body;
  return new Promise(function(resolve, reject) {
    if (url.indexOf('blob:') === 0) {
      if (method !== 'GET' || body !== null) {
        reject(new TypeError('Failed to fetch'));
        return;
      }
      const entry = blob === null ? __tbLookupObjectUrl(url) : blob;
      if (entry === null) {
        reject(new TypeError('Failed to fetch'));
        return;
      }
      resolve(__tbMakeResponse(entry.contents, 200, url, entry.type, []));
      return;
    }
    __tbQueuePageRequest(url, method, body, headers, (status, bytes, finalUrl, type, fields) => {
      if (status !== 0) resolve(__tbMakeResponse(bytes, status, finalUrl, type, fields));
      else reject(new TypeError('Failed to fetch'));
    });
  });
};
// https://w3c.github.io/FileAPI/#blob
