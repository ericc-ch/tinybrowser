// Video and audio playback are not implemented. The canvas drawImage tests
// call play() on a source-less video to prove the engine does not crash; a
// resolved promise keeps that path observable without producing frames.
// https://html.spec.whatwg.org/multipage/media.html#dom-media-play
(function () {
  const prototype = globalThis.HTMLMediaElement && globalThis.HTMLMediaElement.prototype;
  if (!prototype) {
    return;
  }
  if (typeof prototype.play !== 'function') {
    Object.defineProperty(prototype, 'play', {
      value: function play() { return Promise.resolve(); },
      writable: true,
      configurable: true,
    });
  }
  if (typeof prototype.pause !== 'function') {
    Object.defineProperty(prototype, 'pause', {
      value: function pause() {},
      writable: true,
      configurable: true,
    });
  }
})();
