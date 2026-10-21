// OffscreenCanvas is a detached canvas in this engine. A hidden <canvas>
// element owns the bitmap and the existing 2D context, so the OffscreenCanvas
// object forwards size and context calls to it.
// https://html.spec.whatwg.org/multipage/canvas.html#the-offscreencanvas-interface
(function () {
  if (typeof globalThis.OffscreenCanvas === 'function') {
    return;
  }

  const base = globalThis.CanvasRenderingContext2D;
  let offscreenPrototype = base && base.prototype;
  if (base) {
    function OffscreenCanvasRenderingContext2D() {}
    const prototype = Object.create(Object.prototype);
    for (const key of Object.getOwnPropertyNames(base.prototype)) {
      if (key === 'constructor') {
        continue;
      }
      Object.defineProperty(
        prototype,
        key,
        Object.getOwnPropertyDescriptor(base.prototype, key),
      );
    }
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

  // `[EnforceRange] unsigned long` rejects non-finite and out-of-range
  // values instead of coercing them to zero
  // (<https://webidl.spec.whatwg.org/#es-unsigned-long>).
  function toUnsignedLong(value) {
    const number = Number(value);
    if (!Number.isFinite(number)) {
      throw new TypeError('value is not a finite number');
    }
    const integer = Math.trunc(number);
    if (integer < 0 || integer > 4294967295) {
      throw new TypeError('value is outside the unsigned long range');
    }
    return integer;
  }

  class OffscreenCanvas {
    constructor(width, height) {
      if (arguments.length < 2) {
        throw new TypeError('OffscreenCanvas needs width and height');
      }
      const canvas = document.createElement('canvas');
      canvas.width = toUnsignedLong(width);
      canvas.height = toUnsignedLong(height);
      Object.defineProperties(this, {
        __tbCanvas: { value: canvas, configurable: true },
        __tbDoc: { value: canvas.__tbDoc, configurable: true },
        __tbNode: { value: canvas.__tbNode, configurable: true },
      });
    }

    get width() { return this.__tbCanvas.width; }
    set width(value) { this.__tbCanvas.width = toUnsignedLong(value); }

    get height() { return this.__tbCanvas.height; }
    set height(value) { this.__tbCanvas.height = toUnsignedLong(value); }

    getContext(type, options) {
      if (this.__tbDetached) {
        throw new DOMException('the OffscreenCanvas is detached', 'InvalidStateError');
      }
      if (arguments.length === 0) {
        throw new TypeError('getContext needs a context type');
      }
      if (String(type) !== '2d') {
        throw new TypeError('unsupported context type');
      }
      const context = this.__tbCanvas.getContext('2d', options);
      if (context && !context.__tbOffscreenCanvas) {
        if (offscreenPrototype) {
          Object.setPrototypeOf(context, offscreenPrototype);
        }
        Object.defineProperties(context, {
          canvas: { value: this, configurable: true },
          __tbOffscreenCanvas: { value: this, configurable: true },
        });
      }
      return context;
    }

    transferToImageBitmap() {
      if (this.__tbDetached) {
        throw new DOMException('the OffscreenCanvas is detached', 'InvalidStateError');
      }
      return globalThis.__tbCanvasTransferToImageBitmap(this.__tbCanvas);
    }

    convertToBlob(options) {
      try {
        const type = options && options.type ? options.type : 'image/png';
        const bytes = globalThis.__tbCanvasToBlobBytes(this, type, options && options.quality);
        return Promise.resolve(new Blob([new Uint8Array(bytes)], { type }));
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
})();
