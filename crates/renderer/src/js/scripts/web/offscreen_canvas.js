// OffscreenCanvas is a detached canvas in this engine. A hidden <canvas>
// element owns the bitmap and the 2D context; this wrapper keeps the element
// and the per-object state in private maps so page code cannot retarget or
// forge it. Moving the bitmap to a Rust host is the prerequisite for workers.
// https://html.spec.whatwg.org/multipage/canvas.html#the-offscreencanvas-interface
(function () {
  if (typeof globalThis.OffscreenCanvas === 'function') {
    return;
  }

  const states = new __tbPrivateMap();
  const contexts = new __tbPrivateMap();

  // `[EnforceRange] unsigned long long`: finite, integral, in 0..2^53-1.
  function toUnsignedLongLong(value) {
    const number = Number(value);
    if (!Number.isFinite(number)) {
      throw new TypeError('value is not a finite number');
    }
    const integer = Math.trunc(number);
    if (integer < 0 || integer > Number.MAX_SAFE_INTEGER) {
      throw new TypeError('value is outside the unsigned long long range');
    }
    return integer;
  }

  function stateOf(canvas) {
    const state = states.get(canvas);
    if (!state) {
      throw new TypeError('Illegal invocation');
    }
    return state;
  }

  const base = globalThis.CanvasRenderingContext2D;
  let offscreenPrototype = base && base.prototype;
  if (base) {
    function OffscreenCanvasRenderingContext2D() {}
    const prototype = Object.create(Object.prototype);
    for (const key of Object.getOwnPropertyNames(base.prototype)) {
      if (key === 'constructor' || key === 'canvas') {
        continue;
      }
      Object.defineProperty(
        prototype,
        key,
        Object.getOwnPropertyDescriptor(base.prototype, key),
      );
    }
    Object.defineProperty(prototype, 'canvas', {
      configurable: true,
      get() {
        const state = contexts.get(this);
        if (!state) {
          throw new TypeError('Illegal invocation');
        }
        return state.owner;
      },
    });
    Object.defineProperty(prototype, 'constructor', {
      configurable: true,
      writable: true,
      value: OffscreenCanvasRenderingContext2D,
    });
    Object.defineProperty(OffscreenCanvasRenderingContext2D, 'prototype', {
      value: prototype,
      writable: false,
      configurable: false,
    });
    globalThis.OffscreenCanvasRenderingContext2D = OffscreenCanvasRenderingContext2D;
    offscreenPrototype = prototype;
  }

  const validContexts = new Set(['2d', 'bitmaprenderer', 'webgl', 'webgl2', 'webgpu']);

  class OffscreenCanvas {
    constructor(width, height) {
      if (arguments.length < 2) {
        throw new TypeError('OffscreenCanvas needs width and height');
      }
      const canvas = document.createElement('canvas');
      canvas.width = toUnsignedLongLong(width);
      canvas.height = toUnsignedLongLong(height);
      // Non-writable and non-configurable so page code cannot retarget the
      // wrapper at another element. The element stays an implementation
      // detail of the current host until OffscreenCanvas moves to Rust.
      Object.defineProperty(this, '__tbCanvas', {
        value: canvas,
        enumerable: false,
        writable: false,
        configurable: false,
      });
      states.set(this, {
        canvas,
        context: null,
        detached: false,
      });
    }

    get width() { return stateOf(this).canvas.width; }
    set width(value) { stateOf(this).canvas.width = toUnsignedLongLong(value); }

    get height() { return stateOf(this).canvas.height; }
    set height(value) { stateOf(this).canvas.height = toUnsignedLongLong(value); }

    getContext(type, options) {
      const state = stateOf(this);
      if (state.detached) {
        throw new DOMException('the OffscreenCanvas is detached', 'InvalidStateError');
      }
      if (arguments.length === 0) {
        throw new TypeError('getContext needs a context type');
      }
      const contextId = String(type);
      if (!validContexts.has(contextId)) {
        throw new TypeError('unknown context type');
      }
      if (contextId !== '2d') {
        return null;
      }
      if (state.context) {
        return state.context;
      }
      const context = state.canvas.getContext('2d', options);
      if (context) {
        if (offscreenPrototype) {
          Object.setPrototypeOf(context, offscreenPrototype);
        }
        contexts.set(context, { owner: this });
        state.context = context;
      }
      return context;
    }

    transferToImageBitmap() {
      const state = stateOf(this);
      if (state.detached) {
        throw new DOMException('the OffscreenCanvas is detached', 'InvalidStateError');
      }
      if (!state.context) {
        throw new DOMException('the OffscreenCanvas has no 2D context', 'InvalidStateError');
      }
      return globalThis.__tbCanvasTransferToImageBitmap(state.canvas);
    }

    convertToBlob(options) {
      const state = stateOf(this);
      if (state.detached) {
        return Promise.reject(
          new DOMException('the OffscreenCanvas is detached', 'InvalidStateError'),
        );
      }
      if (!state.context) {
        return Promise.reject(
          new DOMException('the OffscreenCanvas has no 2D context', 'IndexSizeError'),
        );
      }
      if (state.canvas.width === 0 || state.canvas.height === 0) {
        return Promise.reject(
          new DOMException('the OffscreenCanvas has a zero-sized bitmap', 'IndexSizeError'),
        );
      }
      if (
        options !== undefined &&
        options !== null &&
        typeof options !== 'object' &&
        typeof options !== 'function'
      ) {
        return Promise.reject(new TypeError('options must be an object'));
      }
      try {
        const blob = globalThis.__tbCanvasToBlobBytes(
          state.canvas,
          options ? options.type : undefined,
          options ? options.quality : undefined,
        );
        if (blob === null) {
          return Promise.reject(
            new DOMException('the OffscreenCanvas could not be serialized', 'IndexSizeError'),
          );
        }
        return Promise.resolve(new Blob([blob.bytes], { type: blob.type }));
      } catch (error) {
        return Promise.reject(error);
      }
    }
  }

  Object.defineProperty(OffscreenCanvas.prototype, Symbol.toStringTag, {
    configurable: true,
    value: 'OffscreenCanvas',
  });
  globalThis.OffscreenCanvas = OffscreenCanvas;

  // Messaging and drawImage use these branded helpers instead of a writable
  // expando. Neither exposes the hidden element.
  globalThis.__tbOffscreenDetached = function (canvas) {
    const state = canvas && typeof canvas === 'object' ? states.get(canvas) : undefined;
    return Boolean(state && state.detached);
  };
  globalThis.__tbOffscreenHasContext = function (canvas) {
    const state = canvas && typeof canvas === 'object' ? states.get(canvas) : undefined;
    return Boolean(state && state.context);
  };
  globalThis.__tbOffscreenDetach = function (canvas) {
    const state = stateOf(canvas);
    state.detached = true;
    // Transferring moves the bitmap away, leaving a zero-sized bitmap. The
    // hidden element follows the same rule so drawImage sees an invalid source
    // without a page-writable detach flag.
    state.canvas.width = 0;
    state.canvas.height = 0;
  };
})();
