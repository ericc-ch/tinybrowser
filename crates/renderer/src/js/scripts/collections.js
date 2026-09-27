(function() {
  const isOptionNode = globalThis.__tbIsOptionNode;
  const appendBlankOptions = globalThis.__tbAppendBlankOptions;
  const collectionNamed = globalThis.__tbCollectionNamed;
  const collectionKeys = globalThis.__tbCollectionKeys;
  const windowNamedValue = globalThis.__tbWindowNamedValue;
  const windowNamedHas = globalThis.__tbWindowNamedHas;
  delete globalThis.__tbIsOptionNode;
  delete globalThis.__tbAppendBlankOptions;
  delete globalThis.__tbCollectionNamed;
  delete globalThis.__tbCollectionKeys;
  delete globalThis.__tbWindowNamedValue;
  delete globalThis.__tbWindowNamedHas;
  const canonicalIndex = /^(0|[1-9][0-9]*)$/;
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
  ]) {
    Object.defineProperty(collectionProto, Symbol.toStringTag, {
      value: collectionName, writable: false, enumerable: false, configurable: true,
    });
  }
  const ctor = function() { throw new TypeError('Illegal constructor'); };
  Object.defineProperty(ctor, 'name', { value: 'HTMLCollection', configurable: true });
  const proto = Object.create(Object.prototype);
  for (const member of ['length', 'item']) {
    const descriptor = Object.getOwnPropertyDescriptor(native, member);
    if (descriptor) Object.defineProperty(proto, member, descriptor);
  }
  Object.defineProperty(proto, 'constructor', { value: ctor, writable: true, configurable: true });
  Object.defineProperty(proto, Symbol.iterator, {
    value: values,
    writable: true,
    configurable: true,
  });
  Object.defineProperty(ctor, 'prototype', { value: proto, writable: false });
  Object.defineProperty(proto, Symbol.toStringTag, {
    value: 'HTMLCollection', writable: false, enumerable: false, configurable: true,
  });
  Object.defineProperty(proto, 'namedItem', {
    value: function(name) { return collectionNamed(this, String(name)); },
    writable: true, enumerable: true, configurable: true,
  });
  Object.defineProperty(globalThis, 'HTMLCollection', { value: ctor, writable: true, configurable: true });
  // <https://html.spec.whatwg.org/multipage/common-dom-interfaces.html#htmloptionscollection>
  const optionsCtor = function() { throw new TypeError('Illegal constructor'); };
  Object.defineProperty(optionsCtor, 'name', { value: 'HTMLOptionsCollection', configurable: true });
  const optionsProto = Object.create(proto);
  Object.defineProperty(optionsProto, 'constructor', {
    value: optionsCtor, writable: true, configurable: true,
  });
  Object.defineProperty(optionsCtor, 'prototype', { value: optionsProto, writable: false });
  Object.setPrototypeOf(optionsCtor, ctor);
  Object.defineProperty(optionsProto, Symbol.toStringTag, {
    value: 'HTMLOptionsCollection', configurable: true,
  });
  Object.defineProperty(globalThis, 'HTMLOptionsCollection', {
    value: optionsCtor, writable: true, configurable: true,
  });
  const collectionTargets = new WeakMap();
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
          if (name === 'options') optionOwners.set(collectionTargets.get(collection), this);
        }
        return collection;
      },
      enumerable: true, configurable: true,
    });
  }
  const toUnsignedLong = value => {
    const number = +value;
    return Number.isFinite(number) ? ((Math.trunc(number) % 4294967296) + 4294967296) % 4294967296 : 0;
  };
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
  const baseLength = Object.getOwnPropertyDescriptor(proto, 'length').get;
  Object.defineProperties(optionsProto, {
    length: {
      get: function() { return baseLength.call(this); },
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
  Object.defineProperty(globalThis, '__tb_liveCollection', {
    enumerable: false,
    configurable: false,
    writable: false,
    value: function(target) {
      // Only platform methods need binding to the target; Object.prototype
      // built-ins like hasOwnProperty must see the proxy as `this`.
      function isPlatformMethod(inner, property) {
        let proto = Object.getPrototypeOf(inner);
        while (proto && proto !== Object.prototype) {
          if (Object.prototype.hasOwnProperty.call(proto, property)) {
            return true;
          }
          proto = Object.getPrototypeOf(proto);
        }
        return false;
      }
      const collection = new Proxy(target, {
        get: function(inner, property) {
          if (typeof property === 'string' && canonicalIndex.test(property)) {
            const index = Number(property);
            return index < inner.length ? inner.item(index) : undefined;
          }
          if (typeof property === 'string' && !Reflect.has(inner, property)
              && typeof inner.namedItem === 'function') {
            const named = inner.namedItem(property);
            if (named !== null) return named;
          }
          const value = Reflect.get(inner, property, inner);
          if (property !== 'constructor' && typeof value === 'function' && isPlatformMethod(inner, property)) {
            return value.bind(inner);
          }
          return value;
        },
        has: function(inner, property) {
          if (typeof property === 'string' && canonicalIndex.test(property)) {
            return Number(property) < inner.length;
          }
          if (typeof property === 'string' && !Reflect.has(inner, property)
              && typeof inner.namedItem === 'function' && inner.namedItem(property) !== null) return true;
          return Reflect.has(inner, property);
        },
        ownKeys: function(inner) {
          const keys = [];
          for (let i = 0; i < inner.length; i++) {
            keys.push(String(i));
          }
          const seen = new Set(keys);
          for (const key of collectionKeys(inner)) {
            if (!seen.has(key)) {
              seen.add(key);
              keys.push(key);
            }
          }
          return keys;
        },
        getOwnPropertyDescriptor: function(inner, property) {
          const options = Object.getPrototypeOf(inner) === optionsProto;
          if (typeof property === 'string' && canonicalIndex.test(property)) {
            const index = Number(property);
            if (index < inner.length) {
              return {
                value: inner.item(index),
                enumerable: true,
                configurable: true,
                writable: options,
              };
            }
          }
          if (typeof property === 'string' && !Reflect.has(inner, property)
              && typeof inner.namedItem === 'function') {
            const named = inner.namedItem(property);
            if (named !== null) {
              // HTMLCollection is `[LegacyUnenumerableNamedProperties]`;
              // HTMLOptionsCollection is not, so its names are enumerable
              // (<https://webidl.spec.whatwg.org/#LegacyUnenumerableNamedProperties>).
              return { value: named, enumerable: options, configurable: true, writable: false };
            }
          }
          return Reflect.getOwnPropertyDescriptor(inner, property);
        },
        set: function(inner, property, value) {
          if (typeof property === 'string' && canonicalIndex.test(property)) {
            const index = Number(property);
            if (Object.getPrototypeOf(inner) === optionsProto && index < 4294967295) {
              setOption(optionOwners.get(inner), index, value);
            }
            return true;
          }
          return Reflect.set(inner, property, value, inner);
        }
      });
      collectionTargets.set(collection, target);
      return collection;
    }
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
