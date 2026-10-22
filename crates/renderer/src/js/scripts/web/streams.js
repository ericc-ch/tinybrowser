// A ReadableStream with a synchronous chunk queue: enough for Blob.stream()
// and other producers that push all their chunks up front.
const __tbStreamBrand = value => __tbBrand(value, __tbStreamData);
const __tbWritableData = host.slots('tinybrowser.writable.data');
globalThis.ReadableStream = class ReadableStream {
  constructor(source, strategy) {
    const sourceObject = source === undefined ? {} : Object(source);
    const data = {
      chunks: [], closed: false, errored: null, canceled: false, reader: null, disturbed: false,
      pull: typeof sourceObject.pull === 'function' ? sourceObject.pull : null,
      cancel: typeof sourceObject.cancel === 'function' ? sourceObject.cancel : null,
      controller: null,
      source: sourceObject,
    };
    __tbStreamData.set(this, data);
    data.controller = {
      enqueue(chunk) { if (!data.closed && data.errored === null && !data.canceled) __tbArray.push(data.chunks, chunk); },
      close() { data.closed = true; },
      error(error) { data.errored = error; },
      get desiredSize() { return data.closed ? null : 1; },
    };
    if (typeof sourceObject.start === 'function') sourceObject.start(data.controller);
  }
  get locked() { return __tbStreamBrand(this).reader !== null; }
  getReader(options) {
    const data = __tbStreamBrand(this);
    if (data.reader !== null) throw new TypeError('The stream is locked to a reader');
    if (options !== undefined && options !== null && Object(options).mode === 'byob') {
      const reader = {
        read: view => {
          if (!ArrayBuffer.isView(view) || !(view instanceof Uint8Array)) {
            return Promise.reject(new TypeError('BYOB read needs a Uint8Array view'));
          }
          const step = () => {
            data.disturbed = true;
            if (data.errored !== null) return Promise.reject(data.errored);
            if (data.chunks.length === 0) {
              if (data.closed || data.canceled) return Promise.resolve({ value: view.subarray(0, 0), done: true });
              if (data.pull !== null) return Promise.resolve(__tbApply(data.pull, data.source, [data.controller])).then(step);
              return Promise.resolve({ value: view.subarray(0, 0), done: true });
            }
            const chunk = __tbArray.shift(data.chunks);
            const bytes = chunk instanceof Uint8Array
              ? chunk
              : ArrayBuffer.isView(chunk)
                ? new Uint8Array(chunk.buffer, chunk.byteOffset, chunk.byteLength)
                : chunk instanceof ArrayBuffer
                  ? new Uint8Array(chunk)
                  : null;
            if (bytes === null) return Promise.reject(new TypeError('A body stream chunk must be a BufferSource'));
            const length = Math.min(bytes.length, view.byteLength);
            view.set(bytes.subarray(0, length));
            if (length < bytes.length) __tbArray.push(data.chunks, bytes.subarray(length));
            return Promise.resolve({ value: view.subarray(0, length), done: false });
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
    const reader = {
      read: () => {
        const step = () => {
          data.disturbed = true;
          if (data.errored !== null) return Promise.reject(data.errored);
          if (data.chunks.length > 0) return Promise.resolve({ value: __tbArray.shift(data.chunks), done: false });
          if (data.closed || data.canceled) return Promise.resolve({ value: undefined, done: true });
          if (data.pull !== null) return Promise.resolve(__tbApply(data.pull, data.source, [data.controller])).then(step);
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
    data.disturbed = true;
    // Canceling resets the queue: a later `read()` resolves done, it does
    // not drain what was queued
    // (<https://streams.spec.whatwg.org/#readable-stream-cancel>).
    data.canceled = true;
    data.chunks.length = 0;
    if (data.cancel !== null) return Promise.resolve(__tbApply(data.cancel, data.source, [reason]));
    return Promise.resolve();
  }
  pipeTo(destination, options) {
    const data = __tbStreamBrand(this);
    data.disturbed = true;
    const reader = this.getReader();
    const writer = destination.getWriter();
    const pump = () => reader.read().then(step => {
      if (step.done) return writer.close();
      return Promise.resolve(writer.write(step.value)).then(pump);
    });
    return pump().catch(error => {
      writer.abort(error);
      throw error;
    });
  }
  pipeThrough(transform, options) {
    this.pipeTo(transform.writable, options);
    return transform.readable;
  }
  tee() {
    const data = __tbStreamBrand(this);
    if (data.reader !== null) throw new TypeError('The stream is locked to a reader');
    const reader = this.getReader();
    const queues = [[], []];
    let done = false;
    let error = null;
    let pumping = null;
    const clone = value => typeof structuredClone === 'function' ? structuredClone(value) : value;
    const pump = () => {
      if (pumping !== null) return pumping;
      pumping = reader.read().then(step => {
        pumping = null;
        if (step.done) {
          done = true;
          return;
        }
        __tbArray.push(queues[0], clone(step.value));
        __tbArray.push(queues[1], clone(step.value));
      }, reason => {
        pumping = null;
        error = reason;
        done = true;
      });
      return pumping;
    };
    const branch = index => new globalThis.ReadableStream({
      pull(controller) {
        const step = () => {
          if (error !== null) {
            controller.error(error);
            return;
          }
          if (queues[index].length > 0) {
            controller.enqueue(__tbArray.shift(queues[index]));
            return;
          }
          if (done) {
            controller.close();
            return;
          }
          return pump().then(step);
        };
        return step();
      },
    });
    return [branch(0), branch(1)];
  }
};
Object.defineProperty(globalThis.ReadableStream.prototype, Symbol.toStringTag, { value: 'ReadableStream', writable: false, enumerable: false, configurable: true });
const __tbStreamDisturbed = stream => __tbStreamBrand(stream).disturbed;
globalThis.WritableStream = class WritableStream {
  constructor(sink) {
    const sinkObject = sink === undefined ? {} : Object(sink);
    __tbWritableData.set(this, { sink: sinkObject, writer: null, closed: false });
  }
  get locked() { return __tbWritableData.get(this).writer !== null; }
  getWriter() {
    const data = __tbWritableData.get(this);
    if (data.writer !== null) throw new TypeError('The stream is locked to a writer');
    const writer = {
      write: chunk => {
        if (data.closed) return Promise.reject(new TypeError('The stream is closed'));
        if (typeof data.sink.write !== 'function') return Promise.resolve();
        return Promise.resolve(__tbApply(data.sink.write, data.sink, [chunk]));
      },
      close: () => {
        data.closed = true;
        if (typeof data.sink.close !== 'function') return Promise.resolve();
        return Promise.resolve(__tbApply(data.sink.close, data.sink, []));
      },
      abort: reason => {
        data.closed = true;
        if (typeof data.sink.abort !== 'function') return Promise.resolve();
        return Promise.resolve(__tbApply(data.sink.abort, data.sink, [reason]));
      },
      releaseLock: () => { data.writer = null; },
      closed: Promise.resolve(),
      desiredSize: 1,
    };
    data.writer = writer;
    return writer;
  }
  abort(reason) {
    const data = __tbWritableData.get(this);
    data.closed = true;
    if (typeof data.sink.abort !== 'function') return Promise.resolve();
    return Promise.resolve(__tbApply(data.sink.abort, data.sink, [reason]));
  }
};
Object.defineProperty(globalThis.WritableStream.prototype, Symbol.toStringTag, { value: 'WritableStream', writable: false, enumerable: false, configurable: true });
