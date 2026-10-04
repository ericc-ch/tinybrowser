const __tbIDLString = value => {
  // https://webidl.spec.whatwg.org/#es-DOMString
  if (typeof value === 'symbol') throw new TypeError('cannot convert a Symbol to DOMString');
  return String(value);
};
const __tbIDLCharCodeAt = String.prototype.charCodeAt;
const __tbIDLFromCharCode = String.fromCharCode;
const __tbIDLFromCodePoint = String.fromCodePoint;
const __tbIDLUSVString = value => {
  // https://webidl.spec.whatwg.org/#es-USVString
  const string = __tbIDLString(value);
  let result = '';
  for (let index = 0; index < string.length; index++) {
    const code = __tbApply(__tbIDLCharCodeAt, string, [index]);
    if (code >= 0xd800 && code <= 0xdbff) {
      const next = __tbApply(__tbIDLCharCodeAt, string, [index + 1]);
      if (next >= 0xdc00 && next <= 0xdfff) {
        result += string[index] + string[++index];
      } else {
        result += '\ufffd';
      }
    } else {
      result += code >= 0xdc00 && code <= 0xdfff ? '\ufffd' : string[index];
    }
  }
  return result;
};
const __tbIDLDescriptors = Object.getOwnPropertyDescriptors;
const __tbIDLDescriptor = Object.getOwnPropertyDescriptor;
const __tbIDLGet = Reflect.get;
const __tbIDLSetPrototypeOf = Object.setPrototypeOf;
const __tbIDLInstanceOf = Function.prototype[Symbol.hasInstance];
const __tbIDLTypedArray = Object.getPrototypeOf(Uint8Array.prototype);
const __tbIDLTypedArrayTag = __tbIDLDescriptor(__tbIDLTypedArray, Symbol.toStringTag).get;
const __tbIDLTypedArrayLength = __tbIDLDescriptor(__tbIDLTypedArray, 'length').get;
const __tbIDLTypedArrayBuffer = __tbIDLDescriptor(__tbIDLTypedArray, 'buffer').get;
const __tbIDLBufferLength = __tbIDLDescriptor(ArrayBuffer.prototype, 'byteLength').get;
const __tbIDLResizable = __tbIDLDescriptor(ArrayBuffer.prototype, 'resizable')?.get;
const __tbIDLSharedPrototype = globalThis.SharedArrayBuffer?.prototype;
const __tbIDLSharedLength = __tbIDLSharedPrototype && __tbIDLDescriptor(__tbIDLSharedPrototype, 'byteLength').get;
const __tbIDLGrowable = __tbIDLSharedPrototype && __tbIDLDescriptor(__tbIDLSharedPrototype, 'growable')?.get;
const __tbIDLUint8Array = (value, allowShared) => {
  // https://webidl.spec.whatwg.org/#es-buffer-source-types
  if (__tbApply(__tbIDLTypedArrayTag, value, []) !== 'Uint8Array') {
    throw new TypeError('value must be a Uint8Array');
  }
  const buffer = __tbApply(__tbIDLTypedArrayBuffer, value, []);
  let shared = false;
  try { __tbApply(__tbIDLBufferLength, buffer, []); }
  catch (error) {
    if (!allowShared || !__tbIDLSharedLength) throw error;
    __tbApply(__tbIDLSharedLength, buffer, []);
    shared = true;
  }
  if (shared) {
    if (__tbIDLGrowable && __tbApply(__tbIDLGrowable, buffer, [])) {
      throw new TypeError('growable buffers are not allowed');
    }
  } else if (__tbIDLResizable && __tbApply(__tbIDLResizable, buffer, [])) {
    throw new TypeError('resizable buffers are not allowed');
  }
  return value;
};
const __tbIDLRealmUint8Array = (value, allowShared, realm) => {
  // https://webidl.spec.whatwg.org/#es-buffer-source-types
  value = __tbIDLUint8Array(value, allowShared);
  if (__tbApply(__tbIDLInstanceOf, realm.Uint8Array, [value])) return value;
  const length = __tbApply(__tbIDLTypedArrayLength, value, []);
  const copy = new realm.Uint8Array(length);
  for (let index = 0; index < length; index++) copy[index] = value[index];
  return copy;
};
const __tbIDLResult = (value, type) => {
  if (typeof value !== type) throw new TypeError('algorithm result does not match IDL');
  return value;
};
const __tbIDLInteger = Number.isInteger;
const __tbIDLResultDictionary = (value, fields) => {
  // https://webidl.spec.whatwg.org/#es-to-js-dictionary
  const result = {};
  for (let index = 0; index < fields.length; index++) {
    const name = fields[index];
    const member = value[name];
    if (member === undefined) continue;
    if (!__tbIDLInteger(member) || member < 0 || member >= 2 ** 64) {
      throw new TypeError('algorithm result does not match unsigned long long');
    }
    __tbDefineProperty(result, name, {
      __proto__: null,
      value: member, writable: true, enumerable: true, configurable: true,
    });
  }
  return result;
};
const __tbInstallInterface = (implementation, name, construction, members) => {
  // https://webidl.spec.whatwg.org/#es-interface
  const brand = host.slots('webidl.' + name);
  const prototype = implementation.prototype;
  // The relevant realm of an instance is the realm that constructed it. Store
  // that realm's constructors on the brand so a cross-realm method call still
  // allocates results in the receiver's realm.
  const realm = { Uint8Array: __tbUint8Array };
  const descriptors = __tbIDLDescriptors(prototype);
  const required = (args, count) => {
    if (args.length < count) throw new TypeError(name + ': insufficient arguments');
  };
  const constructor = function(...args) {
    // https://webidl.spec.whatwg.org/#es-interface-call
    if (new.target === undefined || construction === null) throw new TypeError('Illegal constructor');
    required(args, construction[0]);
    const converted = construction[1](args);
    // OrdinaryCreateFromConstructor reads new.target's prototype once and
    // falls back to the interface prototype when it is not an object
    // (<https://webidl.spec.whatwg.org/#internally-create-a-new-object-implementing-the-interface>).
    const target = __tbIDLGet(new.target, 'prototype');
    const value = __tbConstruct(implementation, [converted], implementation);
    const chosen = target !== null && (typeof target === 'object' || typeof target === 'function')
      ? target
      : prototype;
    if (chosen !== prototype) __tbIDLSetPrototypeOf(value, chosen);
    brand.set(value, realm);
    return value;
  };
  __tbDefineProperty(constructor, 'name', { __proto__: null, value: name, configurable: true });
  __tbDefineProperty(constructor, 'length', { __proto__: null, value: construction === null ? 0 : construction[0], configurable: true });
  __tbDefineProperty(constructor, 'prototype', { __proto__: null, value: prototype, writable: false });
  __tbDefineProperty(prototype, 'constructor', { __proto__: null, value: constructor, writable: true, configurable: true });
  __tbDefineProperty(prototype, Symbol.toStringTag, { __proto__: null, value: name, configurable: true });
  for (let index = 0; index < members.length; index++) {
    const [member, kind, count, convert, result] = members[index];
    const descriptor = descriptors[member];
    const algorithm = kind === 'method' ? descriptor.value : descriptor[kind];
    const operation = {
      invoke(...args) {
        // https://webidl.spec.whatwg.org/#es-operations
        // https://webidl.spec.whatwg.org/#es-attributes
        if (!brand.has(this)) throw new TypeError('Illegal invocation');
        required(args, count);
        return result(__tbApply(algorithm, this, convert(args)), brand.get(this));
      },
    }.invoke;
    __tbDefineProperty(operation, 'name', { __proto__: null, value: kind === 'method' ? member : kind + ' ' + member, configurable: true });
    __tbDefineProperty(operation, 'length', { __proto__: null, value: count, configurable: true });
    const property = { __proto__: null, enumerable: true, configurable: true };
    if (kind === 'method') {
      property.value = operation;
      property.writable = true;
    } else {
      const prior = __tbIDLDescriptor(prototype, member);
      property.get = kind === 'get' ? operation : prior.get;
      property.set = kind === 'set' ? operation : prior.set;
    }
    __tbDefineProperty(prototype, member, property);
  }
  return constructor;
};
