// The Rust ImageData constructor does not check new.target; wrap it so a plain
// call throws like every WebIDL constructor.
// https://webidl.spec.whatwg.org/#internally-create-a-new-object-implementing-the-interface
(function () {
  const Native = globalThis.ImageData;
  if (typeof Native !== 'function') {
    return;
  }
  function ImageData(...args) {
    if (new.target === undefined) {
      throw new TypeError("ImageData constructor requires 'new'");
    }
    return Reflect.construct(Native, args, new.target);
  }
  Object.defineProperty(ImageData, 'prototype', {
    value: Native.prototype,
    writable: false,
    configurable: false,
  });
  Object.defineProperty(Native.prototype, 'constructor', {
    value: ImageData,
    writable: true,
    configurable: true,
  });
  Object.defineProperty(ImageData, 'length', { value: Native.length });
  globalThis.ImageData = ImageData;
})();
