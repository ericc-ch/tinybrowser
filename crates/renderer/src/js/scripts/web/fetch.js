// https://fetch.spec.whatwg.org/#headers-class
// Page requests share the browser process's HTTP client with navigation.
const __tbHeadersData = host.slots('tinybrowser.headers.data');
const __tbResponseData = host.slots('tinybrowser.response.data');
const __tbRequestData = host.slots('tinybrowser.request.data');
// A header value is a byte sequence with no 0x00, 0x0A, or 0x0D, and a JS
// string holds it with one code point per byte
// (<https://fetch.spec.whatwg.org/#concept-header-value>).
const __tbHeaderValue = value => {
  value = String(value).replace(/^[\t\n\r ]+|[\t\n\r ]+$/g, '');
  for (let index = 0; index < value.length; index++) {
    const code = value.charCodeAt(index);
    if (code === 0x00 || code === 0x0A || code === 0x0D || code > 0xFF) throw new TypeError('Invalid header value');
  }
  return value;
};
// https://fetch.spec.whatwg.org/#concept-method-normalize
const __tbRequestMethod = value => {
  const method = String(value);
  if (!/^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/.test(method) || /^(CONNECT|TRACE|TRACK)$/i.test(method)) {
    throw new TypeError('Invalid HTTP method');
  }
  return /^(DELETE|GET|HEAD|OPTIONS|POST|PUT)$/i.test(method) ? method.toUpperCase() : method;
};
// Request mode, credentials mode, and redirect mode
// (<https://fetch.spec.whatwg.org/#request-class>).
const __tbRequestMode = value => {
  const mode = String(value);
  if (mode === 'same-origin' || mode === 'cors' || mode === 'no-cors') return mode;
  throw new TypeError('Invalid request mode');
};
const __tbCredentialsMode = value => {
  const mode = String(value);
  if (mode === 'omit' || mode === 'same-origin' || mode === 'include') return mode;
  throw new TypeError('Invalid credentials mode');
};
const __tbRedirectMode = value => {
  const mode = String(value);
  if (mode === 'follow' || mode === 'error' || mode === 'manual') return mode;
  throw new TypeError('Invalid redirect mode');
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
           __tbArray.push(entries, [String(values[0]).toLowerCase(), __tbHeaderValue(values[1])]);
        }
      } else {
        for (const name of Object.keys(init)) __tbArray.push(entries, [name.toLowerCase(), __tbHeaderValue(init[name])]);
      }
    }
    __tbHeadersData.set(this, entries);
  }
  append(name, value) { __tbArray.push(__tbBrand(this, __tbHeadersData), [String(name).toLowerCase(), __tbHeaderValue(value)]); }
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
    __tbArray.push(__tbBrand(this, __tbHeadersData), [String(name).toLowerCase(), __tbHeaderValue(value)]);
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
    const storedBody = body === undefined || body === null ? null : body;
    const headers = new globalThis.Headers(options.headers);
    // The body's type becomes the Content-Type when the header list holds
    // none (<https://fetch.spec.whatwg.org/#dom-response>).
    if (storedBody !== null && !headers.has('content-type')) {
      const type = __tbBodyType(storedBody);
      if (type !== null) headers.set('content-type', type);
    }
    __tbResponseData.set(this, {
        body: storedBody,
        bodyUsed: false,
        status: options.status === undefined ? 200 : Number(options.status),
        statusText: options.statusText === undefined ? '' : String(options.statusText),
        url: options.url === undefined ? '' : String(options.url),
        // `ResponseInit` has no `type` member; the constructor always yields
        // "default". Only the network path sets another type, through
        // `__tbMakeResponse`
        // (<https://fetch.spec.whatwg.org/#responseinit>).
        type: 'default',
        headers,
    });
  }
  get status() { return __tbBrand(this, __tbResponseData).status; }
  get statusText() { return __tbBrand(this, __tbResponseData).statusText; }
  get url() { return __tbBrand(this, __tbResponseData).url; }
  get type() { return __tbBrand(this, __tbResponseData).type; }
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
    const header = data.headers.get('content-type');
    // The Blob type is the MIME type extracted from the header list
    // (<https://fetch.spec.whatwg.org/#concept-header-extract-mime-type>).
    const type = header === null ? '' : host.__tbExtractMimeType(header);
    return __tbConsumeResponse(this).then(bytes => __tbMakeBlob(bytes, type));
  }
  clone() {
    const data = __tbBrand(this, __tbResponseData);
    if (data.bodyUsed) throw new TypeError('body already used');
    const copy = Object.create(globalThis.Response.prototype);
    __tbResponseData.set(copy, { body: data.body, bodyUsed: false, status: data.status, statusText: data.statusText, url: data.url, type: data.type, headers: new globalThis.Headers(data.headers) });
    return copy;
  }
};
const __tbConsumeResponse = response => {
  const data = __tbBrand(response, __tbResponseData);
  if (data.bodyUsed) return Promise.reject(new TypeError('body already used'));
  data.bodyUsed = true;
  return __tbResponseBytes(data.body);
};
const __tbResponseBytes = async body => {
  if (body === null) return new Uint8Array(0);
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
    let mode = 'cors';
    let credentials = 'same-origin';
    let redirect = 'follow';
    if (input instanceof globalThis.Request) {
      const source = __tbBrand(input, __tbRequestData);
      url = source.url;
      method = source.method;
      blob = source.blob;
      headers = new globalThis.Headers(source.headers);
      body = source.body;
      mode = source.mode;
      credentials = source.credentials;
      redirect = source.redirect;
    } else {
      const resolved = host.__tbResolveUrl(String(input), undefined);
      url = resolved === null ? String(input) : resolved;
    }
    if (options.method !== undefined) method = __tbRequestMethod(options.method);
    if (options.headers !== undefined) headers = new globalThis.Headers(options.headers);
    if (options.body !== undefined) body = options.body;
    if (options.mode !== undefined) mode = __tbRequestMode(options.mode);
    if (options.credentials !== undefined) credentials = __tbCredentialsMode(options.credentials);
    if (options.redirect !== undefined) redirect = __tbRedirectMode(options.redirect);
    // The init body's type becomes the Content-Type when the header list
    // holds none (<https://fetch.spec.whatwg.org/#dom-request>).
    if (options.body !== undefined && options.body !== null && !headers.has('content-type')) {
      const type = __tbBodyType(options.body);
      if (type !== null) headers.set('content-type', type);
    }
    // A Request keeps a blob URL's data alive, so fetching it still works
    // after revokeObjectURL (<https://w3c.github.io/FileAPI/#lifeTime>).
    if (blob === null && url.indexOf('blob:') === 0) blob = __tbLookupObjectUrl(url);
    __tbRequestData.set(this, { url, method, blob, headers, body, bodyUsed: false, mode, credentials, redirect });
  }
  get url() { return __tbBrand(this, __tbRequestData).url; }
  get method() { return __tbBrand(this, __tbRequestData).method; }
  get headers() { return __tbBrand(this, __tbRequestData).headers; }
  get bodyUsed() { return __tbBrand(this, __tbRequestData).bodyUsed; }
  get mode() { return __tbBrand(this, __tbRequestData).mode; }
  get credentials() { return __tbBrand(this, __tbRequestData).credentials; }
  get redirect() { return __tbBrand(this, __tbRequestData).redirect; }
  // https://fetch.spec.whatwg.org/#dom-body-blob
  blob() {
    const data = __tbBrand(this, __tbRequestData);
    if (data.bodyUsed) return Promise.reject(new TypeError('body already used'));
    data.bodyUsed = true;
    const header = data.headers.get('content-type');
    // The Blob type is the MIME type extracted from the header list
    // (<https://fetch.spec.whatwg.org/#concept-header-extract-mime-type>).
    const type = header === null ? '' : host.__tbExtractMimeType(header);
    const bytes = data.body === null ? Promise.resolve(new Uint8Array(0)) : __tbResponseBytes(data.body);
    return bytes.then(value => __tbMakeBlob(value, type));
  }
  clone() {
    const data = __tbBrand(this, __tbRequestData);
    const copy = Object.create(globalThis.Request.prototype);
    __tbRequestData.set(copy, { url: data.url, method: data.method, blob: data.blob, headers: new globalThis.Headers(data.headers), body: data.body, bodyUsed: false, mode: data.mode, credentials: data.credentials, redirect: data.redirect });
    return copy;
  }
};
Object.defineProperty(globalThis.Request.prototype, Symbol.toStringTag, { value: 'Request', writable: false, enumerable: false, configurable: true });
const __tbMakeResponse = (body, status, url, type, fields, responseType) => {
  const headers = new globalThis.Headers(fields);
  if (type && !headers.has('content-type')) headers.set('content-type', type);
  const response = new globalThis.Response(body, { status, url, headers });
  // The network path is the only source of a non-"default" type.
  __tbBrand(response, __tbResponseData).type = responseType;
  return response;
};
// The type a body of a given kind contributes, or null when it has none
// (<https://fetch.spec.whatwg.org/#concept-bodyinit-extract>).
const __tbBodyType = body => {
  if (body instanceof Blob) return body.type === '' ? null : body.type;
  if (body instanceof URLSearchParams) return 'application/x-www-form-urlencoded;charset=UTF-8';
  if (body instanceof ArrayBuffer || ArrayBuffer.isView(body) || body instanceof ReadableStream) return null;
  return 'text/plain;charset=UTF-8';
};
// <https://fetch.spec.whatwg.org/#bodyinit-safely-extract>
const __tbPageRequestBody = async body => {
  if (body === null || body === undefined) return { bytes: [], type: null };
  const type = __tbBodyType(body);
  if (body instanceof Blob) return { bytes: Array.from(new Uint8Array(await body.arrayBuffer())), type };
  if (body instanceof ArrayBuffer) return { bytes: Array.from(new Uint8Array(body)), type };
  if (ArrayBuffer.isView(body)) return { bytes: Array.from(new Uint8Array(body.buffer, body.byteOffset, body.byteLength)), type };
  return { bytes: Array.from(__tbUtf8Encode(String(body))), type };
};
const __tbQueuePageRequest = (url, method, body, headers, cors, callback) => {
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
  host.__tb_fetchCbs[id] = function(status, bytes, finalUrl, contentType, responseHeaders, responseType) {
    delete host.__tb_fetchCbs[id];
    callback(status, bytes, finalUrl, contentType, responseHeaders, responseType);
  };
  __tbPageRequestBody(body).then(payload => {
    if (!host.__tb_fetchCbs[id]) return;
    const type = __tbApply(__tbHeadersGet, headers, ['content-type']) === null ? payload.type : null;
    const fields = __tbArray.map(__tbBrand(headers, __tbHeadersData), entry => [entry[0], entry[1]]);
    host.__queueFetch(url, id, method, payload.bytes, type, fields, [referrer === null || referrer === undefined ? '' : String(referrer), cors.mode, cors.credentials, cors.redirect]);
  }, () => {
    const done = host.__tb_fetchCbs[id];
    if (done) done(0, new Uint8Array(), '', '', [], 'error');
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
  let mode = 'cors';
  let credentials = 'same-origin';
  let redirect = 'follow';
  if (input instanceof globalThis.Request) {
    const source = __tbBrand(input, __tbRequestData);
    url = source.url;
    method = source.method;
    blob = source.blob;
    headers = new globalThis.Headers(source.headers);
    body = source.body;
    mode = source.mode;
    credentials = source.credentials;
    redirect = source.redirect;
  } else {
    const resolved = host.__tbResolveUrl(String(input), undefined);
    url = resolved === null ? String(input) : resolved;
  }
  try {
    if (options.method !== undefined) method = __tbRequestMethod(options.method);
    if (options.headers !== undefined) headers = new globalThis.Headers(options.headers);
    if (options.body !== undefined) body = options.body;
    if (options.mode !== undefined) mode = __tbRequestMode(options.mode);
    if (options.credentials !== undefined) credentials = __tbCredentialsMode(options.credentials);
    if (options.redirect !== undefined) redirect = __tbRedirectMode(options.redirect);
  } catch (error) {
    return Promise.reject(error);
  }
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
      resolve(__tbMakeResponse(entry.contents, 200, url, entry.type, [], 'basic'));
      return;
    }
    __tbQueuePageRequest(url, method, body, headers, { mode, credentials, redirect }, (status, bytes, finalUrl, type, fields, responseType) => {
      if (responseType !== 'error') resolve(__tbMakeResponse(bytes, status, finalUrl, type, fields, responseType));
      else reject(new TypeError('Failed to fetch'));
    });
  });
};
// https://w3c.github.io/FileAPI/#blob
