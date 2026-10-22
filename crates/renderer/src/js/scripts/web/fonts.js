// https://drafts.csswg.org/css-font-loading/#fontfaceset
// System fonts are always available, so the document's font set starts
// settled. `FontFace` objects register their bytes through
// `__tbRegisterFont` only while they are members of the set, and
// `document.fonts.ready` settles after every loading face finishes.
const __tbFontFaces = new Set();

function __tbFontError(message) {
  if (typeof DOMException === 'function') {
    return new DOMException(message, 'NetworkError');
  }
  return new Error(message);
}

// https://drafts.csswg.org/css-font-loading/#fontface-interface
class FontFace {
  constructor(family, source, descriptors = {}) {
    if (arguments.length < 2) {
      throw new TypeError('FontFace needs a family and a source');
    }
    this._family = String(family);
    this._descriptors = descriptors === null ? {} : descriptors;
    this._source = source;
    this._bytes = null;
    this._url = null;
    this._status = 'unloaded';
    this._loadPromise = null;
    this._token = 0;
    this._attached = false;
    this._loadedPromise = new Promise((resolve, reject) => {
      this._resolveLoaded = resolve;
      this._rejectLoaded = reject;
    });
    // Keep a rejection handled until something awaits `loaded`; the status and
    // the promise returned by `load()` still report the failure.
    this._loadedPromise.catch(() => {});
    this._parseSource();
  }

  get family() {
    return this._family;
  }

  get status() {
    return this._status;
  }

  get loaded() {
    // Reading `loaded` never starts a fetch. The promise is created by the
    // constructor and settles when `load()` or set membership does.
    return this._loadedPromise;
  }

  get weight() {
    return this._descriptors.weight ?? 'normal';
  }

  get style() {
    return this._descriptors.style ?? 'normal';
  }

  get stretch() {
    return this._descriptors.stretch ?? 'normal';
  }

  get unicodeRange() {
    return this._descriptors.unicodeRange ?? 'U+0-10FFFF';
  }

  get variant() {
    return this._descriptors.variant ?? 'normal';
  }

  get featureSettings() {
    return this._descriptors.featureSettings ?? 'normal';
  }

  get display() {
    return this._descriptors.display ?? 'auto';
  }

  get ascentOverride() {
    return this._descriptors.ascentOverride ?? 'normal';
  }

  get descentOverride() {
    return this._descriptors.descentOverride ?? 'normal';
  }

  get lineGapOverride() {
    return this._descriptors.lineGapOverride ?? 'normal';
  }

  _parseSource() {
    if (
      typeof ArrayBuffer === 'function' &&
      (this._source instanceof ArrayBuffer || ArrayBuffer.isView(this._source))
    ) {
      const view =
        this._source instanceof ArrayBuffer
          ? new Uint8Array(this._source)
          : new Uint8Array(
              this._source.buffer,
              this._source.byteOffset,
              this._source.byteLength,
            );
      this._bytes = Array.from(view);
      return;
    }
    const source = String(this._source);
    const match = /url\(\s*(['"]?)([^'")]*)\1\s*\)/.exec(source);
    if (match && match[2]) {
      this._url = match[2];
    }
  }

  load() {
    if (this._loadPromise !== null) {
      return this._loadPromise;
    }
    if (this._status === 'loaded' || this._status === 'error') {
      return this._loadedPromise;
    }
    this._status = 'loading';
    if (this._bytes !== null) {
      // Binary data needs no network round trip.
      this._loadPromise = Promise.resolve().then(() => {
        this._finishLoaded();
        return this;
      });
    } else if (this._url === null) {
      // `local()` and malformed sources describe an already-available face.
      this._loadPromise = Promise.resolve().then(() => {
        this._finishLoaded();
        return this;
      });
    } else {
      this._loadPromise = fetch(this._url)
        .then((response) => {
          if (!response.ok) {
            throw __tbFontError(
              `font fetch failed with status ${response.status}`,
            );
          }
          return response.arrayBuffer();
        })
        .then((buffer) => {
          this._bytes = Array.from(new Uint8Array(buffer));
          this._finishLoaded();
          return this;
        })
        .catch((error) => {
          this._fail(error);
          throw error;
        });
    }
    return this._loadPromise;
  }

  _finishLoaded() {
    if (this._status === 'error') {
      return;
    }
    this._status = 'loaded';
    this._resolveLoaded(this);
  }

  _fail(error) {
    if (this._status === 'error') {
      return;
    }
    this._status = 'error';
    this._rejectLoaded(error);
  }

  _register() {
    if (!this._attached || this._token !== 0 || this._bytes === null) {
      return;
    }
    this._token = globalThis.__tbRegisterFont(this._family, this._bytes);
  }

  _unregister() {
    if (this._token !== 0) {
      globalThis.__tbUnregisterFont(this._token);
      this._token = 0;
    }
  }

  _attach() {
    this._attached = true;
    // Set membership starts the load; registering the face waits for bytes.
    this.load().then(
      () => this._register(),
      () => {},
    );
  }

  _detach() {
    this._attached = false;
    this._unregister();
  }
}

globalThis.FontFace = FontFace;

const __tbFontFaceSet = {
  onloading: null,
  onloadingdone: null,
  onloadingerror: null,
  _ready: null,
  get status() {
    for (const face of __tbFontFaces) {
      if (face.status === 'loading') {
        return 'loading';
      }
    }
    return 'loaded';
  },
  get ready() {
    return this._ready;
  },
  check() {
    return true;
  },
  load(font, text) {
    const family = String(font).split(',')[0].trim().replace(/^['"]|['"]$/g, '');
    const faces = Array.from(__tbFontFaces).filter(
      (face) => face.family === family,
    );
    return Promise.all(faces.map((face) => face.load())).then(() => faces);
  },
  add(face) {
    if (!(face instanceof FontFace) || __tbFontFaces.has(face)) {
      return;
    }
    __tbFontFaces.add(face);
    face._attach();
    this._refreshReady();
  },
  delete(face) {
    const deleted = __tbFontFaces.delete(face);
    if (deleted) {
      face._detach();
      this._refreshReady();
    }
    return deleted;
  },
  clear() {
    for (const face of __tbFontFaces) {
      face._detach();
    }
    __tbFontFaces.clear();
    this._refreshReady();
  },
  forEach(callback, thisArg) {
    for (const face of __tbFontFaces) {
      callback.call(thisArg, face, face, __tbFontFaceSet);
    }
  },
  _refreshReady() {
    const faces = Array.from(__tbFontFaces);
    this._ready = Promise.allSettled(
      faces.map((face) => face.loaded),
    ).then(() => __tbFontFaceSet);
  },
};

__tbFontFaceSet._ready = Promise.resolve(__tbFontFaceSet);

if (globalThis.document !== undefined) {
  const prototype = Object.getPrototypeOf(document);
  // A realm can exist before its document is installed; timers.js then uses
  // a plain {} placeholder. Never define `fonts` on Object.prototype.
  if (prototype !== Object.prototype) {
    Object.defineProperty(prototype, 'fonts', {
      configurable: true,
      enumerable: false,
      get() {
        return __tbFontFaceSet;
      },
    });
  }
}
