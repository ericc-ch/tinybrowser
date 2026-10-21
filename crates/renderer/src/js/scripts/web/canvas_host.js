// HTMLCanvasElement serialization. The renderer owns the bitmap and PNG
// encoder; these wrappers only build the data URL or Blob.
// https://html.spec.whatwg.org/multipage/canvas.html#dom-canvas-todataurl
(function () {
  const canvas = document.createElement('canvas');
  const prototype = Object.getPrototypeOf(canvas);
  if (typeof prototype.toDataURL === 'function') {
    return;
  }

  Object.defineProperty(prototype, 'toDataURL', {
    configurable: true,
    writable: true,
    value: function toDataURL(type, quality) {
      return globalThis.__tbCanvasToDataURL(this, type, quality);
    },
  });

  Object.defineProperty(prototype, 'toBlob', {
    configurable: true,
    writable: true,
    value: function toBlob(callback, type, quality) {
      if (typeof callback !== 'function') {
        throw new TypeError('toBlob needs a callback');
      }
      const bytes = globalThis.__tbCanvasToBlobBytes(this, type, quality);
      const blob = new Blob([new Uint8Array(bytes)], {
        type: type || 'image/png',
      });
      Promise.resolve().then(() => callback(blob));
    },
  });
})();
