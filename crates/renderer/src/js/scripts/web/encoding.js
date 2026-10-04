const __tbBlobData = host.slots('tinybrowser.blob.data');
const __tbFileData = host.slots('tinybrowser.file.data');
const __tbFileListData = host.slots('tinybrowser.filelist.data');
const __tbReaderData = host.slots('tinybrowser.filereader.data');
const __tbProgressData = host.slots('tinybrowser.progress.data');
const __tbDecoderData = host.slots('tinybrowser.textdecoder.data');
const __tbStreamData = host.slots('tinybrowser.readablestream.data');
// https://encoding.spec.whatwg.org/#utf-8-encoder, with lone surrogates
// replaced as the standard requires.
const __tbUtf8Encode = value => {
  value = __tbIDLString(value);
  const length = value.length;
  const bytes = __tbPrivateArray();
  for (let index = 0; index < length; index++) {
    let code = __tbApply(__tbIDLCharCodeAt, value, [index]);
    if (code >= 0xd800 && code <= 0xdbff && index + 1 < length) {
      const low = __tbApply(__tbIDLCharCodeAt, value, [index + 1]);
      if (low >= 0xdc00 && low <= 0xdfff) {
        code = 0x10000 + ((code - 0xd800) << 10) + (low - 0xdc00);
        index++;
      }
    }
    if (code >= 0xd800 && code <= 0xdfff) code = 0xfffd;
    if (code <= 0x7f) __tbArray.push(bytes, code);
    else if (code <= 0x7ff) __tbArray.push(bytes, 0xc0 | (code >> 6), 0x80 | (code & 0x3f));
    else if (code <= 0xffff) __tbArray.push(bytes, 0xe0 | (code >> 12), 0x80 | ((code >> 6) & 0x3f), 0x80 | (code & 0x3f));
    else __tbArray.push(bytes, 0xf0 | (code >> 18), 0x80 | ((code >> 12) & 0x3f), 0x80 | ((code >> 6) & 0x3f), 0x80 | (code & 0x3f));
  }
  return new __tbUint8Array(bytes);
};
// https://encoding.spec.whatwg.org/#utf-8-decoder. The trailing incomplete
// sequence is returned so a streaming TextDecoder can carry it forward.
const __tbUtf8Decode = (bytes, fatal, ignoreBOM) => {
  let text = '';
  let index = 0;
  if (!ignoreBOM && bytes.length >= 3 && bytes[0] === 0xef && bytes[1] === 0xbb && bytes[2] === 0xbf) index = 3;
  while (index < bytes.length) {
    const first = bytes[index];
    let needed = 0;
    let code = 0;
    if (first < 0x80) { text += __tbIDLFromCharCode(first); index++; continue; }
    if (first >= 0xc2 && first <= 0xdf) { needed = 1; code = first & 0x1f; }
    else if (first >= 0xe0 && first <= 0xef) { needed = 2; code = first & 0x0f; }
    else if (first >= 0xf0 && first <= 0xf4) { needed = 3; code = first & 0x07; }
    if (needed === 0) {
      if (fatal) throw new TypeError('The encoded data was not valid.');
      text += '\ufffd';
      index++;
      continue;
    }
    if (index + needed >= bytes.length) break;
    let valid = true;
    for (let offset = 1; offset <= needed; offset++) {
      const next = bytes[index + offset];
      if ((next & 0xc0) !== 0x80) { valid = false; break; }
      code = (code << 6) | (next & 0x3f);
    }
    if (valid) {
      const overlong = (needed === 1 && code < 0x80) || (needed === 2 && code < 0x800) || (needed === 3 && code < 0x10000);
      if (overlong || code > 0x10ffff || (code >= 0xd800 && code <= 0xdfff)) valid = false;
    }
    if (!valid) {
      if (fatal) throw new TypeError('The encoded data was not valid.');
      text += '\ufffd';
      index++;
      continue;
    }
    text += __tbIDLFromCodePoint(code);
    index += needed + 1;
  }
  return { text, remainder: __tbBytesCopy(bytes, index) };
};
// https://encoding.spec.whatwg.org/#utf-16le-decoder
const __tbUtf16Decode = (bytes, littleEndian, ignoreBOM) => {
  let start = 0;
  let swap = littleEndian;
  if (!ignoreBOM && bytes.length >= 2) {
    if (bytes[0] === 0xff && bytes[1] === 0xfe) { swap = true; start = 2; }
    else if (bytes[0] === 0xfe && bytes[1] === 0xff) { swap = false; start = 2; }
  }
  let text = '';
  for (let index = start; index + 1 < bytes.length; index += 2) {
    const code = swap ? bytes[index] | (bytes[index + 1] << 8) : (bytes[index] << 8) | bytes[index + 1];
    text += __tbIDLFromCharCode(code);
  }
  const consumed = bytes.length - ((bytes.length - start) & 1);
  return { text, remainder: __tbBytesCopy(bytes, consumed) };
};
// https://encoding.spec.whatwg.org/#windows-1252
const __tbWindows1252 = bytes => {
  const table = '\u20ac\u0081\u201a\u0192\u201e\u2026\u2020\u2021\u02c6\u2030\u0160\u2039\u0152\u008d\u017d\u008f\u0090\u2018\u2019\u201c\u201d\u2022\u2013\u2014\u02dc\u2122\u0161\u203a\u0153\u009d\u017e\u0178';
  let text = '';
  for (let index = 0; index < bytes.length; index++) {
    const byte = bytes[index];
    text += byte >= 0x80 && byte <= 0x9f ? table[byte - 0x80] : __tbIDLFromCharCode(byte);
  }
  return text;
};
const __tbEncoding = label => {
  label = String(label).trim().toLowerCase();
  if (label === 'utf-8' || label === 'utf8' || label === 'unicode-1-1-utf-8') return 'utf-8';
  if (label === 'utf-16' || label === 'utf-16le') return 'utf-16le';
  if (label === 'utf-16be') return 'utf-16be';
  if (label === 'windows-1252' || label === 'cp1252' || label === 'x-cp1252') return 'windows-1252';
  return null;
};
const __tbDecodeBytes = (bytes, encoding, fatal, ignoreBOM) => {
  if (encoding === 'utf-16le' || encoding === 'utf-16be') {
    return __tbUtf16Decode(bytes, encoding === 'utf-16le', ignoreBOM).text;
  }
  if (encoding === 'windows-1252') return __tbWindows1252(bytes);
  const decoded = __tbUtf8Decode(bytes, fatal, ignoreBOM);
  return decoded.text + (decoded.remainder.length ? '\ufffd' : '');
};
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
    // Encode code point by code point so `read` can stop at the last code
    // unit whose bytes fit in the destination
    // (<https://encoding.spec.whatwg.org/#dom-textencoder-encodeinto>).
    const length = __tbApply(__tbIDLTypedArrayLength, destination, []);
    let read = 0;
    let written = 0;
    while (read < source.length) {
      let code = __tbApply(__tbIDLCharCodeAt, source, [read]);
      let units = 1;
      if (code >= 0xd800 && code <= 0xdbff && read + 1 < source.length) {
        const low = __tbApply(__tbIDLCharCodeAt, source, [read + 1]);
        if (low >= 0xdc00 && low <= 0xdfff) {
          code = 0x10000 + ((code - 0xd800) << 10) + (low - 0xdc00);
          units = 2;
        }
      }
      if (code >= 0xd800 && code <= 0xdfff) code = 0xfffd;
      const size = code <= 0x7f ? 1 : code <= 0x7ff ? 2 : code <= 0xffff ? 3 : 4;
      if (written + size > length) break;
      if (size === 1) {
        destination[written++] = code;
      } else if (size === 2) {
        destination[written++] = 0xc0 | (code >> 6);
        destination[written++] = 0x80 | (code & 0x3f);
      } else if (size === 3) {
        destination[written++] = 0xe0 | (code >> 12);
        destination[written++] = 0x80 | ((code >> 6) & 0x3f);
        destination[written++] = 0x80 | (code & 0x3f);
      } else {
        destination[written++] = 0xf0 | (code >> 18);
        destination[written++] = 0x80 | ((code >> 12) & 0x3f);
        destination[written++] = 0x80 | ((code >> 6) & 0x3f);
        destination[written++] = 0x80 | (code & 0x3f);
      }
      read += units;
    }
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
    let text;
    if (data.encoding === 'utf-8') {
      const decoded = __tbUtf8Decode(data.pending, data.fatal, data.ignoreBOM);
      data.pending = stream ? host.slots.bytes(decoded.remainder) : host.slots.bytes(0);
      if (!stream && decoded.remainder.length) {
        // A truncated tail is a decode error; fatal turns it into a throw,
        // otherwise it is one replacement character
        // (<https://encoding.spec.whatwg.org/#utf-8-decoder>).
        if (data.fatal) throw new TypeError('The encoded data was not valid.');
        text = decoded.text + '\ufffd';
      } else {
        text = decoded.text;
      }
    } else if (data.encoding === 'windows-1252') {
      text = __tbWindows1252(data.pending);
      data.pending = host.slots.bytes(0);
    } else {
      const decoded = __tbUtf16Decode(data.pending, data.encoding === 'utf-16le', data.ignoreBOM);
      data.pending = stream ? host.slots.bytes(decoded.remainder) : host.slots.bytes(0);
      text = decoded.text;
    }
    return text;
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
