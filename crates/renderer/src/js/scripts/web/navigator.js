// Navigator identity exposed by the browsing-context global.
// https://html.spec.whatwg.org/multipage/system-state.html#the-navigator-object
{
  class Navigator {
    get userAgent() { return 'tinybrowser/0.1'; }
    get appVersion() { return this.userAgent; }
    get platform() { return 'Linux x86_64'; }
    get language() { return 'en-US'; }
    get languages() { return Object.freeze(['en-US', 'en']); }
    get onLine() { return true; }
    get hardwareConcurrency() { return 1; }
    sendBeacon() { return false; }
  }
  Object.defineProperty(globalThis, 'Navigator', {
    value: Navigator, writable: true, configurable: true,
  });
  Object.defineProperty(globalThis, 'navigator', {
    value: new Navigator(), writable: false, configurable: true,
  });
}
