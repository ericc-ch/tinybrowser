// https://html.spec.whatwg.org/multipage/web-messaging.html#messageevent
const __tbMessageEventData = Symbol.for('tinybrowser.messageevent.data');
globalThis.MessageEvent = class MessageEvent extends Event {
  constructor(type, init) {
    const eventInit = init === undefined ? {} : Object(init);
    super(String(type), eventInit);
    Object.defineProperty(this, __tbMessageEventData, {
      value: {
        data: Object.prototype.hasOwnProperty.call(eventInit, 'data') ? eventInit.data : null,
        origin: eventInit.origin === undefined ? '' : String(eventInit.origin),
        lastEventId: eventInit.lastEventId === undefined ? '' : String(eventInit.lastEventId),
        source: eventInit.source === undefined ? null : eventInit.source,
        ports: eventInit.ports === undefined ? Object.freeze([]) : Object.freeze(Array.from(eventInit.ports)),
      },
      writable: false, enumerable: false, configurable: false,
    });
  }
  get data() { return __tbBrand(this, __tbMessageEventData).data; }
  get origin() { return __tbBrand(this, __tbMessageEventData).origin; }
  get lastEventId() { return __tbBrand(this, __tbMessageEventData).lastEventId; }
  get source() { return __tbBrand(this, __tbMessageEventData).source; }
  get ports() { return __tbBrand(this, __tbMessageEventData).ports; }
};
Object.defineProperty(globalThis.MessageEvent.prototype, Symbol.toStringTag, { value: 'MessageEvent', writable: false, enumerable: false, configurable: true });
// ── cross-realm structured serialization ───────────────────────────────
// Values cannot cross realms, so the sender's realm encodes a message into a
// versioned payload string and the receiver's realm decodes it
// (<https://html.spec.whatwg.org/multipage/structured-data.html#structuredserializeinternal>).
// Rust carries payloads between frames, owns the frame tree, and owns the
// channel endpoints, so a port survives being transferred to another realm.
const __tbPayloadVersion = 'tb1:';
const __tbPlatform = Symbol.for('tinybrowser.platform');
const __tbWindowProxyData = Symbol.for('tinybrowser.windowproxy.data');
const __tbPortData = Symbol.for('tinybrowser.messageport.data');
const __tbFrameId = globalThis.__tb_frameId;
const __tbFrameProxies = Object.create(null);
// Writes made through a proxy whose target realm does not exist yet: a
// browsing context registered inside a script gets its realm at the next
// non-JS turn, and the engine then flushes these through
// `__tbFlushFrameSets` (<https://html.spec.whatwg.org/multipage/window-object.html#windowproxy-set>).
const __tbFramePendingSets = Object.create(null);
const __tbFramePending = frame => {
  let pending = __tbFramePendingSets[frame];
  if (pending === undefined) {
    pending = Object.create(null);
    __tbFramePendingSets[frame] = pending;
  }
  return pending;
};
globalThis.__tbFlushFrameSets = function(frame) {
  const pending = __tbFramePendingSets[frame];
  if (pending === undefined) return;
  delete __tbFramePendingSets[frame];
  const target = __tbFrameGlobal(frame);
  if (target == null) return;
  for (const key of Reflect.ownKeys(pending)) {
    if (key === '__proto__' || typeof key === 'symbol') {
      // `CreateDataProperty`: a `__proto__` write must not walk the prototype
      // setter, and symbol keys need `defineProperty` to become own data
      // properties.
      Object.defineProperty(target, key, {
        value: pending[key], writable: true, enumerable: true, configurable: true,
      });
    } else {
      target[key] = pending[key];
    }
  }
};

const __tbCloneFailure = () => new globalThis.DOMException('The object could not be cloned.', 'DataCloneError');
const __tbBase64Bytes = text => {
  const binary = globalThis.atob(text);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index++) bytes[index] = binary.charCodeAt(index);
  return bytes;
};
// Cross-realm brand probes: an intrinsic from this realm accepts an object
// from any realm, and throws when the internal slot is missing.
const __tbProbe = (read, value) => {
  try { read(value); return true; } catch (error) { return false; }
};
const __tbIsDate = value => __tbProbe(candidate => Date.prototype.getTime.call(candidate), value);
const __tbIsRegExp = value => __tbProbe(candidate => Object.getOwnPropertyDescriptor(RegExp.prototype, 'source').get.call(candidate), value);
const __tbIsMap = value => __tbProbe(candidate => Object.getOwnPropertyDescriptor(Map.prototype, 'size').get.call(candidate), value);
const __tbIsSet = value => __tbProbe(candidate => Object.getOwnPropertyDescriptor(Set.prototype, 'size').get.call(candidate), value);
const __tbIsArrayBuffer = value => __tbProbe(candidate => Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, 'byteLength').get.call(candidate), value);
const __tbIsDataView = value => __tbProbe(candidate => Object.getOwnPropertyDescriptor(DataView.prototype, 'byteLength').get.call(candidate), value);
const __tbTypedArrayPrototype = Object.getPrototypeOf(Uint8Array.prototype);
const __tbIsTypedArray = value => __tbProbe(candidate => Object.getOwnPropertyDescriptor(__tbTypedArrayPrototype, 'length').get.call(candidate), value);
const __tbIsBoxedBoolean = value => __tbProbe(candidate => Boolean.prototype.valueOf.call(candidate), value);
const __tbIsBoxedNumber = value => __tbProbe(candidate => Number.prototype.valueOf.call(candidate), value);
const __tbIsBoxedString = value => __tbProbe(candidate => String.prototype.valueOf.call(candidate), value);
// A real error (from any realm) answers `[object Error]` through its internal
// slot; a plain object can only do so by defining `Symbol.toStringTag`, which
// real errors never do
// (<https://html.spec.whatwg.org/multipage/structured-data.html#structuredserializeinternal>).
const __tbIsError = value => Object.prototype.toString.call(value) === '[object Error]'
  && value[Symbol.toStringTag] === undefined;
// Types with internal slots the serializer cannot reproduce. Weak maps and
// sets answer their own brand probes; promises and weak refs expose a tag and
// have no non-destructive brand probe.
const __tbIsWeakMap = value => __tbProbe(candidate => WeakMap.prototype.has.call(candidate, candidate), value);
const __tbIsWeakSet = value => __tbProbe(candidate => WeakSet.prototype.has.call(candidate, candidate), value);
const __tbIsPromise = value => typeof Promise === 'function' && Object.prototype.toString.call(value) === '[object Promise]';
const __tbIsWeakRef = value => typeof WeakRef === 'function' && Object.prototype.toString.call(value) === '[object WeakRef]';
const __tbHasInternalSlots = value => __tbIsPromise(value) || __tbIsWeakMap(value)
  || __tbIsWeakSet(value) || __tbIsWeakRef(value);

// `[[NumberData]]` specials have no JSON spelling.
const __tbNumberWire = value => {
  if (value !== value) return 'NaN';
  if (value === Infinity) return 'Infinity';
  if (value === -Infinity) return '-Infinity';
  if (value === 0 && 1 / value === -Infinity) return '-0';
  return value;
};
const __tbNumberValue = wire => wire === 'NaN' ? NaN
  : wire === 'Infinity' ? Infinity
  : wire === '-Infinity' ? -Infinity
  : wire === '-0' ? -0
  : wire;

// https://webidl.spec.whatwg.org/#es-sequence
const __tbTransferList = transfer => {
  if (transfer === undefined) return [];
  if (transfer === null || typeof transfer !== 'object' || typeof transfer[Symbol.iterator] !== 'function') {
    throw new TypeError('The transfer list must be an iterable object');
  }
  return Array.from(transfer);
};

// Serializes `value` into { payload, ports }, detaching every transferred
// buffer and endpoint as it does so
// (<https://html.spec.whatwg.org/multipage/structured-data.html#structuredserializewithtransfer>).
const __tbEncode = (value, transfer, sourcePort) => {
  // Validate the transfer list first: nothing is transferred until the value
  // graph serializes, because transferring has side effects and
  // StructuredSerializeInternal must be able to throw first
  // (<https://html.spec.whatwg.org/multipage/structured-data.html#structuredserializewithtransfer>).
  const buffers = new Map();     // ArrayBuffer -> node slot, or -1 once listed
  const transferred = new Map(); // MessagePort -> endpoint id
  for (const item of __tbTransferList(transfer)) {
    if (item instanceof ArrayBuffer) {
      if (buffers.has(item)) throw new globalThis.DOMException('Transfer list contains duplicate buffers', 'DataCloneError');
      if (typeof item.transfer !== 'function') throw new globalThis.DOMException('The buffer is not transferable', 'DataCloneError');
      buffers.set(item, -1);
    } else if (item instanceof globalThis.MessagePort) {
      if (item === sourcePort) throw new globalThis.DOMException('Cannot transfer the source port', 'DataCloneError');
      const itemData = __tbBrand(item, __tbPortData);
      if (itemData.closed) throw new globalThis.DOMException('Cannot transfer a detached MessagePort', 'DataCloneError');
      if (transferred.has(item)) throw new globalThis.DOMException('Transfer list contains duplicate ports', 'DataCloneError');
      transferred.set(item, itemData.id);
    } else {
      throw new globalThis.DOMException('Value not transferable', 'DataCloneError');
    }
  }
  const nodes = [];
  const seen = new Map();
  const slot = descriptor => {
    const index = nodes.length;
    nodes.push(descriptor);
    return index;
  };
  const encode = input => {
    if (input === null) return slot(['null']);
    const kind = typeof input;
    if (kind === 'undefined') return slot(['undefined']);
    if (kind === 'boolean') return slot(['boolean', input]);
    if (kind === 'number') return slot(['number', __tbNumberWire(input)]);
    if (kind === 'string') return slot(['string', input]);
    if (kind === 'bigint') return slot(['bigint', String(input)]);
    if (kind === 'function' || kind === 'symbol') throw __tbCloneFailure();
    if (seen.has(input)) return seen.get(input);
    // Window proxies are never serializable
    // (<https://html.spec.whatwg.org/multipage/structured-data.html#structuredserializeinternal>).
    if (input[__tbWindowProxyData] !== undefined) throw __tbCloneFailure();
    if (transferred.has(input)) {
      const index = slot(['port', transferred.get(input)]);
      seen.set(input, index);
      return index;
    }
    if (buffers.has(input)) {
      // The bytes are captured by the transfer step, after the graph is done.
      const index = slot(null);
      seen.set(input, index);
      buffers.set(input, index);
      return index;
    }
    if (input[__tbBlobData] !== undefined) {
      const blob = input[__tbBlobData];
      const bytes = __tbBase64Encode(new Uint8Array(blob.bytes));
      let descriptor;
      if (input[__tbFileData] !== undefined) {
        const file = input[__tbFileData];
        descriptor = ['file', file.name, file.lastModified, blob.type, bytes];
      } else {
        descriptor = ['blob', blob.type, bytes];
      }
      const index = slot(descriptor);
      seen.set(input, index);
      return index;
    }
    if (__tbIsArrayBuffer(input)) {
      let view;
      try {
        view = new Uint8Array(input);
      } catch (error) {
        // A detached buffer cannot be copied; the spec reports it as a clone
        // failure, not a raw TypeError.
        throw __tbCloneFailure();
      }
      const index = slot(['buffer', __tbBase64Encode(view)]);
      seen.set(input, index);
      return index;
    }
    if (typeof SharedArrayBuffer === 'function' && input instanceof SharedArrayBuffer) {
      // Shared memory would need to stay shared across realms; the engine
      // has no shared-memory transport, so refuse instead of corrupting.
      throw __tbCloneFailure();
    }
    if (__tbIsTypedArray(input) || __tbIsDataView(input)) {
      const buffer = encode(input.buffer);
      const descriptor = __tbIsDataView(input)
        ? ['view', buffer, 'DataView', input.byteOffset, input.byteLength]
        : ['view', buffer, input.constructor.name, input.byteOffset, input.length];
      const index = slot(descriptor);
      seen.set(input, index);
      return index;
    }
    // DOMException is serializable (name and message survive)
    // (<https://html.spec.whatwg.org/multipage/structured-data.html#serializable-objects>).
    if (Object.prototype.toString.call(input) === '[object DOMException]') {
      return slot(['domexception', String(input.name), String(input.message)]);
    }
    // Platform objects (nodes, events, ports, ...) are not serializable.
    if (input[__tbPlatform] !== undefined || input[__tbPortData] !== undefined) throw __tbCloneFailure();
    // Objects with internal slots the serializer cannot reproduce must fail,
    // not silently clone as plain objects
    // (<https://html.spec.whatwg.org/multipage/structured-data.html#structuredserializeinternal>).
    if (__tbHasInternalSlots(input)) throw __tbCloneFailure();
    if (__tbIsDate(input)) return slot(['date', __tbNumberWire(input.getTime())]);
    if (__tbIsRegExp(input)) return slot(['regexp', input.source, input.flags]);
    if (__tbIsError(input)) {
      const index = slot(null);
      seen.set(input, index);
      nodes[index] = ['error', String(input.name), String(input.message)];
      return index;
    }
    if (__tbIsBoxedBoolean(input)) return slot(['boxed', 'Boolean', input.valueOf()]);
    if (__tbIsBoxedNumber(input)) return slot(['boxed', 'Number', __tbNumberWire(input.valueOf())]);
    if (__tbIsBoxedString(input)) return slot(['boxed', 'String', input.valueOf()]);
    if (__tbIsMap(input)) {
      const index = slot(null);
      seen.set(input, index);
      const entries = [];
      for (const [key, entry] of input) entries.push([encode(key), encode(entry)]);
      nodes[index] = ['map', entries];
      return index;
    }
    if (__tbIsSet(input)) {
      const index = slot(null);
      seen.set(input, index);
      const entries = [];
      for (const entry of input) entries.push(encode(entry));
      nodes[index] = ['set', entries];
      return index;
    }
    if (Array.isArray(input)) {
      const index = slot(null);
      seen.set(input, index);
      const items = [];
      for (let item = 0; item < input.length; item++) items.push(encode(input[item]));
      nodes[index] = ['array', items];
      return index;
    }
    // Anything else clones as an object with its own enumerable properties,
    // whatever its prototype chain says
    // (<https://html.spec.whatwg.org/multipage/structured-data.html#structuredserializeinternal>).
    const index = slot(null);
    seen.set(input, index);
    const entries = [];
    for (const key of Object.keys(input)) entries.push([key, encode(input[key])]);
    nodes[index] = ['object', entries];
    return index;
  };
  const root = encode(value);
  // Serialization succeeded, so the transfer steps run now
  // (<https://html.spec.whatwg.org/multipage/structured-data.html#structuredserializewithtransfer>).
  for (const [buffer, index] of buffers) {
    let moved;
    try {
      moved = buffer.transfer();
    } catch (error) {
      throw new globalThis.DOMException('The buffer is already detached', 'DataCloneError');
    }
    if (index !== -1) {
      nodes[index] = ['buffer', __tbBase64Encode(new Uint8Array(moved))];
    }
  }
  const ports = [];
  for (const [port, id] of transferred) {
    if (!__tbPortDetach(id)) throw new globalThis.DOMException('Cannot transfer a detached MessagePort', 'DataCloneError');
    __tbBrand(port, __tbPortData).closed = true;
    // The received port is a new object; the sender's is detached.
    delete __tbPorts[id];
    ports.push(id);
  }
  return { payload: __tbPayloadVersion + JSON.stringify({ root: root, nodes: nodes }), ports: ports };
};

// Deserializes a payload in this realm; `ports` maps transferred endpoint ids
// to the port objects that were already materialized for them.
const __tbDecode = (payload, ports) => {
  const text = String(payload);
  if (text.indexOf(__tbPayloadVersion) !== 0) throw new Error('structured clone payload mismatch');
  const wire = JSON.parse(text.slice(__tbPayloadVersion.length));
  const nodes = wire.nodes;
  const values = new Array(nodes.length);
  for (let index = 0; index < nodes.length; index++) {
    switch (nodes[index][0]) {
      case 'buffer': values[index] = __tbBase64Bytes(nodes[index][1]).buffer; break;
      case 'object': values[index] = {}; break;
      case 'array': values[index] = new Array(nodes[index][1].length); break;
      case 'map': values[index] = new Map(); break;
      case 'set': values[index] = new Set(); break;
      default: values[index] = undefined; break;
    }
  }
  for (let index = 0; index < nodes.length; index++) {
    const descriptor = nodes[index];
    switch (descriptor[0]) {
      case 'view': {
        const buffer = values[descriptor[1]];
        values[index] = descriptor[2] === 'DataView'
          ? new DataView(buffer, descriptor[3], descriptor[4])
          : new globalThis[descriptor[2]](buffer, descriptor[3], descriptor[4]);
        break;
      }
      case 'undefined': values[index] = undefined; break;
      case 'null': values[index] = null; break;
      case 'boolean': values[index] = descriptor[1]; break;
      case 'number': values[index] = __tbNumberValue(descriptor[1]); break;
      case 'string': values[index] = descriptor[1]; break;
      case 'bigint': values[index] = BigInt(descriptor[1]); break;
      case 'date': values[index] = new Date(__tbNumberValue(descriptor[1])); break;
      case 'regexp': values[index] = new RegExp(descriptor[1], descriptor[2]); break;
      case 'domexception': values[index] = new globalThis.DOMException(descriptor[2], descriptor[1]); break;
      case 'error': {
        const name = descriptor[1];
        const ctor = globalThis[name];
        values[index] = typeof ctor === 'function' && ctor.prototype instanceof Error
          ? new ctor(descriptor[2])
          : new Error(descriptor[2]);
        values[index].name = name;
        break;
      }
      case 'boxed': {
        const ctor = globalThis[descriptor[1]];
        values[index] = new ctor(__tbNumberValue(descriptor[2]));
        break;
      }
      case 'blob': values[index] = new Blob([__tbBase64Bytes(descriptor[2])], { type: descriptor[1] }); break;
      case 'file': values[index] = new File([__tbBase64Bytes(descriptor[4])], descriptor[1], { type: descriptor[2], lastModified: descriptor[3] }); break;
      case 'port': {
        const port = ports[descriptor[1]];
        if (port === undefined) throw new Error('missing transferred MessagePort');
        values[index] = port;
        break;
      }
    }
  }
  for (let index = 0; index < nodes.length; index++) {
    const descriptor = nodes[index];
    switch (descriptor[0]) {
      case 'object': {
        const target = values[index];
        for (const [key, slot] of descriptor[1]) {
          // `CreateDataProperty`, not `Set`: a `__proto__` key must become an
          // own property instead of walking the prototype setter
          // (<https://html.spec.whatwg.org/multipage/structured-data.html#structureddeserialize>).
          Object.defineProperty(target, key, {
            value: values[slot], writable: true, enumerable: true, configurable: true,
          });
        }
        break;
      }
      case 'array': {
        const target = values[index];
        for (let item = 0; item < descriptor[1].length; item++) target[item] = values[descriptor[1][item]];
        break;
      }
      case 'map': {
        const target = values[index];
        for (const [key, entry] of descriptor[1]) target.set(values[key], values[entry]);
        break;
      }
      case 'set': {
        const target = values[index];
        for (const slot of descriptor[1]) target.add(values[slot]);
        break;
      }
    }
  }
  return values[wire.root];
};
// https://html.spec.whatwg.org/multipage/structured-data.html#dom-structuredclone
globalThis.structuredClone = function(value, options) {
  if (options != null && typeof options !== 'object') {
    throw new TypeError('The provided value is not of type StructuredSerializeOptions');
  }
  const transfer = options == null ? undefined : options.transfer;
  const encoded = __tbEncode(value, transfer, null);
  const materialized = __tbMaterializePorts(encoded.ports);
  const clone = __tbDecode(encoded.payload, materialized.ports);
  __tbFirePortCloses(materialized.closes);
  return clone;
};

// ── message ports ──────────────────────────────────────────────────────
// The endpoint (id, queue, entanglement) lives in Rust; the JS object is the
// realm's handle on it, registered in `__tbPorts` so deliveries can find it.
const __tbPorts = Object.create(null);

// Materializes every transferred endpoint. Closes are deferred: a port whose
// peer disentangled while it was in transit fires `close` once the carrying
// message has been dispatched, so the message handler can install
// `onclose` first
// (<https://html.spec.whatwg.org/multipage/web-messaging.html#disentangle>).
const __tbMaterializePorts = ids => {
  const ports = {};
  const closes = [];
  for (const id of ids) {
    const pendingClose = __tbPortAdopt(id);
    if (pendingClose == null) throw new Error('missing transferred MessagePort');
    ports[id] = __tbMaterializePort(id);
    if (pendingClose) closes.push(id);
  }
  return { ports: ports, closes: closes };
};
const __tbFirePortCloses = ids => {
  for (const id of ids) {
    const port = __tbPortLookup(id);
    if (port === null) continue;
    const data = __tbBrand(port, __tbPortData);
    data.closed = true;
    __tbPortClose(id);
    port.__tbDispatchTrusted(__tbHostToken, new globalThis.Event('close'));
  }
};
// Drops ports a delivery could not decode; the spec loses them with the
// failed message.
const __tbDiscardPorts = ids => {
  for (const id of ids) {
    const port = __tbPortLookup(id);
    if (port === null) continue;
    __tbBrand(port, __tbPortData).closed = true;
    __tbPortClose(id);
  }
};
const __tbMaterializePort = id => {
  if (__tbPorts[id] !== undefined) return __tbPorts[id];
  const port = Reflect.construct(globalThis.EventTarget, [], globalThis.MessagePort);
  Object.defineProperty(port, __tbPortData, {
    value: { id: id, closed: false, onmessage: null, onclose: null },
    writable: false, enumerable: false, configurable: false,
  });
  __tbPorts[id] = port;
  return port;
};
const __tbPortLookup = id => {
  const port = __tbPorts[id];
  return port === undefined ? null : port;
};
// https://html.spec.whatwg.org/multipage/web-messaging.html#messageport
globalThis.MessagePort = class MessagePort extends EventTarget {
  constructor() { throw new TypeError('Illegal constructor'); }
  postMessage(message, transfer) {
    const data = __tbBrand(this, __tbPortData);
    // Overload resolution: an iterable second argument is the transfer
    // sequence; any other object is the options dictionary, whose unknown
    // members are ignored
    // (<https://html.spec.whatwg.org/multipage/web-messaging.html#dom-messageport-postmessage>).
    if (transfer !== null && typeof transfer === 'object'
        && typeof transfer[Symbol.iterator] !== 'function') {
      transfer = transfer.transfer;
    }
    // The transfer list is consumed even when the port is detached or the
    // message is doomed
    // (<https://html.spec.whatwg.org/multipage/web-messaging.html#message-port-post-message-steps>).
    const encoded = __tbEncode(message, transfer, this);
    if (data.closed) return;
    // A port posted to its own entangled port loses the channel
    // (<https://html.spec.whatwg.org/multipage/web-messaging.html#message-port-post-message-steps>).
    const peer = __tbPortPeer(data.id);
    if (peer != null && encoded.ports.indexOf(peer) !== -1) return;
    __tbPortPost(data.id, encoded.payload, encoded.ports);
  }
  start() {
    const data = __tbBrand(this, __tbPortData);
    if (data.closed) return;
    __tbPortStart(data.id);
  }
  close() {
    const data = __tbBrand(this, __tbPortData);
    if (data.closed) return;
    data.closed = true;
    __tbPortClose(data.id);
  }
  get onmessage() { return __tbBrand(this, __tbPortData).onmessage; }
  set onmessage(value) {
    __tbBrand(this, __tbPortData).onmessage = value;
    // The first set enables the queue, whatever the value
    // (<https://html.spec.whatwg.org/multipage/web-messaging.html#message-ports>).
    this.start();
  }
  get onmessageerror() { return __tbBrand(this, __tbPortData).onmessageerror; }
  set onmessageerror(value) { __tbBrand(this, __tbPortData).onmessageerror = value; }
  get onclose() { return __tbBrand(this, __tbPortData).onclose; }
  set onclose(value) { __tbBrand(this, __tbPortData).onclose = value; }
};
Object.defineProperty(globalThis.MessagePort.prototype, Symbol.toStringTag, { value: 'MessagePort', writable: false, enumerable: false, configurable: true });
// https://html.spec.whatwg.org/multipage/web-messaging.html#messagechannel
globalThis.MessageChannel = class MessageChannel {
  constructor() {
    const pair = __tbPortNew();
    const port1 = __tbMaterializePort(pair[0]);
    const port2 = __tbMaterializePort(pair[1]);
    Object.defineProperty(this, 'port1', { value: port1, writable: false, enumerable: true, configurable: true });
    Object.defineProperty(this, 'port2', { value: port2, writable: false, enumerable: true, configurable: true });
  }
};
Object.defineProperty(globalThis.MessageChannel.prototype, Symbol.toStringTag, { value: 'MessageChannel', writable: false, enumerable: false, configurable: true });

// ── window proxies ─────────────────────────────────────────────────────
// One proxy per frame per realm, stable across the frame's navigations. It
// carries the cross-origin whitelist; same-origin members forward through the
// target realm's window
// (<https://html.spec.whatwg.org/multipage/window-object.html#the-windowproxy-exotic-object>).
// The cross-origin member set follows Blink's `[CrossOrigin]` IDL attributes
// and Firefox's `sCrossOriginProperties`: postMessage, window, self, frames,
// length, top, parent, closed, and the indexed getter.
const __tbFrameProxy = frame => {
  if (frame == null) return undefined;
  if (frame === __tbFrameId) return globalThis;
  if (__tbFrameProxies[frame] !== undefined) return __tbFrameProxies[frame];
  let proxy;
  const crossOrigin = () => {
    throw new globalThis.DOMException('Blocked a frame from accessing a cross-origin frame.', 'SecurityError');
  };
  // One function per proxy: a shipped engine keeps
  // `contentWindow.postMessage` identity stable, and so do we.
  const postMessage = function(message, targetOrigin, transfer) {
    try {
      return __tbPostMessage(frame, arguments.length, message, targetOrigin, transfer);
    } catch (error) {
      // Exceptions from a proxy's postMessage come from the target window's
      // realm, the way a shipped engine throws them.
      if (error instanceof globalThis.DOMException) {
        const global = __tbFrameGlobal(frame);
        const constructor = global == null ? undefined : global.DOMException;
        if (typeof constructor === 'function' && error.constructor !== constructor) {
          throw new constructor(error.message, error.name);
        }
      }
      throw error;
    }
  };
  const handler = {
    get(target, property) {
      if (property === __tbWindowProxyData) return { frame: frame };
      switch (property) {
        case 'postMessage':
          return postMessage;
        case 'parent': {
          const parent = __tbFrameParent(frame);
          return parent == null ? proxy : __tbFrameProxy(parent);
        }
        case 'top': {
          const top = __tbFrameTop(frame);
          return top == null || top === frame ? proxy : __tbFrameProxy(top);
        }
        case 'window': case 'self': case 'frames': return proxy;
        case 'length': return __tbFrameChildCount(frame);
        case 'closed': return !__tbFrameRegistered(frame);
        case Symbol.toStringTag: return 'Window';
        case 'document': return __tbFrameDocument(frame);
      }
      const sameOrigin = __tbFrameGlobal(frame);
      if (sameOrigin == null) {
        if (__tbFrameRegistered(frame)) {
          const pending = __tbFramePendingSets[frame];
          if (pending !== undefined && Object.prototype.hasOwnProperty.call(pending, property)) {
            return pending[property];
          }
          return undefined;
        }
        crossOrigin();
      }
      return sameOrigin[property];
    },
    set(target, property, value) {
      const sameOrigin = __tbFrameGlobal(frame);
      if (sameOrigin == null) {
        if (__tbFrameRegistered(frame)) {
          __tbFramePending(frame)[property] = value;
          return true;
        }
        crossOrigin();
      }
      sameOrigin[property] = value;
      return true;
    },
    has(target, property) {
      switch (property) {
        case 'postMessage': case 'parent': case 'top': case 'window': case 'self':
        case 'frames': case 'length': case 'closed': case 'document':
          return true;
      }
      const sameOrigin = __tbFrameGlobal(frame);
      if (sameOrigin != null) return property in sameOrigin;
      const pending = __tbFramePendingSets[frame];
      return pending !== undefined && property in pending;
    },
    getPrototypeOf() {
      // The spec forwards to the target; cross-origin callers cannot reach
      // it, so the plain object prototype stands in
      // (<https://html.spec.whatwg.org/multipage/window-object.html#windowproxy-getprototypeof>).
      const sameOrigin = __tbFrameGlobal(frame);
      return sameOrigin == null ? globalThis.Object.prototype : globalThis.Object.getPrototypeOf(sameOrigin);
    },
    ownKeys() {
      const sameOrigin = __tbFrameGlobal(frame);
      return sameOrigin == null ? [] : globalThis.Reflect.ownKeys(sameOrigin);
    },
    getOwnPropertyDescriptor() { return undefined; },
  };
  proxy = new Proxy({}, handler);
  __tbFrameProxies[frame] = proxy;
  return proxy;
};
globalThis.__tbFrameProxy = __tbFrameProxy;

// ── posting and delivery ───────────────────────────────────────────────
// https://html.spec.whatwg.org/multipage/web-messaging.html#dom-window-postmessage
function __tbPostMessage(targetFrame, argumentCount, message, targetOrigin, transfer) {
  if (argumentCount === 0) {
    throw new TypeError(`Failed to execute 'postMessage' on 'Window': 1 argument required, but only 0 present.`);
  }
  if (targetFrame == null) {
    throw new TypeError(`Failed to execute 'postMessage' on 'Window': the target window is missing.`);
  }
  // `postMessage(message, options)` dictionary overload
  // (<https://html.spec.whatwg.org/multipage/web-messaging.html#dom-window-postmessage>).
  if (targetOrigin !== null && typeof targetOrigin === 'object') {
    transfer = targetOrigin.transfer;
    targetOrigin = targetOrigin.targetOrigin;
  }
  const sourceOrigin = String(location.origin);
  let checkedOrigin = targetOrigin === undefined ? '/' : String(targetOrigin);
  if (checkedOrigin === '/') {
    checkedOrigin = sourceOrigin;
  } else if (checkedOrigin !== '*') {
    let parsed;
    try {
      parsed = new globalThis.URL(checkedOrigin);
    } catch (error) {
      throw new globalThis.DOMException('Invalid target origin', 'SyntaxError');
    }
    checkedOrigin = parsed.origin;
  }
  const encoded = __tbEncode(message, transfer, null);
  __tbPostWindowMessage(targetFrame, checkedOrigin, encoded.payload, encoded.ports);
}
globalThis.postMessage = function(message, targetOrigin, transfer) {
  return __tbPostMessage(__tbFrameId, arguments.length, message, targetOrigin, transfer);
};
// Runs in the target realm: decode, then dispatch a trusted message event, or
// report the failure so the engine dispatches `messageerror`
// (<https://html.spec.whatwg.org/multipage/web-messaging.html#window-post-message-steps>).
globalThis.__tbDeliverMessage = function(payload, sourceFrame, origin, portIds) {
  if (String(payload).indexOf(__tbPayloadVersion) !== 0) return false;
  const materialized = __tbMaterializePorts(portIds);
  let data;
  try {
    data = __tbDecode(payload, materialized.ports);
  } catch (error) {
    // The ports arrived with a message that cannot be decoded; they are lost
    // with it.
    __tbDiscardPorts(portIds);
    return false;
  }
  const portArray = portIds.map(id => materialized.ports[id]);
  globalThis.__tbDispatchTrusted(__tbHostToken, new globalThis.MessageEvent('message', {
    data: data, origin: origin, source: __tbFrameProxy(sourceFrame), ports: Object.freeze(portArray),
  }));
  __tbFirePortCloses(materialized.closes);
  return true;
};
globalThis.__tbDeliverMessageError = function(sourceFrame, origin) {
  globalThis.__tbDispatchTrusted(__tbHostToken, new globalThis.MessageEvent('messageerror', {
    data: null, origin: origin, source: __tbFrameProxy(sourceFrame),
  }));
};
// A message from another tab cannot name a frame in this renderer, so its
// source is `null`
// (<https://html.spec.whatwg.org/multipage/web-messaging.html#window-post-message-steps>).
globalThis.__tbDeliverRemoteMessage = function(payload) {
  if (String(payload).indexOf(__tbPayloadVersion) !== 0) return;
  let data;
  try {
    data = __tbDecode(payload, []);
  } catch (error) {
    return;
  }
  globalThis.__tbDispatchTrusted(__tbHostToken, new globalThis.MessageEvent('message', {
    data: data, origin: '', source: null, ports: Object.freeze([]),
  }));
};

// ── broadcast channels ─────────────────────────────────────────────────
// https://html.spec.whatwg.org/multipage/web-messaging.html#broadcastchannel
const __tbBroadcastData = Symbol.for('tinybrowser.broadcastchannel.data');
const __tbBroadcastChannels = new Set();
let __tbNextBroadcastChannel = 1;

globalThis.BroadcastChannel = class BroadcastChannel extends EventTarget {
  constructor(name) {
    if (arguments.length < 1) {
      throw new TypeError("Failed to construct 'BroadcastChannel': 1 argument required, but only 0 present.");
    }
    super();
    Object.defineProperty(this, __tbBroadcastData, {
      value: {
        id: __tbNextBroadcastChannel++, name: String(name), closed: false,
        onmessage: null, onmessageerror: null,
      },
      writable: false, enumerable: false, configurable: false,
    });
    __tbBroadcastChannels.add(this);
  }
  get name() { return __tbBrand(this, __tbBroadcastData).name; }
  postMessage(message) {
    if (arguments.length < 1) {
      throw new TypeError("Failed to execute 'postMessage' on 'BroadcastChannel': 1 argument required, but only 0 present.");
    }
    const data = __tbBrand(this, __tbBroadcastData);
    if (data.closed) {
      throw new globalThis.DOMException('The channel is closed.', 'InvalidStateError');
    }
    const encoded = __tbEncode(message, [], null);
    const origin = __tbStorageOrigin();
    if (origin === null || origin === undefined) return;
    __tbBroadcastPost(origin, data.name, encoded.payload, data.id);
  }
  close() {
    const data = __tbBrand(this, __tbBroadcastData);
    if (data.closed) return;
    data.closed = true;
    __tbBroadcastChannels.delete(this);
  }
  get onmessage() { return __tbBrand(this, __tbBroadcastData).onmessage; }
  set onmessage(value) { __tbBrand(this, __tbBroadcastData).onmessage = value; }
  get onmessageerror() { return __tbBrand(this, __tbBroadcastData).onmessageerror; }
  set onmessageerror(value) { __tbBrand(this, __tbBroadcastData).onmessageerror = value; }
};
Object.defineProperty(globalThis.BroadcastChannel.prototype, Symbol.toStringTag, {
  value: 'BroadcastChannel', writable: false, enumerable: false, configurable: true,
});

globalThis.__tbDeliverBroadcast = function(name, payload, origin, sourceChannel) {
  for (const channel of __tbBroadcastChannels) {
    const data = channel[__tbBroadcastData];
    if (data === undefined || data.closed || data.name !== name) continue;
    if (sourceChannel !== null && sourceChannel !== undefined && data.id === sourceChannel) continue;
    let event;
    try {
      event = new globalThis.MessageEvent('message', {
        data: __tbDecode(payload, []), origin: origin, source: null, ports: Object.freeze([]),
      });
    } catch (error) {
      event = new globalThis.MessageEvent('messageerror', {
        data: null, origin: origin, source: null, ports: Object.freeze([]),
      });
    }
    channel.__tbDispatchTrusted(__tbHostToken, event);
  }
};
globalThis.__tbDeliverPortMessage = function(endpoint, payload, portIds) {
  const port = __tbPortLookup(endpoint);
  if (port === null) return true;
  const data = __tbBrand(port, __tbPortData);
  if (data.closed) return true;
  if (String(payload).indexOf(__tbPayloadVersion) !== 0) return false;
  const materialized = __tbMaterializePorts(portIds);
  let value;
  try {
    value = __tbDecode(payload, materialized.ports);
  } catch (error) {
    __tbDiscardPorts(portIds);
    return false;
  }
  const portArray = portIds.map(id => materialized.ports[id]);
  port.__tbDispatchTrusted(__tbHostToken, new globalThis.MessageEvent('message', {
    data: value, ports: Object.freeze(portArray),
  }));
  __tbFirePortCloses(materialized.closes);
  return true;
};
globalThis.__tbDeliverPortMessageError = function(endpoint) {
  const port = __tbPortLookup(endpoint);
  if (port === null) return;
  port.__tbDispatchTrusted(__tbHostToken, new globalThis.MessageEvent('messageerror'));
};
globalThis.__tbDeliverPortClose = function(endpoint) {
  const port = __tbPortLookup(endpoint);
  if (port === null) return;
  const data = __tbBrand(port, __tbPortData);
  if (data.closed) return;
  port.__tbDispatchTrusted(__tbHostToken, new globalThis.Event('close'));
};

// ── the window's own indexed and browsing-context members ──────────────
// https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-length
Object.defineProperty(globalThis, 'length', {
  get() { return __tbFrameChildCount(__tbFrameId); },
  configurable: true, enumerable: false,
});
// https://html.spec.whatwg.org/multipage/window-object.html#dom-origin
Object.defineProperty(globalThis, 'origin', {
  get() {
    const value = __tbStorageOrigin();
    return value === null || value === undefined ? 'null' : value;
  },
  configurable: true, enumerable: true,
});
// https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-window-item
for (let index = 0; index < __tb_maxFrames; index++) {
  Object.defineProperty(globalThis, String(index), {
    get() {
      const child = __tbFrameChild(__tbFrameId, index);
      return child == null ? undefined : __tbFrameProxy(child);
    },
    configurable: true, enumerable: false,
  });
}
// https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-parent
Object.defineProperty(globalThis, 'parent', {
  get() {
    const parent = __tbFrameParent(__tbFrameId);
    return parent == null ? globalThis : __tbFrameProxy(parent);
  },
  configurable: true, enumerable: true,
});
// https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-top
Object.defineProperty(globalThis, 'top', {
  get() {
    const top = __tbFrameTop(__tbFrameId);
    return top === __tbFrameId ? globalThis : __tbFrameProxy(top);
  },
  configurable: true, enumerable: true,
});

// Event handler properties live in the world rather than on the wrapper, so
// a collected wrapper cannot lose `element.onload`
// (<https://html.spec.whatwg.org/multipage/webappapis.html#event-handlers>).
(function() {
  const names = globalThis.__tb_handlerNames;
  if (names === undefined) return;
  for (const name of names) {
    Object.defineProperty(globalThis, name, {
      get() { return globalThis.__tbGetWindowHandler(name); },
      set(value) { globalThis.__tbSetWindowHandler(name, value); },
      enumerable: true, configurable: true,
    });
    for (const proto of [globalThis.HTMLElement.prototype, globalThis.SVGElement.prototype]) {
      Object.defineProperty(proto, name, {
        get() { return globalThis.__tbGetNodeHandler(this, name); },
        set(value) { globalThis.__tbSetNodeHandler(this, name, value); },
        enumerable: true, configurable: true,
      });
    }
  }
})();

// ── auxiliary windows ──────────────────────────────────────────────────
// `window.open` creates a browser tab; the returned remote-window object
// exposes `close`, `postMessage`, and the same-origin storage areas. A named
// window is one browsing context, so reopening a live name returns the same
// object without a session copy
// (<https://html.spec.whatwg.org/multipage/window-object.html#dom-open>).
(function() {
  const remoteWindows = new Map();
  const namedWindows = new Map();
  const remoteSessions = new Map();
  // Tabs closed through `close()`: reported by `closed` and evicted from
  // `namedWindows` so a later `open(url, name)` opens a fresh tab.
  const closedTabs = new Set();
  const tabNames = new Map();

  function remoteWindow(tab) {
    const existing = remoteWindows.get(tab);
    if (existing !== undefined) return existing;
    const handler = {
      get(target, property) {
        switch (property) {
          case 'close': return () => {
            __tbWindowClose(tab);
            closedTabs.add(tab);
            const name = tabNames.get(tab);
            if (name !== undefined) {
              namedWindows.delete(name);
              tabNames.delete(tab);
            }
          };
          case 'closed': return closedTabs.has(tab);
          case 'postMessage': return (message, targetOrigin, transfer) => {
            const encoded = __tbEncode(message, transfer ?? [], null);
            __tbWindowPostMessage(tab, encoded.payload);
          };
          case 'localStorage': return globalThis.__tbStorageArea('local');
          case 'sessionStorage': return remoteSession(tab);
          case Symbol.toStringTag: return 'Window';
          default: return undefined;
        }
      },
      has(target, property) {
        switch (property) {
          case 'close': case 'closed': case 'postMessage':
          case 'localStorage': case 'sessionStorage':
            return true;
        }
        return false;
      },
      getOwnPropertyDescriptor() { return undefined; },
      ownKeys() { return []; },
    };
    const proxy = new Proxy({}, handler);
    remoteWindows.set(tab, proxy);
    return proxy;
  }

  // A live read of another window's session area, for same-origin openers and
  // opened windows. The response carries the storage seam's encoded strings.
  function remoteSession(tab) {
    const existing = remoteSessions.get(tab);
    if (existing !== undefined) return existing;
    const session = {
      getItem(key) {
        if (arguments.length < 1) {
          throw new TypeError("Failed to execute 'getItem' on 'Storage': 1 argument required, but only 0 present.");
        }
        const value = __tbRemoteSessionGet(tab, globalThis.__tbStorageEncode(String(key)));
        return value === undefined ? null : globalThis.__tbStorageDecode(value);
      },
    };
    remoteSessions.set(tab, session);
    return session;
  }

  globalThis.open = function(url, target, features) {
    if (arguments.length < 1 || url === undefined || url === null) url = '';
    const spec = url === '' ? '' : __tbResolveUrl(String(url), undefined);
    if (spec === null || spec === undefined) return null;
    const name = target === undefined || target === null ? '' : String(target);
    const featureString = features === undefined || features === null ? '' : String(features);
    if (name !== '' && namedWindows.has(name)) return namedWindows.get(name);
    const tab = __tbWindowOpen(spec, name, featureString);
    if (tab === null || tab === undefined) return null;
    const proxy = remoteWindow(tab);
    if (name !== '') {
      namedWindows.set(name, proxy);
      tabNames.set(tab, name);
    }
    return proxy;
  };

  // https://html.spec.whatwg.org/multipage/window-object.html#dom-opener
  Object.defineProperty(globalThis, 'opener', {
    get() {
      const tab = __tbWindowOpener();
      return tab === null || tab === undefined ? null : remoteWindow(tab);
    },
    configurable: true,
  });
})();

// ── web storage ─────────────────────────────────────────────────────────
// `localStorage` and `sessionStorage` are one interface over two areas: the
// browser process owns the local area (shared by every tab, persisted with
// the profile), the engine owns the session area (one per top-level browsing
// context). Items are named properties per the legacy platform-object rules:
// a stored item is hidden by a member of the prototype chain, and every
// string-keyed write stores
// (<https://html.spec.whatwg.org/multipage/webstorage.html#the-storage-interface>,
// <https://webidl.spec.whatwg.org/#legacy-platform-object>).
(function() {
  const kindSlot = Symbol('storageKind');
  const holders = new Map();

  function kindOf(storage) {
    const kind = storage == null ? undefined : storage[kindSlot];
    if (kind !== 'local' && kind !== 'session') {
      throw new TypeError('Illegal invocation');
    }
    return kind;
  }

  const quotaError = key => new globalThis.QuotaExceededError(
    "Failed to execute 'setItem' on 'Storage': Setting the value of '" + key +
    "' exceeded the quota.");

  // JavaScript strings may hold lone surrogates; DOMString preserves them but
  // the UTF-8 host seam cannot. JSON escaping round-trips them exactly
  // (<https://webidl.spec.whatwg.org/#idl-DOMString>).
  const encode = value => JSON.stringify(value);
  const decode = value => (value === null || value === undefined ? null : JSON.parse(value));

  // https://storage.spec.whatwg.org/#quotaexceedederror
  // This user agent names no requested size, so both members are `null`;
  // they live on the subclass so a plain `DOMException` named
  // `QuotaExceededError` does not grow them.
  globalThis.QuotaExceededError = class QuotaExceededError extends globalThis.DOMException {
    constructor(message = '') {
      super(message, 'QuotaExceededError');
    }
    get requested() { return null; }
    get quota() { return null; }
  };
  Object.defineProperty(globalThis.QuotaExceededError.prototype, Symbol.toStringTag, {
    value: 'QuotaExceededError', writable: false, enumerable: false, configurable: true,
  });

  class Storage {
    constructor(kind) {
      Object.defineProperty(this, kindSlot, {
        value: kind, writable: false, enumerable: false, configurable: false,
      });
    }
    get length() {
      return __tbStorageKeys(kindOf(this)).length;
    }
    key(index) {
      if (arguments.length < 1) {
        throw new TypeError("Failed to execute 'key' on 'Storage': 1 argument required, but only 0 present.");
      }
      const keys = __tbStorageKeys(kindOf(this));
      const n = Number(index) >>> 0;
      return n < keys.length ? decode(keys[n]) : null;
    }
    getItem(key) {
      if (arguments.length < 1) {
        throw new TypeError("Failed to execute 'getItem' on 'Storage': 1 argument required, but only 0 present.");
      }
      const item = __tbStorageGet(kindOf(this), encode(String(key)));
      return item === undefined ? null : decode(item);
    }
    setItem(key, value) {
      if (arguments.length < 2) {
        throw new TypeError("Failed to execute 'setItem' on 'Storage': 2 arguments required.");
      }
      key = String(key);
      if (!__tbStorageSet(kindOf(this), encode(key), encode(String(value)))) {
        throw quotaError(key);
      }
    }
    removeItem(key) {
      if (arguments.length < 1) {
        throw new TypeError("Failed to execute 'removeItem' on 'Storage': 1 argument required, but only 0 present.");
      }
      __tbStorageRemove(kindOf(this), encode(String(key)));
    }
    clear() {
      __tbStorageClear(kindOf(this));
    }
  }
  globalThis.Storage = Storage;
  Object.defineProperty(Storage.prototype, Symbol.toStringTag, {
    value: 'Storage', writable: false, enumerable: false, configurable: true,
  });

  function area(kind) {
    const existing = holders.get(kind);
    if (existing !== undefined) return existing;
    const origin = __tbStorageOrigin();
    if (origin === null || origin === undefined) {
      throw new globalThis.DOMException(
        "Failed to read the '" + (kind === 'session' ? 'sessionStorage' : 'localStorage') +
        "' property from 'Window': Storage is unavailable for opaque origins.", 'SecurityError');
    }
    const target = new Storage(kind);
    const handler = {
      get(t, property, receiver) {
        if (typeof property === 'symbol' || property in t) {
          return Reflect.get(t, property, receiver);
        }
        const item = __tbStorageGet(kind, encode(property));
        return item === null || item === undefined ? undefined : decode(item);
      },
      set(t, property, value) {
        if (typeof property === 'symbol') return Reflect.set(t, property, value);
        if (!__tbStorageSet(kind, encode(property), encode(String(value)))) {
          throw quotaError(property);
        }
        return true;
      },
      has(t, property) {
        if (typeof property === 'symbol' || property in t) return true;
        const item = __tbStorageGet(kind, encode(property));
        return item !== null && item !== undefined;
      },
      deleteProperty(t, property) {
        if (typeof property === 'symbol') return Reflect.deleteProperty(t, property);
        __tbStorageRemove(kind, encode(property));
        return true;
      },
      defineProperty(t, property, descriptor) {
        if (typeof property === 'symbol') return Reflect.defineProperty(t, property, descriptor);
        const value = 'value' in descriptor ? String(descriptor.value) : 'undefined';
        if (!__tbStorageSet(kind, encode(property), encode(value))) throw quotaError(property);
        return true;
      },
      getOwnPropertyDescriptor(t, property) {
        if (typeof property === 'symbol') return Reflect.getOwnPropertyDescriptor(t, property);
        if (property in t) return undefined;
        const item = __tbStorageGet(kind, encode(property));
        if (item === null || item === undefined) return undefined;
        return { value: decode(item), writable: true, enumerable: true, configurable: true };
      },
      ownKeys(t) {
        const names = __tbStorageKeys(kind).map(decode).filter(key => !(key in t));
        return Reflect.ownKeys(t).concat(names);
      },
    };
    const proxy = new Proxy(target, handler);
    holders.set(kind, proxy);
    return proxy;
  }

  // https://html.spec.whatwg.org/multipage/webstorage.html#the-storageevent-interface
  const eventData = Symbol.for('tinybrowser.storageevent.data');
  function dataOf(event) {
    const data = event == null ? undefined : event[eventData];
    if (data === undefined) throw new TypeError('Illegal invocation');
    return data;
  }
  function nullableString(value) {
    return value === undefined || value === null ? null : String(value);
  }
  globalThis.StorageEvent = class StorageEvent extends Event {
    constructor(type, init = undefined) {
      if (arguments.length < 1) {
        throw new TypeError("Failed to construct 'StorageEvent': 1 argument required, but only 0 present.");
      }
      const eventInit = init === undefined ? {} : Object(init);
      super(String(type), eventInit);
      Object.defineProperty(this, eventData, {
        value: {
          key: nullableString(eventInit.key),
          oldValue: nullableString(eventInit.oldValue),
          newValue: nullableString(eventInit.newValue),
          url: eventInit.url === undefined ? '' : String(eventInit.url),
          storageArea: eventInit.storageArea === undefined ? null : eventInit.storageArea,
        },
        writable: false, enumerable: false, configurable: false,
      });
    }
    get key() { return dataOf(this).key; }
    get oldValue() { return dataOf(this).oldValue; }
    get newValue() { return dataOf(this).newValue; }
    get url() { return dataOf(this).url; }
    get storageArea() { return dataOf(this).storageArea; }
    initStorageEvent(type, bubbles = false, cancelable = false, key = null, oldValue = null, newValue = null, url = '', storageArea = null) {
      if (arguments.length < 1) {
        throw new TypeError("Failed to execute 'initStorageEvent' on 'StorageEvent': 1 argument required, but only 0 present.");
      }
      Event.prototype.initEvent.call(this, String(type), Boolean(bubbles), Boolean(cancelable));
      const data = dataOf(this);
      data.key = key === null ? null : String(key);
      data.oldValue = oldValue === null ? null : String(oldValue);
      data.newValue = newValue === null ? null : String(newValue);
      data.url = String(url);
      data.storageArea = storageArea;
    }
  };
  Object.defineProperty(globalThis.StorageEvent.prototype, Symbol.toStringTag, {
    value: 'StorageEvent', writable: false, enumerable: false, configurable: true,
  });

  /// Fires one storage event in this realm, with this realm's area as
  /// `storageArea`
  /// (<https://html.spec.whatwg.org/multipage/webstorage.html#concept-storage-broadcast>).
  globalThis.__tbFireStorageEvent = function(kind, key, oldValue, newValue, url) {
    globalThis.__tbDispatchTrusted(__tbHostToken, new globalThis.StorageEvent('storage', {
      key: decode(key), oldValue: decode(oldValue), newValue: decode(newValue),
      url: url, storageArea: area(kind),
    }));
  };

  Object.defineProperty(globalThis, 'localStorage', {
    get() { return area('local'); }, configurable: true,
  });
  Object.defineProperty(globalThis, 'sessionStorage', {
    get() { return area('session'); }, configurable: true,
  });
  // Remote-window proxies share these areas for same-origin openers.
  globalThis.__tbStorageArea = area;
  globalThis.__tbStorageEncode = encode;
  globalThis.__tbStorageDecode = decode;
})();

// Platform objects and globals are not serializable; the marker travels with
// the prototype, so it identifies an object from another realm too
// (<https://html.spec.whatwg.org/multipage/structured-data.html#serializable-objects>).
(function() {
  const names = [
    'Event', 'EventTarget', 'Node', 'DOMException', 'Attr', 'NamedNodeMap',
    'TokenList', 'Implementation', 'DOMParser', 'XMLSerializer', 'MutationObserver',
    'MutationRecord', 'MessageEvent', 'MessagePort', 'MessageChannel', 'Headers',
    'Request', 'Response', 'Blob', 'File', 'FileList', 'FileReader', 'ProgressEvent',
    'ReadableStream', 'TextDecoder', 'TextEncoder', 'URL', 'URLSearchParams',
    'AbortController', 'AbortSignal', 'CustomEvent', 'Document',
    'Storage', 'StorageEvent', 'QuotaExceededError',
  ];
  for (const name of names) {
    const ctor = globalThis[name];
    if (typeof ctor === 'function' && ctor.prototype !== undefined) {
      try {
        Object.defineProperty(ctor.prototype, __tbPlatform, {
          value: true, writable: false, enumerable: false, configurable: true,
        });
      } catch (error) {}
    }
  }
  Object.defineProperty(globalThis, __tbPlatform, {
    value: true, writable: false, enumerable: false, configurable: true,
  });
})();
