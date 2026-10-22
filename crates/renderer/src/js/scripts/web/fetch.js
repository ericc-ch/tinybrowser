// https://fetch.spec.whatwg.org/#headers-class
// Page requests share the browser process's HTTP client with navigation.
const __tbHeadersData = host.slots('tinybrowser.headers.data');
const __tbHeadersGuard = host.slots('tinybrowser.headers.guard');
const __tbHeadersIteratorState = host.slots('tinybrowser.headers.iterator');
const __tbResponseData = host.slots('tinybrowser.response.data');
const __tbRequestData = host.slots('tinybrowser.request.data');
let __tbKeepaliveAvailable = 65536;
// A header name is a byte sequence that is an HTTP token
// (<https://fetch.spec.whatwg.org/#concept-header-name>).
const __tbHeaderName = value => {
  const name = __tbToDOMString(value);
  for (let index = 0; index < name.length; index++) {
    if (name.charCodeAt(index) > 0xFF) throw new TypeError('Invalid header name');
  }
  if (!/^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/.test(name)) throw new TypeError('Invalid header name');
  return name.toLowerCase();
};
// A header value is a byte sequence with no 0x00, 0x0A, or 0x0D, and a JS
// string holds it with one code point per byte
// (<https://fetch.spec.whatwg.org/#concept-header-value>).
const __tbHeaderValue = value => {
  value = __tbToDOMString(value).replace(/^[\t\n\r ]+|[\t\n\r ]+$/g, '');
  for (let index = 0; index < value.length; index++) {
    const code = value.charCodeAt(index);
    if (code === 0x00 || code === 0x0A || code === 0x0D || code > 0xFF) throw new TypeError('Invalid header value');
  }
  return value;
};
// <https://fetch.spec.whatwg.org/#forbidden-request-header>
const __tbForbiddenRequestHeader = name =>
  name === 'accept-charset' || name === 'accept-encoding' ||
  name === 'access-control-request-headers' || name === 'access-control-request-method' ||
  name === 'connection' || name === 'content-length' || name === 'cookie' ||
  name === 'cookie2' || name === 'date' || name === 'dnt' || name === 'expect' ||
  name === 'host' || name === 'keep-alive' || name === 'origin' || name === 'referer' ||
  name === 'set-cookie' || name === 'te' || name === 'trailer' ||
  name === 'transfer-encoding' || name === 'upgrade' || name === 'via' ||
  name.indexOf('proxy-') === 0 || name.indexOf('sec-') === 0;
// <https://fetch.spec.whatwg.org/#forbidden-method>
const __tbForbiddenMethod = value => /^(CONNECT|TRACE|TRACK)$/i.test(value.trim());
const __tbForbiddenMethodHeader = name =>
  name === 'x-http-method-override' || name === 'x-http-method' || name === 'x-method-override';
// <https://fetch.spec.whatwg.org/#cors-safelisted-request-header>
const __tbCorsSafelistedHeader = (name, value) => {
  if (name !== 'accept' && name !== 'accept-language' && name !== 'content-language' && name !== 'content-type') return false;
  if (value.length > 128) return false;
  for (let index = 0; index < value.length; index++) {
    const code = value.charCodeAt(index);
    if ((code >= 0x00 && code <= 0x08) || (code >= 0x0A && code <= 0x1F) || code === 0x7F ||
        code === 0x22 || code === 0x28 || code === 0x29 || code === 0x3A || code === 0x3C ||
        code === 0x3E || code === 0x3F || code === 0x40 || code === 0x5B || code === 0x5C ||
        code === 0x5D || code === 0x7B || code === 0x7D) return false;
  }
  if (name === 'content-type') {
    const essence = __tbAsciiLower(__tbApply(__tbStringTrim, __tbApply(__tbStringSplit, value, [';'])[0], []));
    if (essence !== 'application/x-www-form-urlencoded' && essence !== 'multipart/form-data' && essence !== 'text/plain') return false;
  }
  return true;
};
const __tbHeadersList = headers => __tbBrand(headers, __tbHeadersData);
const __tbHeadersGuardOf = headers => {
  const guard = __tbHeadersGuard.get(headers);
  return guard === undefined ? 'none' : guard;
};
// Sorted-and-combined view the iterator and `forEach` walk: names sort
// alphabetically, duplicate values combine with ", ", and every set-cookie
// entry stays separate in insertion order
// (<https://fetch.spec.whatwg.org/#concept-header-list-sort-and-combine>).
const __tbSortedHeaders = headers => {
  const entries = __tbHeadersList(headers);
  const list = [];
  const indexByName = Object.create(null);
  for (let index = 0; index < entries.length; index++) {
    const name = entries[index][0];
    const value = entries[index][1];
    if (name === 'set-cookie') {
      __tbArray.push(list, [name, value]);
      continue;
    }
    const at = indexByName[name];
    if (at === undefined) {
      indexByName[name] = list.length;
      __tbArray.push(list, [name, value]);
    } else {
      list[at][1] = list[at][1] + ', ' + value;
    }
  }
  __tbArray.sort(list, (a, b) => a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0);
  return list;
};
const __tbHeadersIteratorProto = Object.create(Object.getPrototypeOf(Object.getPrototypeOf([][Symbol.iterator]())));
Object.defineProperty(__tbHeadersIteratorProto, Symbol.iterator, {
  value: function() { return this; },
  writable: true, enumerable: false, configurable: true,
});
Object.defineProperty(__tbHeadersIteratorProto, 'next', {
  value: function() {
    const state = __tbHeadersIteratorState.get(this);
    if (state === undefined) throw new TypeError('Illegal invocation');
    const list = __tbSortedHeaders(state.headers);
    if (state.index >= list.length) return { value: undefined, done: true };
    const entry = list[state.index++];
    return { value: state.kind === 'entries' ? [entry[0], entry[1]] : state.kind === 'keys' ? entry[0] : entry[1], done: false };
  },
  writable: true, enumerable: true, configurable: true,
});
const __tbHeadersIterator = (headers, kind) => {
  const iterator = Object.create(__tbHeadersIteratorProto);
  __tbHeadersIteratorState.set(iterator, { headers, kind, index: 0 });
  return iterator;
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
// A `no-cors` request may use only a CORS-safelisted method
// (<https://fetch.spec.whatwg.org/#cors-safelisted-method>).
const __tbCorsSafelistedMethod = method =>
  method === 'GET' || method === 'HEAD' || method === 'POST';
const __tbBrand = (value, slots, message) => {
  const data = slots.get(value);
  if (data === undefined) throw new TypeError(message === undefined ? 'Illegal invocation' : message);
  return data;
};
globalThis.Headers = class Headers {
  constructor(init) {
    const entries = [];
    __tbHeadersData.set(this, entries);
    __tbHeadersGuard.set(this, 'none');
    if (init === undefined) return;
    if (init === null || (typeof init !== 'object' && typeof init !== 'function')) {
      throw new TypeError('The Headers init must be a sequence or a record');
    }
    const iteratorMethod = init[Symbol.iterator];
    if (iteratorMethod !== undefined && iteratorMethod !== null) {
      if (typeof iteratorMethod !== 'function') throw new TypeError('The Headers init must be iterable');
      const iterator = __tbApply(iteratorMethod, init, []);
      while (true) {
        const step = iterator.next();
        if (step.done) break;
        const values = __tbArrayFrom(step.value);
        if (values.length !== 2) throw new TypeError('Header pairs must contain two values');
        __tbArray.push(entries, [__tbHeaderName(values[0]), __tbHeaderValue(values[1])]);
      }
    } else {
      for (const name of __tbObjectKeys(init)) {
        __tbArray.push(entries, [__tbHeaderName(name), __tbHeaderValue(init[name])]);
      }
    }
  }
  append(name, value) {
    const guard = __tbHeadersGuardOf(this);
    if (guard === 'immutable') throw new TypeError('Headers are immutable');
    const header = [__tbHeaderName(name), __tbHeaderValue(value)];
    if ((guard === 'request' || guard === 'request-no-cors') && __tbForbiddenRequestHeader(header[0])) return;
    if (guard === 'response' && header[0] === 'set-cookie') return;
    if (guard === 'request' && __tbForbiddenMethodHeader(header[0]) && __tbForbiddenMethod(header[1])) return;
    if (guard === 'request-no-cors') {
      const existing = this.get(header[0]);
      if (!__tbCorsSafelistedHeader(header[0], existing === null ? header[1] : existing + ', ' + header[1])) return;
    }
    __tbArray.push(__tbHeadersList(this), header);
  }
  delete(name) {
    if (__tbHeadersGuardOf(this) === 'immutable') throw new TypeError('Headers are immutable');
    const entries = __tbHeadersList(this);
    name = __tbHeaderName(name);
    for (let index = entries.length - 1; index >= 0; index--) {
      if (entries[index][0] === name) __tbArray.remove(entries, index);
    }
  }
  get(name) {
    const entries = __tbHeadersList(this);
    name = __tbHeaderName(name);
    // Duplicate values combine with ', '
    // (<https://fetch.spec.whatwg.org/#dom-headers-get>).
    const values = [];
    for (let index = 0; index < entries.length; index++) {
      if (entries[index][0] === name) values[values.length] = entries[index][1];
    }
    return values.length === 0 ? null : __tbArray.join(values, ', ');
  }
  getSetCookie() {
    const entries = __tbHeadersList(this);
    const values = [];
    for (let index = 0; index < entries.length; index++) {
      if (entries[index][0] === 'set-cookie') __tbArray.push(values, entries[index][1]);
    }
    return values;
  }
  has(name) { return this.get(name) !== null; }
  set(name, value) {
    if (__tbHeadersGuardOf(this) === 'immutable') throw new TypeError('Headers are immutable');
    const header = [__tbHeaderName(name), __tbHeaderValue(value)];
    this.delete(header[0]);
    const guard = __tbHeadersGuardOf(this);
    if ((guard === 'request' || guard === 'request-no-cors') && __tbForbiddenRequestHeader(header[0])) return;
    if (guard === 'response' && header[0] === 'set-cookie') return;
    if (guard === 'request' && __tbForbiddenMethodHeader(header[0]) && __tbForbiddenMethod(header[1])) return;
    if (guard === 'request-no-cors') {
      const existing = this.get(header[0]);
      if (!__tbCorsSafelistedHeader(header[0], existing === null ? header[1] : existing + ', ' + header[1])) return;
    }
    __tbArray.push(__tbHeadersList(this), header);
  }
  forEach(callback, thisArg) {
    if (typeof callback !== 'function') throw new TypeError('callback is not a function');
    const list = __tbSortedHeaders(this);
    for (let index = 0; index < list.length; index++) {
      __tbApply(callback, thisArg, [list[index][1], list[index][0], this]);
    }
  }
  entries() { return __tbHeadersIterator(this, 'entries'); }
  keys() { return __tbHeadersIterator(this, 'keys'); }
  values() { return __tbHeadersIterator(this, 'values'); }
  [Symbol.iterator]() { return this.entries(); }
};
Object.defineProperty(globalThis.Headers.prototype, Symbol.toStringTag, { value: 'Headers', writable: false, enumerable: false, configurable: true });
const __tbHeadersConstructor = globalThis.Headers;
const __tbHeadersGet = __tbHeadersConstructor.prototype.get;
const __tbHeadersAppend = __tbHeadersConstructor.prototype.append;
// Internal construction with a guard; page code only sees the default guard
// (<https://fetch.spec.whatwg.org/#concept-headers-guard>).
const __tbMakeHeaders = (entries, guard) => {
  const headers = new __tbHeadersConstructor();
  __tbHeadersData.set(headers, entries);
  __tbHeadersGuard.set(headers, guard);
  return headers;
};
// A BodyInit becomes bytes plus a type, or a ReadableStream when the caller
// supplied one (<https://fetch.spec.whatwg.org/#bodyinit>). The bytes stay in
// memory: every consumer in this engine reads the whole body anyway.
const __tbConcatBytes = chunks => {
  let length = 0;
  for (let index = 0; index < chunks.length; index++) length += chunks[index].length;
  const bytes = new __tbUint8Array(length);
  let offset = 0;
  for (let index = 0; index < chunks.length; index++) {
    const chunk = chunks[index];
    for (let byte = 0; byte < chunk.length; byte++) bytes[offset + byte] = chunk[byte];
    offset += chunk.length;
  }
  return bytes;
};
const __tbMultipartEscape = value =>
  String(value).replace(/\r/g, '%0D').replace(/\n/g, '%0A').replace(/"/g, '%22');
let __tbMultipartSequence = 0;
const __tbMultipartBoundary = () => {
  __tbMultipartSequence += 1;
  return '----tinybrowser' + __tbMultipartSequence + Math.random().toString(16).slice(2);
};
// <https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#multipart-form-data>
const __tbFormDataBody = formData => {
  const boundary = __tbMultipartBoundary();
  const type = 'multipart/form-data; boundary=' + boundary;
  const chunks = [];
  let count = 0;
  for (const entry of formData) {
    count += 1;
    const name = entry[0];
    const value = entry[1];
    let head = '--' + boundary + '\r\nContent-Disposition: form-data; name="' + __tbMultipartEscape(name) + '"';
    let fileBytes = null;
    if (value instanceof Blob) {
      head += '; filename="' + __tbMultipartEscape(value.name === undefined ? 'blob' : value.name) + '"\r\n';
      head += 'Content-Type: ' + (value.type === '' ? 'application/octet-stream' : value.type) + '\r\n';
      fileBytes = __tbBytesCopy(__tbBlobData.get(value).bytes);
    } else {
      head += '\r\n';
    }
    head += '\r\n';
    __tbArray.push(chunks, __tbUtf8Encode(head));
    __tbArray.push(chunks, fileBytes === null ? __tbUtf8Encode(String(value)) : fileBytes);
    __tbArray.push(chunks, __tbUtf8Encode('\r\n'));
  }
  if (count === 0) return { bytes: new __tbUint8Array(0), type };
  __tbArray.push(chunks, __tbUtf8Encode('--' + boundary + '--\r\n'));
  return { bytes: __tbConcatBytes(chunks), type };
};
const __tbExtractBody = body => {
  if (body === null || body === undefined) return { bytes: null, type: null, stream: null };
  if (body instanceof ReadableStream) {
    if (body.locked || __tbStreamDisturbed(body)) throw new TypeError('Stream is locked or disturbed');
    return { bytes: null, type: null, stream: body };
  }
  if (body instanceof Blob) {
    return { bytes: __tbBytesCopy(__tbBlobData.get(body).bytes), type: body.type === '' ? null : body.type, stream: null };
  }
  if (body instanceof URLSearchParams) {
    return { bytes: __tbUtf8Encode(String(body)), type: 'application/x-www-form-urlencoded;charset=UTF-8', stream: null };
  }
  if (body instanceof FormData) {
    const encoded = __tbFormDataBody(body);
    return { bytes: encoded.bytes, type: encoded.type, stream: null };
  }
  if (body instanceof ArrayBuffer) return { bytes: __tbBytesCopy(new __tbUint8Array(body)), type: null, stream: null };
  if (ArrayBuffer.isView(body)) {
    return { bytes: __tbBytesCopy(new __tbUint8Array(body.buffer, body.byteOffset, body.byteLength)), type: null, stream: null };
  }
  return { bytes: __tbUtf8Encode(__tbToDOMString(body)), type: 'text/plain;charset=UTF-8', stream: null };
};
const __tbBlobTypeFromHeader = header => {
  if (header === null) return '';
  let type = host.__tbExtractMimeType(header);
  // Chrome keeps the space in the multipart boundary parameter; WPT expects
  // blob.type to match the response Content-Type (<fetch/api/response/response-consume.html>).
  if (type.indexOf('multipart/form-data;') === 0 && type.indexOf('multipart/form-data; ') !== 0) {
    type = 'multipart/form-data; ' + type.slice('multipart/form-data;'.length);
  }
  return type.replace(/;charset=([^;]+)/i, (match, value) => ';charset=' + value.toLowerCase());
};
const __tbBodyStream = extracted => {
  if (extracted.stream !== null) return extracted.stream;
  if (extracted.bytes === null) return null;
  return new ReadableStream({
    start(controller) {
      controller.enqueue(extracted.bytes);
      controller.close();
    },
  });
};
const __tbBodyUsed = data =>
  data.bodyUsed || (data.stream !== null && __tbStreamDisturbed(data.stream));
const __tbConsumeBody = data => {
  if (data.stream === null && data.bodyError !== true) return Promise.resolve(new __tbUint8Array(0));
  if (__tbBodyUsed(data)) return Promise.reject(new TypeError('Body already used'));
  data.bodyUsed = true;
  if (data.bodyError === true) return Promise.reject(new TypeError('Body decoding failed'));
  return __tbResponseBytes(data.stream);
};
// A Content-Type split into its essence and boundary. A failed parse yields
// null, which every caller treats as "not a form body".
const __tbContentType = headers => {
  const value = __tbApply(__tbHeadersGet, headers, ['content-type']);
  if (value === null) return null;
  const parts = __tbApply(__tbStringSplit, value, [';']);
  const essence = __tbAsciiLower(__tbApply(__tbStringTrim, parts[0], []));
  let boundary = null;
  for (let index = 1; index < parts.length; index++) {
    const parameter = __tbApply(__tbStringSplit, parts[index], ['=']);
    if (parameter.length !== 2) continue;
    if (__tbAsciiLower(__tbApply(__tbStringTrim, parameter[0], [])) !== 'boundary') continue;
    boundary = __tbApply(__tbStringTrim, parameter[1], []);
    if (boundary.length >= 2 && boundary[0] === '"' && boundary[boundary.length - 1] === '"') {
      boundary = __tbApply(__tbStringSlice, boundary, [1, boundary.length - 1]);
    }
  }
  return { essence, boundary };
};
const __tbBytesIndexOf = (bytes, needle, from) => {
  for (let index = from; index + needle.length <= bytes.length; index++) {
    let match = true;
    for (let offset = 0; offset < needle.length; offset++) {
      if (bytes[index + offset] !== needle[offset]) { match = false; break; }
    }
    if (match) return index;
  }
  return -1;
};
const __tbAsciiBytes = bytes => {
  let text = '';
  for (let index = 0; index < bytes.length; index++) text += String.fromCharCode(bytes[index]);
  return text;
};
const __tbDispositionParam = (head, key) => {
  const quoted = new RegExp(key + '="([^"]*)"', 'i').exec(head);
  if (quoted !== null) return quoted[1];
  const bare = new RegExp(key + '=([^;\\r\\n]*)', 'i').exec(head);
  return bare === null ? null : __tbApply(__tbStringTrim, bare[1], []);
};
const __tbParseMultipart = (bytes, boundary) => {
  const formData = new FormData();
  const delimiter = __tbUtf8Encode('--' + boundary);
  const separator = __tbUtf8Encode('\r\n\r\n');
  let position = __tbBytesIndexOf(bytes, delimiter, 0);
  if (position === -1) return formData;
  while (position !== -1) {
    position += delimiter.length;
    if (position + 1 < bytes.length && bytes[position] === 0x2D && bytes[position + 1] === 0x2D) break;
    if (position + 1 < bytes.length && bytes[position] === 0x0D && bytes[position + 1] === 0x0A) position += 2;
    const headerEnd = __tbBytesIndexOf(bytes, separator, position);
    if (headerEnd === -1) break;
    const head = __tbAsciiBytes(bytes.subarray(position, headerEnd));
    const valueStart = headerEnd + separator.length;
    const next = __tbBytesIndexOf(bytes, delimiter, valueStart);
    if (next === -1) break;
    let valueEnd = next;
    if (valueEnd >= 2 && bytes[valueEnd - 2] === 0x0D && bytes[valueEnd - 1] === 0x0A) valueEnd -= 2;
    const value = bytes.subarray(valueStart, valueEnd);
    const name = __tbDispositionParam(head, 'name');
    if (name !== null) {
      const filename = __tbDispositionParam(head, 'filename');
      if (filename !== null) {
        const typeMatch = /content-type:\s*([^\r\n]+)/i.exec(head);
        const type = typeMatch === null ? 'text/plain' : __tbApply(__tbStringTrim, typeMatch[1], []);
        formData.append(name, new File([value], filename, { type }));
      } else {
        formData.append(name, __tbDecodeBytes(value, 'utf-8', false, true));
      }
    }
    position = next;
  }
  return formData;
};
const __tbFormDataFromBytes = (bytes, contentType) => {
  if (contentType === null) throw new TypeError('Body is not a form');
  if (contentType.essence === 'multipart/form-data') {
    if (contentType.boundary === null) throw new TypeError('Missing multipart boundary');
    return __tbParseMultipart(bytes, contentType.boundary);
  }
  if (contentType.essence === 'application/x-www-form-urlencoded') {
    const formData = new FormData();
    const parameters = new URLSearchParams(__tbDecodeBytes(bytes, 'utf-8', false, false));
    for (const entry of parameters) formData.append(entry[0], entry[1]);
    return formData;
  }
  throw new TypeError('Body is not a form');
};
// https://fetch.spec.whatwg.org/#response-class
globalThis.Response = class Response {
  constructor(body, init) {
    const options = init === undefined ? {} : Object(init);
    const status = options.status === undefined ? 200 : Number(options.status);
    if (!Number.isInteger(status) || status < 200 || status > 599) throw new RangeError('Invalid response status');
    const storedBody = body === undefined || body === null ? null : body;
    // A null body status has no body (<https://fetch.spec.whatwg.org/#null-body-status>).
    if (storedBody !== null && (status === 101 || status === 204 || status === 205 || status === 304)) {
      throw new TypeError('A null body status cannot have a body');
    }
    const headers = new __tbHeadersConstructor(options.headers);
    __tbHeadersGuard.set(headers, 'response');
    const statusText = options.statusText === undefined ? '' : String(options.statusText);
    for (let index = 0; index < statusText.length; index++) {
      const code = statusText.charCodeAt(index);
      if (code > 0xFF || code === 0x0A || code === 0x0D) throw new TypeError('Invalid status text');
    }
    const extracted = __tbExtractBody(storedBody);
    // The body's type becomes the Content-Type when the header list holds
    // none (<https://fetch.spec.whatwg.org/#dom-response>).
    if (extracted.type !== null && __tbApply(__tbHeadersGet, headers, ['content-type']) === null) {
      __tbApply(__tbHeadersAppend, headers, ['content-type', extracted.type]);
    }
    __tbResponseData.set(this, {
        stream: __tbBodyStream(extracted),
        bodyUsed: false,
        status,
        statusText: statusText,
        url: options.url === undefined ? '' : String(options.url),
        // `ResponseInit` has no `type` member; the constructor always yields
        // "default". Only the network path sets another type, through
        // `__tbMakeResponse`
        // (<https://fetch.spec.whatwg.org/#responseinit>).
        type: 'default',
        redirected: false,
        headers,
    });
  }
  get status() { return __tbBrand(this, __tbResponseData).status; }
  get statusText() { return __tbBrand(this, __tbResponseData).statusText; }
  get url() { return __tbBrand(this, __tbResponseData).url; }
  get type() { return __tbBrand(this, __tbResponseData).type; }
  get redirected() { return __tbBrand(this, __tbResponseData).redirected; }
  get headers() { return __tbBrand(this, __tbResponseData).headers; }
  get ok() { const status = __tbBrand(this, __tbResponseData).status; return status >= 200 && status <= 299; }
  get body() { return __tbBrand(this, __tbResponseData).stream; }
  get bodyUsed() { return __tbBodyUsed(__tbBrand(this, __tbResponseData)); }
  text() { return __tbConsumeBody(__tbBrand(this, __tbResponseData)).then(bytes => __tbDecodeBytes(bytes, 'utf-8', false, false)); }
  json() {
    return this.text().then(JSON.parse);
  }
  arrayBuffer() { return __tbConsumeBody(__tbBrand(this, __tbResponseData)).then(bytes => __tbBytesCopy(bytes).buffer); }
  bytes() { return __tbConsumeBody(__tbBrand(this, __tbResponseData)).then(bytes => __tbBytesCopy(bytes)); }
  blob() {
    const data = __tbBrand(this, __tbResponseData);
    const header = __tbApply(__tbHeadersGet, data.headers, ['content-type']);
    // The Blob type is the MIME type extracted from the header list
    // (<https://fetch.spec.whatwg.org/#concept-header-extract-mime-type>).
    const type = __tbBlobTypeFromHeader(header);
    return __tbConsumeBody(data).then(bytes => __tbMakeBlob(bytes, type));
  }
  formData() {
    const data = __tbBrand(this, __tbResponseData);
    const contentType = __tbContentType(data.headers);
    if (data.stream === null) {
      // An absent body is an empty urlencoded form, and an error for every
      // other content type (<https://fetch.spec.whatwg.org/#dom-body-formdata>).
      if (contentType !== null && contentType.essence === 'application/x-www-form-urlencoded') {
        return Promise.resolve(new FormData());
      }
      return Promise.reject(new TypeError('Body is not a form'));
    }
    return __tbConsumeBody(data).then(bytes => __tbFormDataFromBytes(bytes, contentType));
  }
  clone() {
    const data = __tbBrand(this, __tbResponseData);
    if (__tbBodyUsed(data)) throw new TypeError('body already used');
    let stream = null;
    if (data.stream !== null) {
      if (data.stream.locked) throw new TypeError('body is locked');
      const branches = data.stream.tee();
      data.stream = branches[0];
      stream = branches[1];
    }
    const copy = Object.create(globalThis.Response.prototype);
    const headers = new __tbHeadersConstructor(data.headers);
    __tbHeadersGuard.set(headers, __tbHeadersGuardOf(data.headers));
    __tbResponseData.set(copy, { ...data, stream, bodyUsed: false, headers });
    return copy;
  }
  // <https://fetch.spec.whatwg.org/#dom-response-json>
  static json(data, init) {
    const options = init === undefined ? {} : Object(init);
    const body = JSON.stringify(data);
    if (body === undefined) throw new TypeError('The data is not JSON serializable');
    const headers = new __tbHeadersConstructor(options.headers);
    if (__tbApply(__tbHeadersGet, headers, ['content-type']) === null) {
      __tbApply(__tbHeadersAppend, headers, ['content-type', 'application/json']);
    }
    return new globalThis.Response(body, { status: options.status, statusText: options.statusText, headers });
  }
  // <https://fetch.spec.whatwg.org/#dom-response-redirect>
  static redirect(url, status) {
    const code = status === undefined ? 302 : Number(status);
    if (code !== 301 && code !== 302 && code !== 303 && code !== 307 && code !== 308) {
      throw new RangeError('Invalid redirect status');
    }
    let resolved;
    try {
      const base = host.__tbResolveUrl('', undefined);
      resolved = new URL(String(url), base === null ? undefined : base).href;
    } catch (error) {
      throw new TypeError('Invalid URL');
    }
    return new globalThis.Response(null, { status: code, headers: [['location', resolved]] });
  }
  // <https://fetch.spec.whatwg.org/#dom-response-error>
  static error() {
    const response = Object.create(globalThis.Response.prototype);
    __tbResponseData.set(response, {
      stream: null,
      bodyUsed: false,
      status: 0,
      statusText: '',
      url: '',
      type: 'error',
      redirected: false,
      headers: __tbMakeHeaders([], 'immutable'),
    });
    return response;
  }
};
const __tbConsumeResponse = response => __tbConsumeBody(__tbBrand(response, __tbResponseData));
const __tbResponseBytes = async body => {
  if (body === null) return new Uint8Array(0);
  if (body instanceof ReadableStream) {
    const chunks = [];
    let length = 0;
    const reader = body.getReader();
    while (true) {
      const step = await reader.read();
      if (step.done) break;
      let chunk;
      if (step.value instanceof Uint8Array) chunk = step.value;
      else if (ArrayBuffer.isView(step.value)) chunk = new Uint8Array(step.value.buffer, step.value.byteOffset, step.value.byteLength);
      else if (step.value instanceof ArrayBuffer) chunk = new Uint8Array(step.value);
      else throw new TypeError('A body stream chunk must be a BufferSource');
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
const __tbReferrerPolicy = value => {
  const policy = String(value);
  const allowed = ['', 'no-referrer', 'no-referrer-when-downgrade', 'origin', 'origin-when-cross-origin', 'unsafe-url', 'same-origin', 'strict-origin', 'strict-origin-when-cross-origin'];
  if (__tbArray.indexOf(allowed, policy) === -1) throw new TypeError('Invalid referrer policy');
  return policy;
};
const __tbRequestCache = value => {
  const cache = String(value);
  const allowed = ['default', 'no-store', 'reload', 'no-cache', 'force-cache', 'only-if-cached'];
  if (__tbArray.indexOf(allowed, cache) === -1) throw new TypeError('Invalid cache mode');
  return cache;
};
const __tbRequestDuplex = value => {
  const duplex = String(value);
  if (duplex !== 'half' && duplex !== 'full') throw new TypeError('Invalid duplex mode');
  return duplex;
};
globalThis.Request = class Request {
  constructor(input, init) {
    const options = init === undefined ? {} : Object(init);
    const source = input instanceof globalThis.Request ? __tbBrand(input, __tbRequestData) : null;
    const url = source === null
      ? (host.__tbResolveUrl(String(input), undefined) ?? String(input))
      : source.url;
    const method = options.method !== undefined ? __tbRequestMethod(options.method) : source === null ? 'GET' : source.method;
    const headers = new __tbHeadersConstructor(options.headers !== undefined ? options.headers : source === null ? undefined : source.headers);
    const mode = options.mode !== undefined ? __tbRequestMode(options.mode) : source === null ? 'cors' : source.mode;
    // The guard follows the mode: a no-cors request only accepts
    // CORS-safelisted headers
    // (<https://fetch.spec.whatwg.org/#concept-headers-guard>).
    __tbHeadersGuard.set(headers, mode === 'no-cors' ? 'request-no-cors' : 'request');
    const credentials = options.credentials !== undefined ? __tbCredentialsMode(options.credentials) : source === null ? 'same-origin' : source.credentials;
    const redirect = options.redirect !== undefined ? __tbRedirectMode(options.redirect) : source === null ? 'follow' : source.redirect;
    const cache = options.cache !== undefined ? __tbRequestCache(options.cache) : source === null ? 'default' : source.cache;
    const integrity = options.integrity !== undefined ? String(options.integrity) : source === null ? '' : source.integrity;
    const keepalive = options.keepalive !== undefined ? Boolean(options.keepalive) : source === null ? false : source.keepalive;
    const duplex = options.duplex !== undefined ? __tbRequestDuplex(options.duplex) : source === null ? 'half' : source.duplex;
    const referrerPolicy = options.referrerPolicy !== undefined ? __tbReferrerPolicy(options.referrerPolicy) : source === null ? '' : source.referrerPolicy;
    let referrer;
    if (options.referrer !== undefined) {
      const value = String(options.referrer);
      if (value === '' || value === 'about:client') {
        referrer = value;
      } else {
        const resolved = host.__tbResolveUrl(value, undefined);
        if (resolved === null) throw new TypeError('Invalid referrer');
        // A referrer cross-origin to the current settings object is not
        // exposed; it becomes "about:client"
        // (<https://fetch.spec.whatwg.org/#dom-request-referrer>).
        const base = host.__tbResolveUrl('', undefined);
        const baseOrigin = base === null ? null : new URL(base).origin;
        referrer = baseOrigin !== null && new URL(resolved).origin === baseOrigin ? resolved : 'about:client';
      }
    } else {
      referrer = source === null ? 'about:client' : source.referrer;
    }
    const signal = options.signal !== undefined ? options.signal : source === null ? new AbortController().signal : source.signal;
    // A body inherited from a Request is transferred: the source becomes
    // disturbed and the new request gets its own stream
    // (<https://fetch.spec.whatwg.org/#dom-request>).
    const inheritedBody = options.body === undefined && source !== null;
    if (inheritedBody && source.stream !== null && (__tbBodyUsed(source) || source.stream.locked)) {
      throw new TypeError('Body already used');
    }
    if (options.body !== undefined && options.body !== null && options.body instanceof ReadableStream && options.duplex === undefined) {
      throw new TypeError('A stream body requires duplex');
    }
    if (duplex === 'full') {
      throw new TypeError('duplex "full" is not supported');
    }
    if ((method === 'GET' || method === 'HEAD') && (inheritedBody ? source.stream !== null : options.body !== undefined && options.body !== null)) {
      throw new TypeError('GET and HEAD requests cannot have a body');
    }
    // A `no-cors` Request rejects a method that is not CORS-safelisted
    // (<https://fetch.spec.whatwg.org/#dom-request>).
    if (mode === 'no-cors' && !__tbCorsSafelistedMethod(method)) {
      throw new TypeError('Invalid request method for no-cors mode');
    }
    let stream;
    if (inheritedBody) {
      if (source.stream === null) {
        stream = null;
      } else {
        const branches = source.stream.tee();
        source.bodyUsed = true;
        stream = branches[0];
      }
    } else {
      if (source !== null && source.stream !== null && !__tbBodyUsed(source) && !source.stream.locked) {
        // Replacing the body still consumes the source request.
        source.stream.tee();
        source.bodyUsed = true;
      }
      const extracted = __tbExtractBody(options.body === undefined ? null : options.body);
      if (extracted.type !== null && __tbApply(__tbHeadersGet, headers, ['content-type']) === null) {
        __tbApply(__tbHeadersAppend, headers, ['content-type', extracted.type]);
      }
      stream = __tbBodyStream(extracted);
    }
    // A Request keeps a blob URL's data alive, so fetching it still works
    // after revokeObjectURL (<https://w3c.github.io/FileAPI/#lifeTime>).
    const blob = source !== null ? source.blob : url.indexOf('blob:') === 0 ? __tbLookupObjectUrl(url) : null;
    __tbRequestData.set(this, {
      url, method, blob, headers, stream, bodyUsed: false, mode, credentials, redirect,
      referrer, referrerPolicy, cache, integrity, keepalive, duplex, signal,
      destination: source === null ? '' : source.destination,
      isReloadNavigation: false,
      isHistoryNavigation: false,
    });
  }
  get url() { return __tbBrand(this, __tbRequestData).url; }
  get method() { return __tbBrand(this, __tbRequestData).method; }
  get headers() { return __tbBrand(this, __tbRequestData).headers; }
  get body() { return __tbBrand(this, __tbRequestData).stream; }
  get bodyUsed() { return __tbBodyUsed(__tbBrand(this, __tbRequestData)); }
  get mode() { return __tbBrand(this, __tbRequestData).mode; }
  get credentials() { return __tbBrand(this, __tbRequestData).credentials; }
  get redirect() { return __tbBrand(this, __tbRequestData).redirect; }
  get referrer() { return __tbBrand(this, __tbRequestData).referrer; }
  get referrerPolicy() { return __tbBrand(this, __tbRequestData).referrerPolicy; }
  get cache() { return __tbBrand(this, __tbRequestData).cache; }
  get integrity() { return __tbBrand(this, __tbRequestData).integrity; }
  get keepalive() { return __tbBrand(this, __tbRequestData).keepalive; }
  get duplex() { return __tbBrand(this, __tbRequestData).duplex; }
  get destination() { return __tbBrand(this, __tbRequestData).destination; }
  get isReloadNavigation() { return __tbBrand(this, __tbRequestData).isReloadNavigation; }
  get isHistoryNavigation() { return __tbBrand(this, __tbRequestData).isHistoryNavigation; }
  get signal() { return __tbBrand(this, __tbRequestData).signal; }
  text() { return __tbConsumeBody(__tbBrand(this, __tbRequestData)).then(bytes => __tbDecodeBytes(bytes, 'utf-8', false, false)); }
  json() { return this.text().then(JSON.parse); }
  arrayBuffer() { return __tbConsumeBody(__tbBrand(this, __tbRequestData)).then(bytes => __tbBytesCopy(bytes).buffer); }
  bytes() { return __tbConsumeBody(__tbBrand(this, __tbRequestData)).then(bytes => __tbBytesCopy(bytes)); }
  // https://fetch.spec.whatwg.org/#dom-body-blob
  blob() {
    const data = __tbBrand(this, __tbRequestData);
    const header = __tbApply(__tbHeadersGet, data.headers, ['content-type']);
    // The Blob type is the MIME type extracted from the header list
    // (<https://fetch.spec.whatwg.org/#concept-header-extract-mime-type>).
    const type = __tbBlobTypeFromHeader(header);
    return __tbConsumeBody(data).then(bytes => __tbMakeBlob(bytes, type));
  }
  formData() {
    const data = __tbBrand(this, __tbRequestData);
    const contentType = __tbContentType(data.headers);
    if (data.stream === null) {
      if (contentType !== null && contentType.essence === 'application/x-www-form-urlencoded') {
        return Promise.resolve(new FormData());
      }
      return Promise.reject(new TypeError('Body is not a form'));
    }
    return __tbConsumeBody(data).then(bytes => __tbFormDataFromBytes(bytes, contentType));
  }
  clone() {
    const data = __tbBrand(this, __tbRequestData);
    if (__tbBodyUsed(data)) throw new TypeError('body already used');
    let stream = null;
    if (data.stream !== null) {
      if (data.stream.locked) throw new TypeError('body is locked');
      const branches = data.stream.tee();
      data.stream = branches[0];
      stream = branches[1];
    }
    const copy = Object.create(globalThis.Request.prototype);
    const headers = new __tbHeadersConstructor(data.headers);
    __tbHeadersGuard.set(headers, __tbHeadersGuardOf(data.headers));
    __tbRequestData.set(copy, { ...data, stream, bodyUsed: false, headers });
    return copy;
  }
};
Object.defineProperty(globalThis.Request.prototype, Symbol.toStringTag, { value: 'Request', writable: false, enumerable: false, configurable: true });
const __tbMakeResponse = (body, status, url, type, fields, responseType, redirected, bodyError) => {
  const headers = __tbMakeHeaders(fields, 'response');
  if (type && __tbApply(__tbHeadersGet, headers, ['content-type']) === null) {
    __tbApply(__tbHeadersAppend, headers, ['content-type', type]);
  }
  // An opaque or network-error response carries status 0, which the public
  // constructor rejects; build it with a placeholder and set the real status
  // on the private state (<https://fetch.spec.whatwg.org/#concept-filtered-response-opaque>).
  const constructorStatus = status >= 200 && status <= 599 ? status : 200;
  const nullBody = constructorStatus === 101 || constructorStatus === 204 ||
    constructorStatus === 205 || constructorStatus === 304 ||
    responseType === 'opaque' || responseType === 'opaqueredirect';
  const response = new globalThis.Response(nullBody ? null : body, { status: constructorStatus, url, headers });
  const data = __tbBrand(response, __tbResponseData);
  data.status = status;
  // A data: or blob: response uses the "OK" reason phrase; it did not come
  // from an HTTP status line
  // (<https://fetch.spec.whatwg.org/#scheme-fetch>).
  if (status === 200 && (url.indexOf('data:') === 0 || url.indexOf('blob:') === 0)) {
    data.statusText = 'OK';
  }
  // Network response headers are immutable
  // (<https://fetch.spec.whatwg.org/#concept-response-header-list>).
  __tbHeadersGuard.set(data.headers, 'immutable');
  // The network path is the only source of a non-"default" type.
  data.type = responseType;
  data.redirected = redirected === true;
  data.bodyError = bodyError === true;
  return response;
};
// <https://fetch.spec.whatwg.org/#bodyinit-safely-extract>
const __tbPageRequestBody = async body => {
  if (body === null || body === undefined) return { bytes: [], type: null };
  if (body instanceof ReadableStream) {
    return { bytes: Array.from(await __tbResponseBytes(body)), type: null };
  }
  const extracted = __tbExtractBody(body);
  return { bytes: extracted.bytes === null ? [] : Array.from(extracted.bytes), type: extracted.type };
};
const __tbQueuePageRequest = (url, method, body, headers, cors, referrer, callback) => {
  const id = ++host.__tb_fetchSeq;
  host.__tb_fetchCbs[id] = function(status, bytes, finalUrl, contentType, responseHeaders, responseType, bodyError) {
    delete host.__tb_fetchCbs[id];
    callback(status, bytes, finalUrl, contentType, responseHeaders, responseType, bodyError);
  };
  __tbPageRequestBody(body).then(payload => {
    if (!host.__tb_fetchCbs[id]) return;
    const type = __tbApply(__tbHeadersGet, headers, ['content-type']) === null ? payload.type : null;
    const fields = __tbArray.map(__tbSortedHeaders(headers), entry => [entry[0], entry[1]]);
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
// <https://fetch.spec.whatwg.org/#concept-range-header>
const __tbParseRange = (value, length) => {
  const match = /^bytes[ \t]*=[ \t]*([0-9]*)[ \t]*-[ \t]*([0-9]*)$/.exec(value);
  if (match === null) return null;
  const startText = match[1];
  const endText = match[2];
  if (startText === '' && endText === '') return null;
  if (startText === '') {
    const suffix = Number(endText);
    if (suffix === 0) return null;
    return { start: Math.max(length - suffix, 0), end: length - 1 };
  }
  const start = Number(startText);
  if (start >= length) return null;
  let end = endText === '' ? length - 1 : Number(endText);
  if (end < start) return null;
  if (end >= length) end = length - 1;
  return { start, end };
};
const __tbBodySize = body => {
  if (body === null || body === undefined) return 0;
  if (typeof body === 'string') return __tbUtf8Encode(body).length;
  if (body instanceof Blob) return body.size;
  if (body instanceof ArrayBuffer) return body.byteLength;
  if (ArrayBuffer.isView(body)) return body.byteLength;
  if (body instanceof URLSearchParams) return __tbUtf8Encode(String(body)).length;
  if (body instanceof FormData) return __tbFormDataBody(body).bytes.length;
  return 0;
};
globalThis.fetch = function(input, init) {
  const options = init === undefined ? {} : Object(init);
  let url;
  let method = 'GET';
  let blob = null;
  let headers = new __tbHeadersConstructor();
  let body = null;
  let mode = 'cors';
  let credentials = 'same-origin';
  let redirect = 'follow';
  let referrer = null;
  let signal = null;
  let keepalive = false;
  if (input instanceof globalThis.Request) {
    const source = __tbBrand(input, __tbRequestData);
    url = source.url;
    method = source.method;
    blob = source.blob;
    headers = new __tbHeadersConstructor(source.headers);
    body = source.stream;
    mode = source.mode;
    credentials = source.credentials;
    redirect = source.redirect;
    referrer = source.referrer === 'about:client' ? host.__tbReferrer(url) : source.referrer;
    signal = source.signal;
    keepalive = source.keepalive;
    if (source.stream !== null && (__tbBodyUsed(source) || source.stream.locked)) {
      return Promise.reject(new TypeError('Body already used'));
    }
  } else {
    const resolved = host.__tbResolveUrl(String(input), undefined);
    url = resolved === null ? String(input) : resolved;
    referrer = host.__tbReferrer(url);
  }
  try {
    if (options.method !== undefined) method = __tbRequestMethod(options.method);
    if (options.headers !== undefined) headers = new __tbHeadersConstructor(options.headers);
    if (options.body !== undefined) body = options.body;
    if (options.mode !== undefined) mode = __tbRequestMode(options.mode);
    if (options.credentials !== undefined) credentials = __tbCredentialsMode(options.credentials);
    if (options.redirect !== undefined) redirect = __tbRedirectMode(options.redirect);
    if (options.signal !== undefined) signal = options.signal;
    if (options.keepalive !== undefined) keepalive = Boolean(options.keepalive);
    if (mode === 'no-cors' && !__tbCorsSafelistedMethod(method)) {
      throw new TypeError('Invalid request method for no-cors mode');
    }
    if ((method === 'GET' || method === 'HEAD') && body !== null) {
      throw new TypeError('GET and HEAD requests cannot have a body');
    }
  } catch (error) {
    return Promise.reject(error);
  }
  // A user agent supplies Accept and Accept-Language when the page did not
  // (<https://fetch.spec.whatwg.org/#http-network-or-cache-fetch>).
  if (__tbApply(__tbHeadersGet, headers, ['accept']) === null) {
    __tbApply(__tbHeadersAppend, headers, ['accept', '*/*']);
  }
  if (__tbApply(__tbHeadersGet, headers, ['accept-language']) === null) {
    __tbApply(__tbHeadersAppend, headers, ['accept-language', 'en-US']);
  }
  if (body === null && (method === 'POST' || method === 'PUT') &&
      __tbApply(__tbHeadersGet, headers, ['content-length']) === null) {
    __tbApply(__tbHeadersAppend, headers, ['content-length', '0']);
  }
  return new Promise(function(resolve, reject) {
    if (signal !== null && signal !== undefined && signal.aborted) {
      reject(signal.reason);
      return;
    }
    let requestID = null;
    let settled = false;
    let quota = 0;
    if (keepalive) {
      quota = __tbBodySize(body);
      if (quota > __tbKeepaliveAvailable) {
        reject(new TypeError('Keepalive quota exceeded'));
        return;
      }
      __tbKeepaliveAvailable -= quota;
    }
    const onAbort = () => {
      if (settled) return;
      settled = true;
      if (quota !== 0) __tbKeepaliveAvailable += quota;
      __tbCancelPageRequest(requestID);
      reject(signal.reason);
    };
    if (signal !== null && signal !== undefined) signal.addEventListener('abort', onAbort);
    const finish = (callback, value) => {
      if (settled) return;
      settled = true;
      if (quota !== 0) __tbKeepaliveAvailable += quota;
      if (signal !== null && signal !== undefined) signal.removeEventListener('abort', onAbort);
      callback(value);
    };
    if (url.indexOf('blob:') === 0) {
      if (method !== 'GET' || body !== null) {
        finish(reject, new TypeError('Failed to fetch'));
        return;
      }
      const entry = blob === null ? __tbLookupObjectUrl(url) : blob;
      if (entry === null) {
        finish(reject, new TypeError('Failed to fetch'));
        return;
      }
      const range = __tbApply(__tbHeadersGet, headers, ['range']);
      if (range === null) {
        finish(resolve, __tbMakeResponse(entry.contents, 200, url, entry.type, [], 'basic', false, false));
        return;
      }
      const contents = typeof entry.contents === 'string' ? __tbUtf8Encode(entry.contents) : entry.contents;
      const parsed = __tbParseRange(range, contents.length);
      if (parsed === null) {
        finish(reject, new TypeError('Failed to fetch'));
        return;
      }
      const fields = [
        ['content-range', 'bytes ' + parsed.start + '-' + parsed.end + '/' + contents.length],
        ['content-length', String(parsed.end - parsed.start + 1)],
        ['content-type', entry.type === null || entry.type === undefined ? '' : entry.type],
      ];
      finish(resolve, __tbMakeResponse(
        __tbBytesCopy(contents, parsed.start, parsed.end + 1),
        206, url, entry.type, fields, 'basic', false, false,
      ));
      return;
    }
    requestID = __tbQueuePageRequest(url, method, body, headers, { mode, credentials, redirect }, referrer, (status, bytes, finalUrl, type, fields, responseType, bodyError) => {
      if (responseType !== 'error') {
        // A manual redirect is an opaque-redirect filtered response
        // (<https://fetch.spec.whatwg.org/#concept-filtered-response-opaque-redirect>).
        if (redirect === 'manual' && (status === 301 || status === 302 || status === 303 || status === 307 || status === 308)) {
          finish(resolve, __tbMakeResponse(null, 0, '', '', [], 'opaqueredirect', false, false));
          return;
        }
        const response = __tbMakeResponse(bytes, status, finalUrl, type, fields, responseType, finalUrl !== url, bodyError);
        // A HEAD response has no body regardless of what the wire carried
        // (<https://fetch.spec.whatwg.org/#concept-head>).
        if (method === 'HEAD') __tbBrand(response, __tbResponseData).stream = null;
        finish(resolve, response);
      } else {
        finish(reject, new TypeError('Failed to fetch'));
      }
    });
  });
};
// https://w3c.github.io/FileAPI/#blob
