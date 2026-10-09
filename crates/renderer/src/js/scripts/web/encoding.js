const __tbBlobData = host.slots('tinybrowser.blob.data');
const __tbFileData = host.slots('tinybrowser.file.data');
const __tbFileListData = host.slots('tinybrowser.filelist.data');
const __tbReaderData = host.slots('tinybrowser.filereader.data');
const __tbProgressData = host.slots('tinybrowser.progress.data');
const __tbDecoderData = host.slots('tinybrowser.textdecoder.data');
const __tbStreamData = host.slots('tinybrowser.readablestream.data');
const __tbToLatin1 = bytes => {
  // Chunked: per-byte `+=` is quadratic on large inputs (TextDecoder
  // pending, response bodies). 4096-arg applies stay inside the engine
  // call stack (<https://tc39.es/ecma262/#sec-apply>).
  const parts = __tbPrivateArray();
  for (let start = 0; start < bytes.length; start += 4096) {
    __tbArray.push(parts, __tbApply(String.fromCharCode, null, bytes.slice(start, start + 4096)));
  }
  return __tbArray.join(parts, '');
};
const __tbFromLatin1 = text => {
  const bytes = host.slots.bytes(text.length);
  for (let index = 0; index < text.length; index++) bytes[index] = text.charCodeAt(index);
  return bytes;
};
const __tbUtf8Encode = value => host.__tbUtf8Encode(value);
const __tbUtf8Decode = (bytes, fatal, ignoreBOM) => {
  const decoded = host.__tbDecode(__tbToLatin1(bytes), 'utf-8', fatal, ignoreBOM, true);
  return { text: decoded.text, remainder: __tbFromLatin1(decoded.remainder) };
};
const __tbEncoding = label => {
  const name = host.__tbEncodingName(String(label));
  return name === null || name === undefined ? null : String(name).toLowerCase();
};
const __tbDecodeBytes = (bytes, encoding, fatal, ignoreBOM) =>
  host.__tbDecode(__tbToLatin1(bytes), encoding, fatal, ignoreBOM, false).text;
// https://w3c.github.io/FileAPI/#convert-line-endings-to-native: LF is the
// native line ending on this platform, so CR and CRLF collapse to LF.
const __tbNativeEndings = value => String(value).replace(/\r\n?|\n/g, '\n');
// https://webidl.spec.whatwg.org/#Clamp: round to the nearest integer with
// ties going to the even integer.
const __tbClampRound = value => {
  value = Number(value);
  if (Number.isNaN(value) || value === 0) return 0;
  if (value === Infinity) return Number.MAX_SAFE_INTEGER;
  if (value === -Infinity) return Number.MIN_SAFE_INTEGER;
  const floor = Math.floor(value);
  const fraction = value - floor;
  if (fraction < 0.5) return floor;
  if (fraction > 0.5) return floor + 1;
  return floor % 2 === 0 ? floor : floor + 1;
};
globalThis.TextEncoder = __tbInstallInterface(class TextEncoder {
  constructor() {}
  get encoding() { return 'utf-8'; }
  encode(input) {
    return __tbUtf8Encode(input);
  }
  encodeInto(source, destination) {
    const length = __tbApply(__tbIDLTypedArrayLength, destination, []);
    const packed = host.__tbEncodeInto(source, length);
    const read = packed.read;
    const written = packed.written;
    const bytes = packed.bytes;
    for (let index = 0; index < written; index++) destination[index] = bytes[index];
    return { read, written };
  }
});
globalThis.TextDecoder = class TextDecoder {
  constructor(label, options) {
    const optionsObject = options === undefined ? {} : Object(options);
    const encoding = label === undefined ? 'utf-8' : __tbEncoding(String(label));
    if (encoding === null) throw new RangeError('The encoding label is not supported');
    __tbDecoderData.set(this, {
        encoding,
        fatal: optionsObject.fatal !== undefined && Boolean(optionsObject.fatal),
        ignoreBOM: optionsObject.ignoreBOM !== undefined && Boolean(optionsObject.ignoreBOM),
        pending: host.slots.bytes(0),
    });
  }
  get encoding() { return __tbBrand(this, __tbDecoderData).encoding; }
  get fatal() { return __tbBrand(this, __tbDecoderData).fatal; }
  get ignoreBOM() { return __tbBrand(this, __tbDecoderData).ignoreBOM; }
  decode(input, options) {
    const data = __tbBrand(this, __tbDecoderData);
    if (input !== undefined && input !== null) {
      let bytes;
      if (ArrayBuffer.isView(input)) bytes = new Uint8Array(input.buffer, input.byteOffset, input.byteLength);
      else if (input instanceof ArrayBuffer) bytes = new Uint8Array(input);
      else throw new TypeError('The input argument must be an ArrayBuffer or ArrayBufferView');
      if (bytes.length > 0) {
        const combined = host.slots.bytes(data.pending.length + bytes.length);
        for (let index = 0; index < data.pending.length; index++) combined[index] = data.pending[index];
        for (let index = 0; index < bytes.length; index++) combined[data.pending.length + index] = bytes[index];
        data.pending = combined;
      }
    }
    const stream = options !== undefined && options.stream === true;
    const decoded = host.__tbDecode(__tbToLatin1(data.pending), data.encoding, data.fatal, data.ignoreBOM, stream);
    data.pending = stream ? __tbFromLatin1(decoded.remainder) : host.slots.bytes(0);
    return decoded.text;
  }
};
Object.defineProperty(globalThis.TextDecoder.prototype, Symbol.toStringTag, { value: 'TextDecoder', writable: false, enumerable: false, configurable: true });
// https://html.spec.whatwg.org/multipage/webappapis.html#atob
const __tbBase64Table = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
const __tbBase64Encode = bytes => {
  let text = '';
  for (let index = 0; index < bytes.length; index += 3) {
    const first = bytes[index];
    const second = index + 1 < bytes.length ? bytes[index + 1] : null;
    const third = index + 2 < bytes.length ? bytes[index + 2] : null;
    text += __tbBase64Table[first >> 2];
    text += __tbBase64Table[((first & 3) << 4) | (second === null ? 0 : second >> 4)];
    text += second === null ? '=' : __tbBase64Table[((second & 15) << 2) | (third === null ? 0 : third >> 6)];
    text += third === null ? '=' : __tbBase64Table[third & 63];
  }
  return text;
};
globalThis.btoa = function(input) {
  input = String(input);
  for (let index = 0; index < input.length; index++) {
    if (input.charCodeAt(index) > 0xff) {
      throw new DOMException('The string to be encoded contains characters outside of the Latin1 range.', 'InvalidCharacterError');
    }
  }
  const bytes = new Uint8Array(input.length);
  for (let index = 0; index < input.length; index++) bytes[index] = input.charCodeAt(index);
  return __tbBase64Encode(bytes);
};
globalThis.atob = function(input) {
  const cleaned = String(input).replace(/[\t\n\f\r ]/g, '');
  // Forgiving-base64 strips padding only when the stripped length is a
  // multiple of four; `ab=` is an error, not `ab`
  // (<https://infra.spec.whatwg.org/#forgiving-base64-decode>).
  let body = cleaned;
  if (body.length % 4 === 0) {
    if (body.endsWith('==')) body = body.slice(0, -2);
    else if (body.endsWith('=')) body = body.slice(0, -1);
  }
  if (/[^A-Za-z0-9+/]/.test(body) || body.length % 4 === 1) {
    throw new DOMException('The string to be decoded is not correctly encoded.', 'InvalidCharacterError');
  }
  let text = '';
  let buffer = 0;
  let bits = 0;
  for (const character of body) {
    buffer = (buffer << 6) | __tbBase64Table.indexOf(character);
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      text += String.fromCharCode((buffer >> bits) & 0xff);
    }
  }
  return text;
};
