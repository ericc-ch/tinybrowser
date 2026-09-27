(function() {
  const isOptionNode = globalThis.__tbIsOptionNode;
  const appendBlankOptions = globalThis.__tbAppendBlankOptions;
  delete globalThis.__tbIsOptionNode;
  delete globalThis.__tbAppendBlankOptions;
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
    value: function(name) {
      const key = String(name);
      if (key === '') return null;
      for (const element of this) {
        if (element.id === key || element.getAttribute('name') === key) return element;
      }
      return null;
    },
    writable: true, configurable: true,
  });
  Object.defineProperty(globalThis, 'HTMLCollection', { value: ctor, writable: true, configurable: true });
  // <https://html.spec.whatwg.org/multipage/common-dom-interfaces.html#htmloptionscollection>
  const optionsCtor = function() { throw new TypeError('Illegal constructor'); };
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
    // The spec does not cap indexed writes, but Blink caps list growth at
    // 100,000 options to avoid unbounded allocations.
    // (<https://html.spec.whatwg.org/multipage/common-dom-interfaces.html#htmloptionscollection>).
    if (index >= 100000 && index >= select.options.length) return;
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
      configurable: true,
    },
    add: {
      value: function(element, before) { optionOwners.get(this).add(element, before); },
      writable: true, configurable: true,
    },
    remove: {
      value: function(index) { optionOwners.get(this).remove(index); },
      writable: true, configurable: true,
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
          if (typeof inner.namedItem === 'function') {
            for (let index = 0; index < inner.length; index++) {
              const element = inner.item(index);
              for (const key of [element.id, element.getAttribute('name')]) {
                if (key && !keys.includes(key) && !Reflect.has(inner, key)) keys.push(key);
              }
            }
          }
          return keys;
        },
        getOwnPropertyDescriptor: function(inner, property) {
          if (typeof property === 'string' && canonicalIndex.test(property)) {
            const index = Number(property);
            if (index < inner.length) {
              return {
                value: inner.item(index),
                enumerable: true,
                configurable: true,
                writable: Object.getPrototypeOf(inner) === optionsProto,
              };
            }
          }
          if (typeof property === 'string' && !Reflect.has(inner, property)
              && typeof inner.namedItem === 'function') {
            const named = inner.namedItem(property);
            if (named !== null) {
              return { value: named, enumerable: false, configurable: true, writable: false };
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
})();
