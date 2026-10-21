// OffscreenCanvas is a detached canvas in this engine. A hidden <canvas>
// element owns the bitmap and the existing 2D context, so the OffscreenCanvas
// object forwards size and context calls to it.
// https://html.spec.whatwg.org/multipage/canvas.html#the-offscreencanvas-interface
(function () {
  if (typeof globalThis.OffscreenCanvas === 'function') {
    return;
  }

  class OffscreenCanvas {
    constructor(width, height) {
      if (arguments.length < 2) {
        throw new TypeError('OffscreenCanvas needs width and height');
      }
      const canvas = document.createElement('canvas');
      canvas.width = Number(width);
      canvas.height = Number(height);
      Object.defineProperties(this, {
        __tbCanvas: { value: canvas, configurable: true },
        __tbDoc: { value: canvas.__tbDoc, configurable: true },
        __tbNode: { value: canvas.__tbNode, configurable: true },
      });
    }

    get width() { return this.__tbCanvas.width; }
    set width(value) { this.__tbCanvas.width = Number(value); }

    get height() { return this.__tbCanvas.height; }
    set height(value) { this.__tbCanvas.height = Number(value); }

    getContext(type, options) {
      const context = this.__tbCanvas.getContext(type, options);
      if (context && !context.__tbOffscreenCanvas) {
        Object.defineProperties(context, {
          canvas: { value: this, configurable: true },
          __tbOffscreenCanvas: { value: this, configurable: true },
        });
      }
      return context;
    }

    transferToImageBitmap() {
      throw new DOMException('transferToImageBitmap is not implemented', 'NotSupportedError');
    }

    convertToBlob() {
      return Promise.reject(
        new DOMException('convertToBlob is not implemented', 'NotSupportedError'),
      );
    }
  }

  Object.defineProperty(OffscreenCanvas.prototype, Symbol.toStringTag, {
    configurable: true,
    value: 'OffscreenCanvas',
  });
  globalThis.OffscreenCanvas = OffscreenCanvas;
})();
