(function() {
  const isOptionNode = globalThis.__tbIsOptionNode;
  const appendBlankOptions = globalThis.__tbAppendBlankOptions;
  const collectionNamed = globalThis.__tbCollectionNamed;
  const collectionLength = globalThis.__tbCollectionLength;
  const windowNamedValue = globalThis.__tbWindowNamedValue;
  const windowNamedHas = globalThis.__tbWindowNamedHas;
  delete globalThis.__tbIsOptionNode;
  delete globalThis.__tbAppendBlankOptions;
  delete globalThis.__tbCollectionNamed;
  delete globalThis.__tbCollectionLength;
  delete globalThis.__tbWindowNamedValue;
  delete globalThis.__tbWindowNamedHas;
  const toUnsignedLong = value => {
    const number = +value;
    return Number.isFinite(number) ? ((Math.trunc(number) % 4294967296) + 4294967296) % 4294967296 : 0;
  };
  const native = globalThis.NodeList.prototype;
  function values() {
    let index = 0;
    const collection = this;
    return {
      next: function() {
        if (index >= collection.length) return { value: undefined, done: true };
        return { value: collection.item(index++), done: false };
      },
      [Symbol.iterator]: function() { return this; }
    };
  }
  Object.defineProperty(native, Symbol.iterator, {
    value: values,
    writable: true,
    configurable: true,
  });
  Object.defineProperty(globalThis.NamedNodeMap.prototype, Symbol.iterator, {
    value: values,
    writable: true,
    configurable: true,
  });
  Object.defineProperty(globalThis.DOMTokenList.prototype, Symbol.iterator, {
    value: values,
    writable: true,
    configurable: true,
  });
  for (const [collectionName, collectionProto] of [
    ['NodeList', native],
    ['NamedNodeMap', globalThis.NamedNodeMap.prototype],
    ['DOMTokenList', globalThis.DOMTokenList.prototype],
  ]) {
    Object.defineProperty(collectionProto, Symbol.toStringTag, {
      value: collectionName, writable: false, enumerable: false, configurable: true,
    });
  }
  const ctor = globalThis.HTMLCollection;
  const proto = ctor.prototype;
  Object.defineProperty(proto, Symbol.iterator, {
    value: values,
    writable: true,
    configurable: true,
  });
  Object.defineProperty(proto, Symbol.toStringTag, {
    value: 'HTMLCollection', writable: false, enumerable: false, configurable: true,
  });
  Object.defineProperty(proto, 'namedItem', {
    value: function(name) { return collectionNamed(this, String(name)); },
    writable: true, enumerable: true, configurable: true,
  });
  // <https://html.spec.whatwg.org/multipage/common-dom-interfaces.html#htmloptionscollection>
  const optionsCtor = globalThis.HTMLOptionsCollection;
  const optionsProto = optionsCtor.prototype;
  Object.setPrototypeOf(optionsProto, proto);
  Object.setPrototypeOf(optionsCtor, ctor);
  Object.defineProperty(optionsProto, Symbol.toStringTag, {
    value: 'HTMLOptionsCollection', configurable: true,
  });
  const optionOwners = new WeakMap();
  for (const name of ['options', 'selectedOptions']) {
    const nativeGetter = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, name).get;
    const cache = new WeakMap();
    Object.defineProperty(HTMLSelectElement.prototype, name, {
      get: function() {
        let collection = cache.get(this);
        if (collection === undefined) {
          collection = nativeGetter.call(this);
          cache.set(this, collection);
          if (name === 'options') optionOwners.set(collection, this);
        }
        return collection;
      },
      enumerable: true, configurable: true,
    });
  }
  const setOption = (select, index, option) => {
    if (option === null || option === undefined) {
      select.remove(index);
      return;
    }
    if (!isOptionNode(option)) {
      throw new TypeError('option must be an HTMLOptionElement');
    }
    // The spec does not cap indexed writes, but Blink refuses to grow the
    // list past 100,000 options to avoid unbounded allocations
    // (<https://html.spec.whatwg.org/multipage/common-dom-interfaces.html#htmloptionscollection>).
    if (index > select.options.length && index >= 100000) return;
    const existing = select.options.item(index);
    if (existing) {
      existing.parentNode.replaceChild(option, existing);
    } else {
      select.options.length = index;
      select.add(option);
    }
  };
  // Captured by Rust at install, then deleted from the global by Rust so page
  // script cannot call it directly. Configurable so the install can remove it.
  Object.defineProperty(globalThis, '__tbSetOption', {
    value: setOption, configurable: true, writable: false,
  });
  Object.defineProperties(optionsProto, {
    length: {
      get: function() { return collectionLength(this); },
      set: function(value) {
        const select = optionOwners.get(this);
        const length = toUnsignedLong(value);
        const current = this.length;
        // <https://html.spec.whatwg.org/multipage/common-dom-interfaces.html#dom-htmloptionscollection-length>
        if (length > current && length > 100000) return;
        if (length < current) {
          const removed = [];
          for (let index = length; index < current; index++) removed.push(this.item(index));
          for (const option of removed) if (option.parentNode) option.remove();
        } else if (length > current) {
          appendBlankOptions(select, length - current);
        }
      },
      configurable: true,
    },
    selectedIndex: {
      get: function() { return optionOwners.get(this).selectedIndex; },
      set: function(value) { optionOwners.get(this).selectedIndex = value; },
      enumerable: true, configurable: true,
    },
    add: {
      value: function(element, before) { optionOwners.get(this).add(element, before); },
      writable: true, enumerable: true, configurable: true,
    },
    remove: {
      value: function(index) {
        if (arguments.length === 0) throw new TypeError('remove requires an index');
        optionOwners.get(this).remove(index);
      },
      writable: true, enumerable: true, configurable: true,
    },
  });
  // NamedNodeMap exposes both indexed and named properties, and its own
  // property names are the indices followed by the qualified names
  // (<https://dom.spec.whatwg.org/#interface-namednodemap>).
  // NamedNodeMap exposes indexed and named properties as real own
  // properties; interface members and prototype methods always win.
  Object.defineProperty(globalThis, '__tb_refreshNamedNodeMap', {
    enumerable: false,
    configurable: false,
    writable: false,
    value: function(map, names, lowercaseOnly) {
      for (const key of Object.getOwnPropertyNames(map)) {
        delete map[key];
      }
      for (let i = 0; i < names.length; i++) {
        const attr = map.item(i);
        Object.defineProperty(map, String(i), {
          value: attr,
          enumerable: true,
          configurable: true,
          writable: false,
        });
        const name = names[i];
        const named = !lowercaseOnly || !/[A-Z]/.test(name);
        if (named && !(name in map)) {
          Object.defineProperty(map, name, {
            value: attr,
            enumerable: false,
            configurable: true,
            writable: false,
          });
        }
      }
    }
  });
  // Named access on the Window object
  // (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#named-access-on-the-window-object>):
  // a named properties object sits between the global and its prototype. It
  // exposes an element for a name, or an `HTMLCollection` when several named
  // objects share the name. The name test is an O(1) index lookup; only a
  // name that is present pays for the value scan.
  {
    const originalProto = Object.getPrototypeOf(globalThis);
    const target = Object.create(originalProto);
    Object.setPrototypeOf(globalThis, new Proxy(target, {
      get: function(inner, property, receiver) {
        if (typeof property === 'string' && !Reflect.has(inner, property)
            && windowNamedHas(property)) {
          const value = windowNamedValue(property);
          if (value !== undefined) return value;
        }
        return Reflect.get(inner, property, receiver);
      },
      has: function(inner, property) {
        if (typeof property === 'string' && !Reflect.has(inner, property)
            && windowNamedHas(property)) {
          return windowNamedValue(property) !== undefined;
        }
        return Reflect.has(inner, property);
      },
      getOwnPropertyDescriptor: function(inner, property) {
        if (typeof property === 'string' && !Reflect.has(inner, property)
            && windowNamedHas(property)) {
          const value = windowNamedValue(property);
          if (value !== undefined) {
            return { value, writable: true, enumerable: false, configurable: true };
          }
        }
        return Reflect.getOwnPropertyDescriptor(inner, property);
      },
      // WebIDL keeps the named properties object immutable: it rejects
      // defining and deleting properties, preventing extensions, and changing
      // the prototype
      // (<https://webidl.spec.whatwg.org/#named-properties-object>). Named
      // properties are not real own properties, so there is no ownKeys trap.
      defineProperty: function() { return false; },
      deleteProperty: function() { return false; },
      preventExtensions: function() { return false; },
      setPrototypeOf: function(inner, proto) {
        return proto === Reflect.getPrototypeOf(inner);
      },
    }));
  }
})();
