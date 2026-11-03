// Navigator identity exposed by the browsing-context global.
// https://html.spec.whatwg.org/multipage/system-state.html#the-navigator-object
{
  const identity = host.__tbIdentity();
  const uaBrands = Object.freeze(identity.brands.map((entry) => Object.freeze(entry)));
  const uaFullVersionList = Object.freeze(identity.fullVersionList.map((entry) => Object.freeze(entry)));
  const uaMobile = identity.mobile;
  const uaPlatform = identity.platform;
  // `navigator.languages` is a FrozenArray of the identity's language list, in
  // preference order, and `language` is its first tag
  // https://html.spec.whatwg.org/multipage/system-state.html#dom-navigator-languages
  const languages = Object.freeze(identity.languages.slice());
  // `uaFullVersion` reports the engine version, so the Chromium brand wins; an
  // identity without one falls back to its last brand rather than to nothing
  // https://wicg.github.io/ua-client-hints/#getHighEntropyValues
  const chromium = uaFullVersionList.find((entry) => entry.brand === 'Chromium');
  const lastFullVersion = uaFullVersionList[uaFullVersionList.length - 1];
  const uaFullVersion = chromium !== undefined
    ? chromium.version
    : (lastFullVersion === undefined ? '' : lastFullVersion.version);

  // `NavigatorUAData` is generated from its IDL shape. Its global stays
  // conditional because tinybrowser exposes it only where `userAgentData`
  // itself is available; the installer's `__tbInstallGlobal` form is what a
  // `[SecureContext]` interface needs, and `NavigatorUAData` is not one.
  const [NavigatorUAData, userAgentData] = __tbInstallInterface(class NavigatorUAData {
    get brands() {
      return uaBrands;
    }
    get mobile() {
      return uaMobile;
    }
    get platform() {
      return uaPlatform;
    }
    // https://wicg.github.io/ua-client-hints/#getHighEntropyValues
    getHighEntropyValues(hints) {
      const values = { brands: uaBrands, mobile: uaMobile, platform: uaPlatform };
      if (hints.includes('architecture')) values.architecture = identity.architecture;
      if (hints.includes('bitness')) values.bitness = identity.bitness;
      if (hints.includes('formFactors')) values.formFactors = identity.formFactors.slice();
      if (hints.includes('fullVersionList')) values.fullVersionList = uaFullVersionList;
      if (hints.includes('model')) values.model = identity.model;
      if (hints.includes('platformVersion')) values.platformVersion = identity.platformVersion;
      if (hints.includes('uaFullVersion')) values.uaFullVersion = uaFullVersion;
      if (hints.includes('wow64')) values.wow64 = identity.wow64;
      return Promise.resolve(values);
    }
    // https://wicg.github.io/ua-client-hints/#h-tojson
    toJSON() {
      return { brands: uaBrands, mobile: uaMobile, platform: uaPlatform };
    }
  });

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

  // `Navigator` includes seven NavigatorID/Language/etc. mixins, so the
  // generator resolves `userAgentData` from `Navigator includes NavigatorUA`
  // and applies its `[SecureContext]` exposure per member.
  const [Navigator, navigator] = __tbInstallGlobal('Navigator', class Navigator {
    get userAgent() {
      return identity.userAgent;
    }
    get appVersion() { return this.userAgent.replace(/^Mozilla\//, ''); }
    get appCodeName() { return 'Mozilla'; }
    get appName() { return 'Netscape'; }
    get product() { return 'Gecko'; }
    get productSub() { return identity.productSub; }
    get vendor() { return identity.vendor; }
    get vendorSub() { return ''; }
    get platform() { return identity.navigatorPlatform; }
    get language() { return languages[0]; }
    get languages() { return languages; }
    get onLine() { return true; }
    get hardwareConcurrency() { return 1; }
    get maxTouchPoints() { return 0; }
    get cookieEnabled() { return true; }
    get webdriver() { return false; }
    get userActivation() {
      return userActivation;
    }
    get userAgentData() {
      return userAgentData;
    }
  }, [
    'adAuctionComponents', 'attribution', 'audioSession', 'bluetooth',
    'canShare', 'clearAppBadge', 'clipboard', 'connection', 'contacts',
    'cpuPerformance', 'createHandwritingRecognizer', 'credentials',
    'crossOriginStorage', 'deprecatedReplaceInURN', 'deprecatedURNtoURL',
    'deviceMemory', 'devicePosture', 'geolocation', 'getAutoplayPolicy',
    'getBattery', 'getGamepads', 'getInstalledRelatedApps',
    'globalPrivacyControl', 'gpu', 'hid', 'ink', 'install',
    'javaEnabled', 'keyboard', 'locks', 'login', 'managed',
    'mediaCapabilities', 'mediaDevices', 'mediaSession', 'mimeTypes', 'ml',
    'oscpu', 'pdfViewerEnabled', 'permissions', 'plugins', 'preferences',
    'presentation', 'queryHandwritingRecognizer',
    'registerProtocolHandler', 'requestMediaKeySystemAccess',
    'requestMIDIAccess', 'scheduling', 'sendBeacon', 'serial',
    'serviceWorker', 'setAppBadge', 'share', 'storage', 'storageBuckets',
    'taintEnabled', 'unregisterProtocolHandler', 'usb', 'vibrate',
    'virtualKeyboard', 'wakeLock', 'windowControlsOverlay', 'xr',
  ]);

  if (host.__tbSecureContext === true) {
    Object.defineProperty(globalThis, 'NavigatorUAData', {
      value: NavigatorUAData, writable: true, configurable: true,
    });
  }
  Object.defineProperty(globalThis, 'UserActivation', {
    value: UserActivation, writable: true, configurable: true,
  });
  Object.defineProperty(globalThis, 'navigator', {
    value: navigator, writable: false, configurable: true,
  });
}
