// Web Crypto entropy and UUID generation. Algorithms remain absent until the
// engine can implement them faithfully.
// https://w3c.github.io/webcrypto/#Crypto-interface
{
  const Exception = DOMException;
  const QuotaExceeded = globalThis.QuotaExceededError;
  const hexDigits = '0123456789abcdef';
  // Both algorithms fill here, so neither consults a page method
  // (<https://w3c.github.io/webcrypto/#Crypto-method-getRandomValues>).
  const fill = bytes => {
    __tbApply(__tbIDLTypedArraySet, bytes, [
      host.__tbRandomBytes(__tbApply(__tbIDLTypedArrayByteLength, bytes, [])),
    ]);
  };
  const isIntegerType = tag => {
    switch (tag) {
      case 'Int8Array':
      case 'Uint8Array':
      case 'Uint8ClampedArray':
      case 'Int16Array':
      case 'Uint16Array':
      case 'Int32Array':
      case 'Uint32Array':
      case 'BigInt64Array':
      case 'BigUint64Array':
        return true;
      default:
        return false;
    }
  };
  // `Crypto` is not constructible; `crypto` is its one instance.
  const [Crypto, crypto] = __tbInstallInterface(class Crypto {
    getRandomValues(array) {
      // A float typed array or a `DataView` is an `ArrayBufferView`, so the
      // conversion accepts it and this step rejects it
      // (<https://w3c.github.io/webcrypto/#Crypto-method-getRandomValues>).
      if (!isIntegerType(__tbApply(__tbIDLTypedArrayTag, array, []))) {
        throw new Exception('destination must be an integer TypedArray', 'TypeMismatchError');
      }
      const length = __tbApply(__tbIDLTypedArrayByteLength, array, []);
      if (length > 65536) throw new QuotaExceeded('random byte quota exceeded');
      // Constructing the view also rejects a detached destination buffer.
      const bytes = new __tbUint8Array(
        __tbApply(__tbIDLTypedArrayBuffer, array, []),
        __tbApply(__tbIDLTypedArrayByteOffset, array, []),
        length,
      );
      fill(bytes);
      return array;
    }
    randomUUID() {
      const bytes = new __tbUint8Array(16);
      fill(bytes);
      // https://w3c.github.io/webcrypto/#Crypto-method-randomUUID
      bytes[6] = (bytes[6] & 0x0f) | 0x40;
      bytes[8] = (bytes[8] & 0x3f) | 0x80;
      let uuid = '';
      for (let index = 0; index < 16; index++) {
        if (index === 4 || index === 6 || index === 8 || index === 10) uuid += '-';
        uuid += hexDigits[bytes[index] >> 4] + hexDigits[bytes[index] & 0x0f];
      }
      return uuid;
    }
  }, ['subtle']);
  __tbDefineProperty(globalThis, 'Crypto', {
    __proto__: null, value: Crypto, writable: true, enumerable: false, configurable: true,
  });
  // `[SameObject] readonly attribute Crypto crypto`
  // (<https://w3c.github.io/webcrypto/#WindowOrWorkerGlobalScope-partial>).
  __tbDefineProperty(globalThis, 'crypto', {
    __proto__: null, get: () => crypto, enumerable: true, configurable: true,
  });
}
