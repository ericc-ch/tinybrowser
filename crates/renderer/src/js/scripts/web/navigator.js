// Navigator identity exposed by the browsing-context global.
// https://html.spec.whatwg.org/multipage/system-state.html#the-navigator-object
{
  const uaBrands = Object.freeze([
    Object.freeze({ brand: 'Not_A Brand', version: '99' }),
    Object.freeze({ brand: 'Chromium', version: '152' }),
    Object.freeze({ brand: 'Google Chrome', version: '152' }),
  ]);
  const uaFullVersionList = Object.freeze([
    Object.freeze({ brand: 'Not_A Brand', version: '99.0.0.0' }),
    Object.freeze({ brand: 'Chromium', version: '152.0.0.0' }),
    Object.freeze({ brand: 'Google Chrome', version: '152.0.0.0' }),
  ]);
  const uaDataKey = Symbol('NavigatorUAData');
  class NavigatorUAData {
    constructor() {
      throw new TypeError('Illegal constructor');
    }
    get brands() {
      __tbBrand(this, uaDataKey);
      return uaBrands;
    }
    get mobile() {
      __tbBrand(this, uaDataKey);
      return false;
    }
    get platform() {
      __tbBrand(this, uaDataKey);
      return 'Linux';
    }
    // https://wicg.github.io/ua-client-hints/#getHighEntropyValues
    getHighEntropyValues(hints) {
      __tbBrand(this, uaDataKey);
      if (arguments.length < 1) {
        throw new TypeError("Failed to execute 'getHighEntropyValues' on 'NavigatorUAData': 1 argument required, but only 0 present.");
      }
      if (hints === null || (typeof hints !== 'object' && typeof hints !== 'function')) {
        throw new TypeError('The hints argument must be a sequence');
      }
      const iteratorMethod = hints[Symbol.iterator];
      if (typeof iteratorMethod !== 'function') {
        throw new TypeError('The hints argument must be iterable');
      }
      const names = [];
      const iterator = iteratorMethod.call(hints);
      while (true) {
        const step = iterator.next();
        if (step.done) break;
        names.push(String(step.value));
      }
      const values = { brands: uaBrands, mobile: false, platform: 'Linux' };
      if (names.includes('architecture')) values.architecture = 'x86';
      if (names.includes('bitness')) values.bitness = '64';
      if (names.includes('formFactors')) values.formFactors = Object.freeze(['Desktop']);
      if (names.includes('fullVersionList')) values.fullVersionList = uaFullVersionList;
      if (names.includes('model')) values.model = '';
      if (names.includes('platformVersion')) values.platformVersion = '';
      if (names.includes('uaFullVersion')) values.uaFullVersion = '152.0.0.0';
      if (names.includes('wow64')) values.wow64 = false;
      return Promise.resolve(values);
    }
    // https://wicg.github.io/ua-client-hints/#h-tojson
    toJSON() {
      __tbBrand(this, uaDataKey);
      return { brands: uaBrands, mobile: false, platform: 'Linux' };
    }
  }
  Object.defineProperty(NavigatorUAData.prototype, Symbol.toStringTag, {
    value: 'NavigatorUAData', writable: false, enumerable: false, configurable: true,
  });
  const userAgentData = Object.create(NavigatorUAData.prototype);
  Object.defineProperty(userAgentData, uaDataKey, {
    value: Object.freeze({}), writable: false, enumerable: false, configurable: false,
  });
  class Navigator {
    get userAgent() {
      return 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/152.0.0.0 Safari/537.36';
    }
    get appVersion() { return this.userAgent.replace(/^Mozilla\//, ''); }
    get appCodeName() { return 'Mozilla'; }
    get appName() { return 'Netscape'; }
    get product() { return 'Gecko'; }
    get productSub() { return '20030107'; }
    get vendor() { return 'Google Inc.'; }
    get vendorSub() { return ''; }
    get platform() { return 'Linux x86_64'; }
    get language() { return 'en-US'; }
    get languages() { return Object.freeze(['en-US', 'en']); }
    get onLine() { return true; }
    get hardwareConcurrency() { return 1; }
    get maxTouchPoints() { return 0; }
    get cookieEnabled() { return true; }
    get webdriver() { return false; }
    get userAgentData() {
      if (globalThis.isSecureContext === false) return undefined;
      return userAgentData;
    }
    sendBeacon() { return false; }
  }
  Object.defineProperty(globalThis, 'NavigatorUAData', {
    value: NavigatorUAData, writable: true, configurable: true,
  });
  Object.defineProperty(globalThis, 'Navigator', {
    value: Navigator, writable: true, configurable: true,
  });
  Object.defineProperty(globalThis, 'navigator', {
    value: new Navigator(), writable: false, configurable: true,
  });
}
