// https://xhr.spec.whatwg.org/#the-xmlhttprequest-interface
const __tbXhrData = host.slots('tinybrowser.xhr');
const __tbXhrBlobConstructor = globalThis.Blob;
const __tbXhrCancel = data => {
  data.generation++;
  __tbCancelPageRequest(data.request);
  data.request = null;
  if (data.timer !== null) clearTimeout(data.timer);
  data.timer = null;
};
// https://xhr.spec.whatwg.org/#request-error-steps
const __tbXhrError = (xhr, data, type) => {
  __tbXhrCancel(data);
  data.sent = false;
  data.state = 4;
  data.status = 0;
  data.responseURL = '';
  data.responseHeaders = new __tbHeadersConstructor();
  data.responseBytes = host.slots.bytes(0);
  xhr.dispatchEvent(new Event('readystatechange'));
  xhr.dispatchEvent(new ProgressEvent(type));
  xhr.dispatchEvent(new ProgressEvent('loadend'));
};
// https://xhr.spec.whatwg.org/#the-timeout-attribute
const __tbXhrTimeout = (xhr, data) => {
  if (data.timer !== null) clearTimeout(data.timer);
  data.timer = null;
  if (!data.sent || data.timeout === 0) return;
  const generation = data.generation;
  data.timer = host.__tb_timeouts.length;
  host.__tb_timeouts[data.timer] = () => {
    if (data.sent && data.generation === generation) __tbXhrError(xhr, data, 'timeout');
  };
  host.__scheduleTimeout(data.timer, Math.max(0, data.started + data.timeout - Date.now()));
};
globalThis.XMLHttpRequestUpload = class XMLHttpRequestUpload extends EventTarget {};
globalThis.XMLHttpRequest = class XMLHttpRequest extends EventTarget {
  constructor() {
    super();
    __tbXhrData.set(this, {
        __proto__: null,
        state: 0, sent: false, method: '', url: '', headers: new __tbHeadersConstructor(),
        responseHeaders: new __tbHeadersConstructor(), responseURL: '', status: 0, responseBytes: host.slots.bytes(0),
        responseType: '', timeout: 0, withCredentials: false, generation: 0,
        request: null, timer: null, started: 0,
        upload: new XMLHttpRequestUpload(),
    });
  }
  get readyState() { return __tbBrand(this, __tbXhrData).state; }
  get status() { return __tbBrand(this, __tbXhrData).status; }
  get statusText() { return ''; }
  get responseURL() { return __tbBrand(this, __tbXhrData).responseURL; }
  get upload() { return __tbBrand(this, __tbXhrData).upload; }
  get timeout() { return __tbBrand(this, __tbXhrData).timeout; }
  set timeout(value) {
    const data = __tbBrand(this, __tbXhrData);
    data.timeout = Number(value) >>> 0;
    __tbXhrTimeout(this, data);
  }
  get withCredentials() { return __tbBrand(this, __tbXhrData).withCredentials; }
  set withCredentials(value) {
    const data = __tbBrand(this, __tbXhrData);
    if (data.state > 1 || data.sent) throw new DOMException('Request already sent', 'InvalidStateError');
    data.withCredentials = Boolean(value);
  }
  get responseType() { return __tbBrand(this, __tbXhrData).responseType; }
  set responseType(value) {
    const data = __tbBrand(this, __tbXhrData);
    if (data.state >= 3) throw new DOMException('Response loading', 'InvalidStateError');
    if (['', 'text', 'json', 'arraybuffer', 'blob', 'document'].includes(String(value))) data.responseType = String(value);
  }
  get responseText() {
    const data = __tbBrand(this, __tbXhrData);
    if (data.responseType !== '' && data.responseType !== 'text') throw new DOMException('Not a text response', 'InvalidStateError');
    return data.state < 3 ? '' : __tbDecodeBytes(data.responseBytes, 'utf-8', false, false);
  }
  get response() {
    // https://xhr.spec.whatwg.org/#dom-xmlhttprequest-response
    const data = __tbBrand(this, __tbXhrData);
    if (data.responseType === '' || data.responseType === 'text') return this.responseText;
    if (data.state !== 4) return null;
    if (data.responseType === 'arraybuffer') return __tbBytesCopy(data.responseBytes).buffer;
    if (data.responseType === 'blob') return new __tbXhrBlobConstructor([__tbBytesCopy(data.responseBytes)], { type: __tbApply(__tbHeadersGet, data.responseHeaders, ['content-type']) || '' });
    if (data.responseType === 'json') {
      try { return JSON.parse(__tbDecodeBytes(data.responseBytes, 'utf-8', false, false)); }
      catch (_) { return null; }
    }
    return null;
  }
  get responseXML() { return null; }
  // https://xhr.spec.whatwg.org/#the-open()-method
  open(method, url, async = true) {
    const data = __tbBrand(this, __tbXhrData);
    const verb = String(method);
    if (!/^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/.test(verb)) throw new DOMException('Invalid HTTP method', 'SyntaxError');
    if (/^(CONNECT|TRACE|TRACK)$/i.test(verb)) throw new DOMException('Forbidden HTTP method', 'SecurityError');
    const parsed = host.__tbResolveUrl(String(url), undefined);
    if (parsed === null) throw new DOMException('Invalid URL', 'SyntaxError');
    if (!async) throw new DOMException('Synchronous requests are not supported', 'InvalidAccessError');
    __tbXhrCancel(data);
    data.method = /^(GET|HEAD|POST|PUT|DELETE|OPTIONS)$/i.test(verb) ? verb.toUpperCase() : verb;
    data.url = parsed;
    data.sent = false;
    data.headers = new __tbHeadersConstructor();
    data.responseHeaders = new __tbHeadersConstructor();
    data.responseURL = '';
    data.responseBytes = host.slots.bytes(0);
    data.status = 0;
    if (data.state !== 1) {
      data.state = 1;
      this.dispatchEvent(new Event('readystatechange'));
    }
  }
  // https://xhr.spec.whatwg.org/#the-setrequestheader()-method
  setRequestHeader(name, value) {
    const data = __tbBrand(this, __tbXhrData);
    if (data.state !== 1 || data.sent) throw new DOMException('Request not open', 'InvalidStateError');
    __tbApply(__tbHeadersAppend, data.headers, [name, value]);
  }
  // https://xhr.spec.whatwg.org/#the-send()-method
  send(body = null) {
    const data = __tbBrand(this, __tbXhrData);
    if (data.state !== 1 || data.sent) throw new DOMException('Request not open', 'InvalidStateError');
    if (data.method === 'GET' || data.method === 'HEAD') body = null;
    data.sent = true;
    data.started = Date.now();
    const generation = data.generation;
    this.dispatchEvent(new ProgressEvent('loadstart'));
    if (data.state !== 1 || !data.sent || data.generation !== generation) return;
    data.request = __tbQueuePageRequest(data.url, data.method, body, data.headers, (status, bytes, url, type, fields) => {
      if (data.generation !== generation || !data.sent) return;
      data.request = null;
      if (status === 0) {
        __tbXhrError(this, data, 'error');
        return;
      }
      data.status = status;
      data.responseURL = url;
      data.responseHeaders = new __tbHeadersConstructor(fields);
      if (type && __tbApply(__tbHeadersGet, data.responseHeaders, ['content-type']) === null) __tbApply(__tbHeadersAppend, data.responseHeaders, ['content-type', type]);
      data.state = 2;
      this.dispatchEvent(new Event('readystatechange'));
      if (data.generation !== generation || data.state !== 2 || !data.sent) return;
      data.responseBytes = host.slots.bytes(bytes);
      if (bytes.length) {
        data.state = 3;
        this.dispatchEvent(new Event('readystatechange'));
        if (data.generation !== generation || data.state !== 3 || !data.sent) return;
        this.dispatchEvent(new ProgressEvent('progress', { loaded: bytes.length }));
        if (data.generation !== generation || data.state !== 3 || !data.sent) return;
      }
      // https://xhr.spec.whatwg.org/#handle-response-end-of-body
      if (data.timer !== null) clearTimeout(data.timer);
      data.timer = null;
      data.state = 4;
      data.sent = false;
      this.dispatchEvent(new Event('readystatechange'));
      if (data.generation !== generation || data.state !== 4) return;
      this.dispatchEvent(new ProgressEvent('load', { loaded: bytes.length }));
      this.dispatchEvent(new ProgressEvent('loadend', { loaded: bytes.length }));
    });
    __tbXhrTimeout(this, data);
  }
  // https://xhr.spec.whatwg.org/#the-abort()-method
  abort() {
    const data = __tbBrand(this, __tbXhrData);
    if (data.sent || data.state === 2 || data.state === 3) {
      __tbXhrError(this, data, 'abort');
    } else {
      __tbXhrCancel(data);
      data.status = 0;
      data.responseURL = '';
      data.responseHeaders = new __tbHeadersConstructor();
      data.responseBytes = host.slots.bytes(0);
    }
    if (data.state === 4) data.state = 0;
  }
  getResponseHeader(name) {
    const data = __tbBrand(this, __tbXhrData);
    if (data.state < 2 || /^set-cookie2?$/i.test(String(name))) return null;
    return __tbApply(__tbHeadersGet, data.responseHeaders, [name]);
  }
  getAllResponseHeaders() {
    const data = __tbBrand(this, __tbXhrData);
    if (data.state < 2) return '';
    return __tbArray.map(__tbBrand(data.responseHeaders, __tbHeadersData), entry => [entry[0], entry[1]]).filter(([name]) => !/^set-cookie2?$/i.test(name))
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([name, value]) => name + ': ' + value + '\r\n').join('');
  }
  overrideMimeType(value) {
    const data = __tbBrand(this, __tbXhrData);
    if (data.state >= 3) throw new DOMException('Response loading', 'InvalidStateError');
    data.overrideMimeType = String(value);
  }
};
for (const [name, value] of [['UNSENT', 0], ['OPENED', 1], ['HEADERS_RECEIVED', 2], ['LOADING', 3], ['DONE', 4]]) {
  for (const target of [globalThis.XMLHttpRequest, globalThis.XMLHttpRequest.prototype]) {
    Object.defineProperty(target, name, { value, writable: false, enumerable: true, configurable: false });
  }
}
for (const name of ['onreadystatechange', 'onloadstart', 'onprogress', 'onabort', 'onerror', 'onload', 'ontimeout', 'onloadend']) {
  Object.defineProperty(globalThis.XMLHttpRequest.prototype, name, { value: null, writable: true, enumerable: true, configurable: true });
  Object.defineProperty(globalThis.XMLHttpRequestUpload.prototype, name, { value: null, writable: true, enumerable: true, configurable: true });
}
Object.defineProperty(globalThis.XMLHttpRequest.prototype, Symbol.toStringTag, { value: 'XMLHttpRequest', configurable: true });
Object.defineProperty(globalThis.XMLHttpRequestUpload.prototype, Symbol.toStringTag, { value: 'XMLHttpRequestUpload', configurable: true });
