// Navigator identity exposed by the browsing-context global.
// https://html.spec.whatwg.org/multipage/system-state.html#the-navigator-object
{
  const identity = host.__tbIdentity();
  const uaBrands = Object.freeze(identity.brands.map((entry) => Object.freeze(entry)));
  const uaFullVersionList = Object.freeze(identity.fullVersionList.map((entry) => Object.freeze(entry)));
  const uaMobile = identity.mobile;
  const uaPlatform = identity.platform;
  // `uaFullVersion` reports the engine version, so the Chromium brand wins; an
  // identity without one falls back to its last brand rather than to nothing
  // https://wicg.github.io/ua-client-hints/#getHighEntropyValues
  const chromium = uaFullVersionList.find((entry) => entry.brand === 'Chromium');
  const lastFullVersion = uaFullVersionList[uaFullVersionList.length - 1];
  const uaFullVersion = chromium !== undefined
    ? chromium.version
    : (lastFullVersion === undefined ? '' : lastFullVersion.version);
  const uaDataKey = host.slots('NavigatorUAData');
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
      return uaMobile;
    }
    get platform() {
      __tbBrand(this, uaDataKey);
      return uaPlatform;
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
      const values = { brands: uaBrands, mobile: uaMobile, platform: uaPlatform };
      if (names.includes('architecture')) values.architecture = identity.architecture;
      if (names.includes('bitness')) values.bitness = identity.bitness;
      if (names.includes('formFactors')) values.formFactors = Object.freeze(identity.formFactors);
      if (names.includes('fullVersionList')) values.fullVersionList = uaFullVersionList;
      if (names.includes('model')) values.model = identity.model;
      if (names.includes('platformVersion')) values.platformVersion = identity.platformVersion;
      if (names.includes('uaFullVersion')) values.uaFullVersion = uaFullVersion;
      if (names.includes('wow64')) values.wow64 = identity.wow64;
      return Promise.resolve(values);
    }
    // https://wicg.github.io/ua-client-hints/#h-tojson
    toJSON() {
      __tbBrand(this, uaDataKey);
      return { brands: uaBrands, mobile: uaMobile, platform: uaPlatform };
    }
  }
  Object.defineProperty(NavigatorUAData.prototype, Symbol.toStringTag, {
    value: 'NavigatorUAData', writable: false, enumerable: false, configurable: true,
  });
  const userAgentData = Object.create(NavigatorUAData.prototype);
  uaDataKey.set(userAgentData, Object.freeze({}));
  // https://html.spec.whatwg.org/multipage/interaction.html#dom-useractivation
  const userActivationKey = host.slots('UserActivation');
  class UserActivation {
    constructor() {
      throw new TypeError('Illegal constructor');
    }
    get isActive() {
      __tbBrand(this, userActivationKey);
      return host.__tbUserActivationActive();
    }
    get hasBeenActive() {
      __tbBrand(this, userActivationKey);
      return host.__tbUserActivationSticky();
    }
  }
  Object.defineProperty(UserActivation.prototype, Symbol.toStringTag, {
    value: 'UserActivation', writable: false, enumerable: false, configurable: true,
  });
  const userActivation = Object.create(UserActivation.prototype);
  userActivationKey.set(userActivation, Object.freeze({}));
  const secureContext = host.__tbSecureContext === true;
  class Navigator {
    get userAgent() {
      return identity.userAgent;
    }
    get appVersion() { return this.userAgent.replace(/^Mozilla\//, ''); }
    get appCodeName() { return 'Mozilla'; }
    get appName() { return 'Netscape'; }
    get product() { return 'Gecko'; }
    get productSub() { return '20030107'; }
    get vendor() { return 'Google Inc.'; }
    get vendorSub() { return ''; }
    get platform() { return identity.navigatorPlatform; }
    get language() { return 'en-US'; }
    get languages() { return Object.freeze(['en-US', 'en']); }
    get onLine() { return true; }
    get hardwareConcurrency() { return 1; }
    get maxTouchPoints() { return 0; }
    get cookieEnabled() { return true; }
    get webdriver() { return false; }
    get userActivation() {
      return userActivation;
    }
  }
  if (secureContext) {
    // Both are `[SecureContext]`
    // (<https://wicg.github.io/ua-client-hints/#dom-navigator-useragentdata>).
    // Web IDL names an attribute getter `get userAgentData`, and an
    // object-literal accessor is what makes the engine name it
    // (<https://webidl.spec.whatwg.org/#idl-attributes>).
    const { get } = Object.getOwnPropertyDescriptor({
      get userAgentData() { return userAgentData; },
    }, 'userAgentData');
    Object.defineProperty(Navigator.prototype, 'userAgentData', {
      get, enumerable: true, configurable: true,
    });
    Object.defineProperty(globalThis, 'NavigatorUAData', {
      value: NavigatorUAData, writable: true, configurable: true,
    });
  }
  Object.defineProperty(globalThis, 'UserActivation', {
    value: UserActivation, writable: true, configurable: true,
  });
  Object.defineProperty(globalThis, 'Navigator', {
    value: Navigator, writable: true, configurable: true,
  });
  Object.defineProperty(globalThis, 'navigator', {
    value: new Navigator(), writable: false, configurable: true,
  });
}
