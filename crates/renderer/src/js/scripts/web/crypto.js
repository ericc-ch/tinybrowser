// Web Crypto entropy and UUID generation. Algorithms remain absent until the
// engine can implement them faithfully.
// https://w3c.github.io/webcrypto/#Crypto-interface
{
  const integerTypes = [
    'Int8Array', 'Uint8Array', 'Uint8ClampedArray', 'Int16Array', 'Uint16Array',
    'Int32Array', 'Uint32Array', 'BigInt64Array', 'BigUint64Array',
  ];
  const Crypto = __tbInstallInterface(class Crypto {
    getRandomValues(array) {
      // A float typed array is an `ArrayBufferView`, so the conversion accepts
      // it and this algorithm rejects it
      // (<https://w3c.github.io/webcrypto/#Crypto-method-getRandomValues>).
      if (!integerTypes.includes(__tbApply(__tbIDLTypedArrayTag, array, []))) {
        throw new DOMException('destination must be an integer TypedArray', 'TypeMismatchError');
      }
      if (array.byteLength > 65536) {
        // https://w3c.github.io/webcrypto/#Crypto-method-getRandomValues
        throw new globalThis.QuotaExceededError('random byte quota exceeded');
      }
      // Constructing this view also rejects a detached destination buffer.
      const bytes = new Uint8Array(array.buffer, array.byteOffset, array.byteLength);
      bytes.set(host.__tbRandomBytes(bytes.length));
      return array;
    }
    randomUUID() {
      const bytes = this.getRandomValues(new Uint8Array(16));
      bytes[6] = (bytes[6] & 0x0f) | 0x40;
      bytes[8] = (bytes[8] & 0x3f) | 0x80;
      const hex = Array.from(bytes, byte => byte.toString(16).padStart(2, '0'));
      return `${hex.slice(0, 4).join('')}-${hex.slice(4, 6).join('')}-${hex.slice(6, 8).join('')}-${hex.slice(8, 10).join('')}-${hex.slice(10).join('')}`;
    }
  });
  // `Crypto` is not constructible; `crypto` is its one instance.
  const crypto = __tbCreateInstance(Crypto);
  Object.defineProperty(globalThis, 'Crypto', {
    value: Crypto, writable: true, configurable: true,
  });
  Object.defineProperty(globalThis, 'crypto', {
    value: Object.freeze(crypto), writable: false, configurable: true,
  });
}
