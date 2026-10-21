// https://drafts.csswg.org/css-font-loading/#fontfaceset
// System fonts are always available, so the document's font set starts
// settled. `FontFace` objects register their fetched bytes through
// `__tbRegisterFont`, and `document.fonts.ready` waits for those loads.
const __tbFontFaces = new Set();

// https://drafts.csswg.org/css-font-loading/#fontface-interface
class FontFace {
  constructor(family, source, descriptors = {}) {
    if (arguments.length < 2) {
      throw new TypeError('FontFace needs a family and a source');
    }
    this.family = String(family);
    this._source = source;
    this._descriptors = descriptors;
    this.status = 'unloaded';
    this._registration = null;
  }

  get loaded() {
    return this._ensureLoad().then(() => this);
  }

  load() {
    return this.loaded;
  }

  _ensureLoad() {
    if (this._registration === null) {
      this._registration = this._register();
    }
    return this._registration;
  }

  _register() {
    this.status = 'loading';
    let url = null;
    if (typeof this._source === 'string') {
      const match = /url\(\s*(['"]?)([^'")]*)\1\s*\)/.exec(this._source);
      if (match) {
        url = match[2];
      }
    }
    if (!url) {
      // `local()` and binary sources need no network load.
      this.status = 'loaded';
      return Promise.resolve();
    }
    return fetch(url)
      .then((response) => response.arrayBuffer())
      .then((buffer) => {
        globalThis.__tbRegisterFont(
          this.family,
          Array.from(new Uint8Array(buffer)),
        );
        this.status = 'loaded';
      })
      .catch((error) => {
        this.status = 'error';
        throw error;
      });
  }
}

globalThis.FontFace = FontFace;

const __tbFontFaceSet = {
  status: 'loaded',
  onloading: null,
  onloadingdone: null,
  onloadingerror: null,
  get ready() {
    return Promise.all(Array.from(__tbFontFaces, (face) => face.loaded)).then(
      () => __tbFontFaceSet,
    );
  },
  check() { return true; },
  load() { return Promise.resolve([]); },
  add(face) {
    if (face instanceof FontFace) {
      __tbFontFaces.add(face);
      face._ensureLoad();
    }
  },
  delete(face) { return __tbFontFaces.delete(face); },
  clear() { __tbFontFaces.clear(); },
  forEach(callback, thisArg) {
    for (const face of __tbFontFaces) {
      callback.call(thisArg, face, face, __tbFontFaceSet);
    }
  },
};

if (globalThis.document !== undefined) {
  const prototype = Object.getPrototypeOf(document);
  // A realm can exist before its document is installed; timers.js then uses
  // a plain {} placeholder. Never define `fonts` on Object.prototype.
  if (prototype !== Object.prototype) {
    Object.defineProperty(prototype, 'fonts', {
      configurable: true,
      enumerable: false,
      get() { return __tbFontFaceSet; },
    });
  }
}
