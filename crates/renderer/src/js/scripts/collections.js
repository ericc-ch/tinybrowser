(function() {
  const windowNamedValue = globalThis.__tbWindowNamedValue;
  const windowNamedHas = globalThis.__tbWindowNamedHas;
  delete globalThis.__tbWindowNamedValue;
  delete globalThis.__tbWindowNamedHas;
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
  // `HTMLOptionsCollection` inherits natively from `HTMLCollection`; the
  // remaining shim only preserves `SameObject` identity for the live
  // `select.options` and `select.selectedOptions` collections.
  for (const name of ['options', 'selectedOptions']) {
    const nativeGetter = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, name).get;
    const cache = new WeakMap();
    Object.defineProperty(HTMLSelectElement.prototype, name, {
      get: function() {
        let collection = cache.get(this);
        if (collection === undefined) {
          collection = nativeGetter.call(this);
          cache.set(this, collection);
        }
        return collection;
      },
      enumerable: true, configurable: true,
    });
  }
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
