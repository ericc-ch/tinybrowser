// https://xhr.spec.whatwg.org/#the-xmlhttprequest-interface
const __tbXhrData = Symbol('tinybrowser.xhr');
globalThis.XMLHttpRequestUpload = class XMLHttpRequestUpload extends EventTarget {};
globalThis.XMLHttpRequest = class XMLHttpRequest extends EventTarget {
  constructor() {
    super();
    Object.defineProperty(this, __tbXhrData, {
      value: {
        state: 0, sent: false, method: '', url: '', headers: new Headers(),
        responseHeaders: new Headers(), responseURL: '', status: 0, responseBytes: new Uint8Array(),
        responseType: '', timeout: 0, withCredentials: false, generation: 0,
        upload: new XMLHttpRequestUpload(),
      },
    });
  }
  get readyState() { return __tbBrand(this, __tbXhrData).state; }
  get status() { return __tbBrand(this, __tbXhrData).status; }
  get statusText() { return ''; }
  get responseURL() { return __tbBrand(this, __tbXhrData).responseURL; }
  get upload() { return __tbBrand(this, __tbXhrData).upload; }
  get timeout() { return __tbBrand(this, __tbXhrData).timeout; }
  set timeout(value) { __tbBrand(this, __tbXhrData).timeout = Number(value) >>> 0; }
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
    const data = __tbBrand(this, __tbXhrData);
    if (data.responseType === '' || data.responseType === 'text') return this.responseText;
    if (data.state !== 4) return null;
    if (data.responseType === 'arraybuffer') return data.responseBytes.slice().buffer;
    if (data.responseType === 'blob') return new Blob([data.responseBytes], { type: data.responseHeaders.get('content-type') || '' });
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
    const parsed = globalThis.__tbResolveUrl(String(url), undefined);
    if (parsed === null) throw new DOMException('Invalid URL', 'SyntaxError');
    if (!async) throw new DOMException('Synchronous requests are not supported', 'InvalidAccessError');
    data.generation++;
    data.method = /^(GET|HEAD|POST|PUT|DELETE|OPTIONS)$/i.test(verb) ? verb.toUpperCase() : verb;
    data.url = parsed;
    data.sent = false;
    data.headers = new Headers();
    data.responseHeaders = new Headers();
    data.responseURL = '';
    data.responseBytes = new Uint8Array();
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
    data.headers.append(name, value);
  }
  // https://xhr.spec.whatwg.org/#the-send()-method
  send(body = null) {
    const data = __tbBrand(this, __tbXhrData);
    if (data.state !== 1 || data.sent) throw new DOMException('Request not open', 'InvalidStateError');
    if (data.method === 'GET' || data.method === 'HEAD') body = null;
    data.sent = true;
    const generation = data.generation;
    this.dispatchEvent(new ProgressEvent('loadstart'));
    if (data.state !== 1 || !data.sent || data.generation !== generation) return;
    __tbQueuePageRequest(data.url, data.method, body, data.headers, (status, bytes, url, type, fields) => {
      if (data.generation !== generation || !data.sent) return;
      if (status === 0) {
        data.state = 4;
        data.sent = false;
        this.dispatchEvent(new Event('readystatechange'));
        this.dispatchEvent(new ProgressEvent('error'));
        this.dispatchEvent(new ProgressEvent('loadend'));
        return;
      }
      data.status = status;
      data.responseURL = url;
      data.responseHeaders = new Headers(fields);
      if (type && !data.responseHeaders.has('content-type')) data.responseHeaders.set('content-type', type);
      data.state = 2;
      this.dispatchEvent(new Event('readystatechange'));
      if (data.state !== 2) return;
      data.responseBytes = bytes;
      if (bytes.length) {
        data.state = 3;
        this.dispatchEvent(new Event('readystatechange'));
        this.dispatchEvent(new ProgressEvent('progress', { loaded: bytes.length }));
      }
      data.state = 4;
      data.sent = false;
      this.dispatchEvent(new Event('readystatechange'));
      this.dispatchEvent(new ProgressEvent('load', { loaded: bytes.length }));
      this.dispatchEvent(new ProgressEvent('loadend', { loaded: bytes.length }));
    });
  }
  // https://xhr.spec.whatwg.org/#the-abort()-method
  abort() {
    const data = __tbBrand(this, __tbXhrData);
    data.generation++;
    if (data.sent || data.state === 2 || data.state === 3) {
      data.sent = false;
      data.state = 4;
      data.status = 0;
      data.responseBytes = new Uint8Array();
      this.dispatchEvent(new Event('readystatechange'));
      this.dispatchEvent(new ProgressEvent('abort'));
      this.dispatchEvent(new ProgressEvent('loadend'));
    }
    if (data.state === 4) data.state = 0;
  }
  getResponseHeader(name) {
    const data = __tbBrand(this, __tbXhrData);
    if (data.state < 2 || /^set-cookie2?$/i.test(String(name))) return null;
    return data.responseHeaders.get(name);
  }
  getAllResponseHeaders() {
    const data = __tbBrand(this, __tbXhrData);
    if (data.state < 2) return '';
    return Array.from(data.responseHeaders).filter(([name]) => !/^set-cookie2?$/i.test(name))
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
