// HTMLCanvasElement serialization. The renderer owns the bitmap and the
// encoder; these wrappers only adapt the IDL argument list and the Blob
// callback. `toBlob` encodes synchronously for now, but schedules the callback
// as a task so it runs after the current task's microtasks.
// https://html.spec.whatwg.org/multipage/canvas.html#dom-canvas-todataurl
(function () {
  const canvas = document.createElement('canvas');
  const prototype = Object.getPrototypeOf(canvas);
  if (typeof prototype.toDataURL === 'function') {
    return;
  }

  function normalizedType(type) {
    // `type` is an optional DOMString, so a missing argument uses image/png
    // and any other value goes through ToString.
    const text = type === undefined ? 'image/png' : String(type);
    return text.toLowerCase() === 'image/jpeg' ? 'image/jpeg' : 'image/png';
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
      const mime = normalizedType(type);
      const bytes = globalThis.__tbCanvasToBlobBytes(this, type, quality);
      // The renderer returns no bytes for a zero-sized canvas; the spec asks
      // for null rather than an empty Blob.
      const blob = bytes.length === 0 ? null : new Blob([new Uint8Array(bytes)], { type: mime });
      setTimeout(() => callback(blob), 0);
    },
  });
})();
