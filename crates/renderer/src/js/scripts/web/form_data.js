// `FormData`: the entry list a form contributes, plus programmatic entries
// (<https://xhr.spec.whatwg.org/#interface-formdata>).
(function() {
  const lists = new WeakMap();

  function FormData(form, submitter) {
    if (!(this instanceof FormData)) {
      throw new TypeError('Class constructor FormData cannot be invoked without new');
    }
    const list = [];
    lists.set(this, list);
    if (form !== undefined && form !== null) {
      const flat = globalThis.__tbFormEntries(
        form, submitter === undefined || submitter === null ? null : submitter);
      for (let index = 0; index + 1 < flat.length; index += 2) {
        list.push([String(flat[index]), flat[index + 1]]);
      }
      // Constructing the entry list fires `formdata`, whose handler may extend
      // the list (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#constructing-form-data-set>).
      if (typeof globalThis.FormDataEvent === 'function') {
        form.dispatchEvent(new globalThis.FormDataEvent('formdata', {
          formData: this, bubbles: true, cancelable: false,
        }));
      }
    }
  }

  const entryList = receiver => {
    const list = lists.get(receiver);
    if (list === undefined) {
      throw new TypeError('Illegal invocation');
    }
    return list;
  };

  // A `Blob` value becomes a `File` named by the optional third argument; any
  // other value is stringified
  // (<https://xhr.spec.whatwg.org/#create-an-entry>).
  const createEntry = (value, filename) => {
    if (globalThis.Blob && value instanceof globalThis.Blob) {
      if (globalThis.File && value instanceof globalThis.File) return value;
      return new globalThis.File(
        [value],
        filename === undefined ? 'blob' : String(filename),
        { type: value.type },
      );
    }
    return String(value);
  };

  Object.defineProperty(FormData.prototype, 'append', {
    value: function(name, value, filename) {
      entryList(this).push([String(name), createEntry(value, filename)]);
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
    value: function(name, value, filename) {
      const key = String(name);
      const entry = [key, createEntry(value, filename)];
      const list = entryList(this);
      // Replace the first match and remove the rest
      // (<https://xhr.spec.whatwg.org/#dom-formdata-set>).
      const first = list.findIndex(item => item[0] === key);
      if (first === -1) {
        list.push(entry);
        return;
      }
      list[first] = entry;
      for (let index = list.length - 1; index > first; index--) {
        if (list[index][0] === key) list.splice(index, 1);
      }
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
