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
    // Canceling resets the queue: a later `read()` resolves done, it does
    // not drain what was queued
    // (<https://streams.spec.whatwg.org/#readable-stream-cancel>).
    data.canceled = true;
    data.chunks.length = 0;
    if (data.cancel !== null) return Promise.resolve(data.cancel(reason));
    return Promise.resolve();
  }
};
Object.defineProperty(globalThis.ReadableStream.prototype, Symbol.toStringTag, { value: 'ReadableStream', writable: false, enumerable: false, configurable: true });
