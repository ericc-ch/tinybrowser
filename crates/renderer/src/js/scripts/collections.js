(function() {
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
  Object.defineProperty(globalThis, 'HTMLCollection', { value: ctor, writable: true, configurable: true });
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
      return new Proxy(target, {
        get: function(inner, property) {
          if (typeof property === 'string' && /^(0|[1-9][0-9]*)$/.test(property)) {
            const index = Number(property);
            return index < inner.length ? inner.item(index) : undefined;
          }
          const value = Reflect.get(inner, property, inner);
          if (typeof value === 'function' && isPlatformMethod(inner, property)) {
            return value.bind(inner);
          }
          return value;
        },
        has: function(inner, property) {
          if (typeof property === 'string' && /^(0|[1-9][0-9]*)$/.test(property)) {
            return Number(property) < inner.length;
          }
          return Reflect.has(inner, property);
        },
        ownKeys: function(inner) {
          const keys = [];
          for (let i = 0; i < inner.length; i++) {
            keys.push(String(i));
          }
          return keys;
        },
        getOwnPropertyDescriptor: function(inner, property) {
          if (typeof property === 'string' && /^(0|[1-9][0-9]*)$/.test(property)) {
            const index = Number(property);
            if (index < inner.length) {
              return {
                value: inner.item(index),
                enumerable: true,
                configurable: true,
                writable: false,
              };
            }
          }
          return Reflect.getOwnPropertyDescriptor(inner, property);
        },
        set: function(inner, property, value) {
          if (typeof property === 'string' && /^(0|[1-9][0-9]*)$/.test(property)) {
            return true;
          }
          return Reflect.set(inner, property, value, inner);
        }
      });
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
