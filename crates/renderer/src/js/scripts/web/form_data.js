// `FormData`: the entry list a form contributes, plus programmatic entries
// (<https://xhr.spec.whatwg.org/#interface-formdata>).
(function() {
  const lists = new WeakMap();

  function FormData(form) {
    if (!(this instanceof FormData)) {
      throw new TypeError('Class constructor FormData cannot be invoked without new');
    }
    const list = [];
    if (form !== undefined && form !== null) {
      const flat = globalThis.__tbFormEntries(form);
      for (let index = 0; index + 1 < flat.length; index += 2) {
        list.push([String(flat[index]), flat[index + 1]]);
      }
    }
    lists.set(this, list);
  }

  const entryList = receiver => {
    const list = lists.get(receiver);
    if (list === undefined) {
      throw new TypeError('Illegal invocation');
    }
    return list;
  };

  // A `Blob` value keeps its file name as the third argument; a string value
  // ignores it (<https://xhr.spec.whatwg.org/#dom-formdata-append>).
  const addValue = (list, name, value) => {
    list.push([String(name), value]);
  };

  Object.defineProperty(FormData.prototype, 'append', {
    value: function(name, value) {
      addValue(entryList(this), name, value);
    },
    writable: true, enumerable: true, configurable: true,
  });

  Object.defineProperty(FormData.prototype, 'delete', {
    value: function(name) {
      const key = String(name);
      const list = entryList(this);
      for (let index = list.length - 1; index >= 0; index--) {
        if (list[index][0] === key) list.splice(index, 1);
      }
    },
    writable: true, enumerable: true, configurable: true,
  });

  Object.defineProperty(FormData.prototype, 'get', {
    value: function(name) {
      const key = String(name);
      for (const entry of entryList(this)) {
        if (entry[0] === key) return entry[1];
      }
      return null;
    },
    writable: true, enumerable: true, configurable: true,
  });

  Object.defineProperty(FormData.prototype, 'getAll', {
    value: function(name) {
      const key = String(name);
      const values = [];
      for (const entry of entryList(this)) {
        if (entry[0] === key) values.push(entry[1]);
      }
      return values;
    },
    writable: true, enumerable: true, configurable: true,
  });

  Object.defineProperty(FormData.prototype, 'has', {
    value: function(name) {
      const key = String(name);
      return entryList(this).some(entry => entry[0] === key);
    },
    writable: true, enumerable: true, configurable: true,
  });

  Object.defineProperty(FormData.prototype, 'set', {
    value: function(name, value) {
      const key = String(name);
      const list = entryList(this);
      let replaced = false;
      for (let index = list.length - 1; index >= 0; index--) {
        if (list[index][0] !== key) continue;
        if (replaced) list.splice(index, 1);
        else { list[index] = [key, value]; replaced = true; }
      }
      if (!replaced) list.push([key, value]);
    },
    writable: true, enumerable: true, configurable: true,
  });

  Object.defineProperty(FormData.prototype, 'forEach', {
    value: function(callback, thisArg) {
      if (typeof callback !== 'function') {
        throw new TypeError('callback is not a function');
      }
      for (const entry of entryList(this).slice()) {
        callback.call(thisArg, entry[1], entry[0], this);
      }
    },
    writable: true, enumerable: true, configurable: true,
  });

  const iterator = (receiver, kind) => {
    const list = entryList(receiver).slice();
    let index = 0;
    const result = {
      next() {
        if (index >= list.length) return { value: undefined, done: true };
        const entry = list[index++];
        let value;
        if (kind === 'keys') value = entry[0];
        else if (kind === 'values') value = entry[1];
        else value = [entry[0], entry[1]];
        return { value: value, done: false };
      },
    };
    result[Symbol.iterator] = function() { return this; };
    return result;
  };

  for (const [name, kind] of [['entries', 'entries'], ['keys', 'keys'], ['values', 'values']]) {
    Object.defineProperty(FormData.prototype, name, {
      value: function() { return iterator(this, kind); },
      writable: true, enumerable: true, configurable: true,
    });
  }
  Object.defineProperty(FormData.prototype, Symbol.iterator, {
    value: FormData.prototype.entries,
    writable: true, enumerable: false, configurable: true,
  });
  Object.defineProperty(FormData.prototype, Symbol.toStringTag, {
    value: 'FormData', writable: false, enumerable: false, configurable: true,
  });
  Object.defineProperty(FormData, 'prototype', { value: FormData.prototype, writable: false });
  Object.defineProperty(globalThis, 'FormData', {
    value: FormData, writable: true, enumerable: false, configurable: true,
  });
})();
