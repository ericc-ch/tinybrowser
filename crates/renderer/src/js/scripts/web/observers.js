// Visibility observation with the headless viewport as the root.
// https://w3c.github.io/IntersectionObserver/#intersection-observer-interface
{
  class IntersectionObserver {
    constructor(callback, options = {}) {
      if (typeof callback !== 'function') throw new TypeError('callback must be callable');
      this._callback = callback;
      this.root = options.root ?? null;
      this.rootMargin = options.rootMargin ?? '0px 0px 0px 0px';
      this.thresholds = Object.freeze(Array.isArray(options.threshold)
        ? options.threshold.map(Number) : [Number(options.threshold ?? 0)]);
      this._targets = new Set();
    }
    observe(target) {
      this._targets.add(target);
      queueMicrotask(() => {
        if (this._targets.has(target)) {
          this._callback([{
            target, isIntersecting: true, intersectionRatio: 1,
            boundingClientRect: target.getBoundingClientRect(),
            intersectionRect: target.getBoundingClientRect(),
            rootBounds: null, time: performance.now(),
          }], this);
        }
      });
    }
    unobserve(target) { this._targets.delete(target); }
    disconnect() { this._targets.clear(); }
    takeRecords() { return []; }
  }
  Object.defineProperty(globalThis, 'IntersectionObserver', {
    value: IntersectionObserver, writable: true, configurable: true,
  });
}
