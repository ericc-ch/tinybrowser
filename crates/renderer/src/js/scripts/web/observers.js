// Visibility observation with the headless viewport as the root.
// https://w3c.github.io/IntersectionObserver/#intersection-observer-interface
{
  const observers = host.slots('IntersectionObserver');
  const queueMicrotask = globalThis.queueMicrotask;
  class IntersectionObserver {
    constructor(callback, options = {}) {
      if (typeof callback !== 'function') throw new TypeError('callback must be callable');
      this.root = options.root ?? null;
      this.rootMargin = options.rootMargin ?? '0px 0px 0px 0px';
      this.thresholds = Object.freeze(Array.isArray(options.threshold)
        ? options.threshold.map(Number) : [Number(options.threshold ?? 0)]);
      observers.set(this, { callback, targets: new __tbPrivateSet() });
    }
    observe(target) {
      const data = observers.get(this);
      data.targets.add(target);
      queueMicrotask(() => {
        if (data.targets.has(target)) {
          __tbApply(data.callback, this, [[{
            target, isIntersecting: true, intersectionRatio: 1,
            boundingClientRect: target.getBoundingClientRect(),
            intersectionRect: target.getBoundingClientRect(),
            rootBounds: null, time: performance.now(),
          }], this]);
        }
      });
    }
    unobserve(target) { observers.get(this).targets.delete(target); }
    disconnect() { observers.get(this).targets.clear(); }
    takeRecords() { return []; }
  }
  Object.defineProperty(globalThis, 'IntersectionObserver', {
    value: IntersectionObserver, writable: true, configurable: true,
  });
}
