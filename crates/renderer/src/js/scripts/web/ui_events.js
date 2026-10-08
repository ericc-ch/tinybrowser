
// ── uievents ───────────────────────────────────────────────────────────────
// Event interfaces for input dispatch. The engine fires its own input events
// from Rust; these classes exist so pages can construct events with the
// spec's properties.

const __tbUIEventData = host.slots('tinybrowser.uievent.data');
const __tbUIEventConstructor = globalThis.UIEvent = class UIEvent extends Event {
  constructor(type, init) {
    if (arguments.length < 1) {
      throw new TypeError("Failed to construct 'UIEvent': 1 argument required, but only 0 present.");
    }
    init = init || {};
    const event = __tbConstruct(__tbEventConstructor, [type, init], new.target);
    __tbUIEventData.set(event, { view: init.view || null, detail: init.detail || 0 });
    return event;
  }
  get view() { return __tbBrand(this, __tbUIEventData).view; }
  get detail() { return __tbBrand(this, __tbUIEventData).detail; }
};
Object.defineProperty(globalThis.UIEvent.prototype, Symbol.toStringTag, { value: 'UIEvent', writable: false, enumerable: false, configurable: true });

const __tbMouseEventData = host.slots('tinybrowser.mouseevent.data');
const __tbMouseEventConstructor = globalThis.MouseEvent = class MouseEvent extends UIEvent {
  constructor(type, init) {
    if (arguments.length < 1) {
      throw new TypeError("Failed to construct 'MouseEvent': 1 argument required, but only 0 present.");
    }
    init = init || {};
    const event = __tbConstruct(__tbUIEventConstructor, [type, init], new.target);
    // `long` members convert with `ToNumber`, not truthiness: `"5"` is 5
    // (<https://w3c.github.io/uievents/#dom-mouseevent-clientx>).
    const toLong = value => { const n = Number(value); return Number.isNaN(n) ? 0 : Math.trunc(n); };
    const button = init.button === undefined ? 0 : toLong(init.button);
    __tbMouseEventData.set(event, {
        screenX: init.screenX === undefined ? 0 : toLong(init.screenX),
        screenY: init.screenY === undefined ? 0 : toLong(init.screenY),
        clientX: init.clientX === undefined ? 0 : toLong(init.clientX),
        clientY: init.clientY === undefined ? 0 : toLong(init.clientY),
        ctrlKey: !!init.ctrlKey, shiftKey: !!init.shiftKey,
        altKey: !!init.altKey, metaKey: !!init.metaKey,
        button: button, buttons: init.buttons === undefined ? 0 : toLong(init.buttons),
        relatedTarget: init.relatedTarget || null,
    });
    return event;
  }
  get screenX() { return __tbBrand(this, __tbMouseEventData).screenX; }
  get screenY() { return __tbBrand(this, __tbMouseEventData).screenY; }
  get clientX() { return __tbBrand(this, __tbMouseEventData).clientX; }
  get clientY() { return __tbBrand(this, __tbMouseEventData).clientY; }
  get ctrlKey() { return __tbBrand(this, __tbMouseEventData).ctrlKey; }
  get shiftKey() { return __tbBrand(this, __tbMouseEventData).shiftKey; }
  get altKey() { return __tbBrand(this, __tbMouseEventData).altKey; }
  get metaKey() { return __tbBrand(this, __tbMouseEventData).metaKey; }
  get button() { return __tbBrand(this, __tbMouseEventData).button; }
  get buttons() { return __tbBrand(this, __tbMouseEventData).buttons; }
  get relatedTarget() { return __tbBrand(this, __tbMouseEventData).relatedTarget; }
  getModifierState(key) {
    return { Alt: !!this.altKey, Control: !!this.ctrlKey, Meta: !!this.metaKey, Shift: !!this.shiftKey }[String(key)] || false;
  }
};
Object.defineProperty(globalThis.MouseEvent.prototype, Symbol.toStringTag, { value: 'MouseEvent', writable: false, enumerable: false, configurable: true });

const __tbPointerEventData = host.slots('tinybrowser.pointerevent.data');
globalThis.PointerEvent = class PointerEvent extends MouseEvent {
  constructor(type, init) {
    if (arguments.length < 1) {
      throw new TypeError("Failed to construct 'PointerEvent': 1 argument required, but only 0 present.");
    }
    init = init || {};
    const event = __tbConstruct(__tbMouseEventConstructor, [type, init], new.target);
    __tbPointerEventData.set(event, {
        pointerId: init.pointerId === undefined ? 1 : init.pointerId,
        // An explicit 0 is a valid width, not a missing one
        // (<https://w3c.github.io/pointerevents/#dom-pointerevent-width>).
        width: init.width === undefined ? 1 : Number(init.width),
        height: init.height === undefined ? 1 : Number(init.height),
        pressure: init.pressure === undefined ? 0 : init.pressure,
        tangentialPressure: init.tangentialPressure || 0,
        tiltX: init.tiltX || 0, tiltY: init.tiltY || 0, twist: init.twist || 0,
        pointerType: init.pointerType || 'mouse',
        isPrimary: init.isPrimary === undefined ? true : !!init.isPrimary,
    });
    return event;
  }
  get pointerId() { return __tbBrand(this, __tbPointerEventData).pointerId; }
  get width() { return __tbBrand(this, __tbPointerEventData).width; }
  get height() { return __tbBrand(this, __tbPointerEventData).height; }
  get pressure() { return __tbBrand(this, __tbPointerEventData).pressure; }
  get tangentialPressure() { return __tbBrand(this, __tbPointerEventData).tangentialPressure; }
  get tiltX() { return __tbBrand(this, __tbPointerEventData).tiltX; }
  get tiltY() { return __tbBrand(this, __tbPointerEventData).tiltY; }
  get twist() { return __tbBrand(this, __tbPointerEventData).twist; }
  get pointerType() { return __tbBrand(this, __tbPointerEventData).pointerType; }
  get isPrimary() { return __tbBrand(this, __tbPointerEventData).isPrimary; }
};
Object.defineProperty(globalThis.PointerEvent.prototype, Symbol.toStringTag, { value: 'PointerEvent', writable: false, enumerable: false, configurable: true });

const __tbWheelEventData = host.slots('tinybrowser.wheelevent.data');
globalThis.WheelEvent = class WheelEvent extends MouseEvent {
  constructor(type, init) {
    if (arguments.length < 1) {
      throw new TypeError("Failed to construct 'WheelEvent': 1 argument required, but only 0 present.");
    }
    init = init || {};
    const event = __tbConstruct(__tbMouseEventConstructor, [type, init], new.target);
    __tbWheelEventData.set(event, {
        deltaX: init.deltaX || 0, deltaY: init.deltaY || 0, deltaZ: init.deltaZ || 0,
        deltaMode: init.deltaMode || 0,
    });
    return event;
  }
  get deltaX() { return __tbBrand(this, __tbWheelEventData).deltaX; }
  get deltaY() { return __tbBrand(this, __tbWheelEventData).deltaY; }
  get deltaZ() { return __tbBrand(this, __tbWheelEventData).deltaZ; }
  get deltaMode() { return __tbBrand(this, __tbWheelEventData).deltaMode; }
};
Object.defineProperty(globalThis.WheelEvent.prototype, Symbol.toStringTag, { value: 'WheelEvent', writable: false, enumerable: false, configurable: true });

const __tbKeyboardEventData = host.slots('tinybrowser.keyboardevent.data');
globalThis.KeyboardEvent = class KeyboardEvent extends UIEvent {
  constructor(type, init) {
    if (arguments.length < 1) {
      throw new TypeError("Failed to construct 'KeyboardEvent': 1 argument required, but only 0 present.");
    }
    init = init || {};
    const event = __tbConstruct(__tbUIEventConstructor, [type, init], new.target);
    const key = init.key === undefined ? '' : String(init.key);
    __tbKeyboardEventData.set(event, {
        key: key,
        code: init.code === undefined ? '' : String(init.code),
        location: init.location || 0,
        ctrlKey: !!init.ctrlKey, shiftKey: !!init.shiftKey,
        altKey: !!init.altKey, metaKey: !!init.metaKey,
        repeat: !!init.repeat, isComposing: !!init.isComposing,
        keyCode: init.keyCode === undefined ? (key.length === 1 ? key.charCodeAt(0) : 0) : init.keyCode,
        charCode: init.charCode || 0,
    });
    return event;
  }
  get key() { return __tbBrand(this, __tbKeyboardEventData).key; }
  get code() { return __tbBrand(this, __tbKeyboardEventData).code; }
  get location() { return __tbBrand(this, __tbKeyboardEventData).location; }
  get ctrlKey() { return __tbBrand(this, __tbKeyboardEventData).ctrlKey; }
  get shiftKey() { return __tbBrand(this, __tbKeyboardEventData).shiftKey; }
  get altKey() { return __tbBrand(this, __tbKeyboardEventData).altKey; }
  get metaKey() { return __tbBrand(this, __tbKeyboardEventData).metaKey; }
  get repeat() { return __tbBrand(this, __tbKeyboardEventData).repeat; }
  get isComposing() { return __tbBrand(this, __tbKeyboardEventData).isComposing; }
  get keyCode() { return __tbBrand(this, __tbKeyboardEventData).keyCode; }
  get charCode() { return __tbBrand(this, __tbKeyboardEventData).charCode; }
  getModifierState(key) {
    return { Alt: !!this.altKey, Control: !!this.ctrlKey, Meta: !!this.metaKey, Shift: !!this.shiftKey }[String(key)] || false;
  }
};
Object.defineProperty(globalThis.KeyboardEvent.prototype, Symbol.toStringTag, { value: 'KeyboardEvent', writable: false, enumerable: false, configurable: true });

const __tbInputEventData = host.slots('tinybrowser.inputevent.data');
globalThis.InputEvent = class InputEvent extends UIEvent {
  constructor(type, init) {
    if (arguments.length < 1) {
      throw new TypeError("Failed to construct 'InputEvent': 1 argument required, but only 0 present.");
    }
    init = init || {};
    const event = __tbConstruct(__tbUIEventConstructor, [type, init], new.target);
    __tbInputEventData.set(event, {
        data: init.data === undefined ? null : init.data,
        inputType: init.inputType === undefined ? '' : String(init.inputType),
        isComposing: !!init.isComposing,
        dataTransfer: init.dataTransfer || null,
    });
    return event;
  }
  get data() { return __tbBrand(this, __tbInputEventData).data; }
  get inputType() { return __tbBrand(this, __tbInputEventData).inputType; }
  get isComposing() { return __tbBrand(this, __tbInputEventData).isComposing; }
  get dataTransfer() { return __tbBrand(this, __tbInputEventData).dataTransfer; }
};
Object.defineProperty(globalThis.InputEvent.prototype, Symbol.toStringTag, { value: 'InputEvent', writable: false, enumerable: false, configurable: true });

// Interfaces named by document.createEvent
// (https://dom.spec.whatwg.org/#dom-document-createevent).
// createEvent builds an uninitialized Event and sets this prototype; it does
// not call the constructor, which requires a type.
function __tbExposeEvent(name, parent) {
  const ctor = {
    [name]: class extends parent {
      constructor(type) {
        if (arguments.length < 1) {
          throw new TypeError("Failed to construct '" + name + "': 1 argument required, but only 0 present.");
        }
        const init = arguments.length < 2 || arguments[1] == null ? {} : arguments[1];
        return __tbConstruct(parent, [type, init], new.target);
      }
    },
  }[name];
  Object.defineProperty(ctor.prototype, Symbol.toStringTag, {
    value: name, writable: false, enumerable: false, configurable: true,
  });
  globalThis[name] = ctor;
}
__tbExposeEvent('BeforeUnloadEvent', Event);
__tbExposeEvent('HashChangeEvent', Event);
__tbExposeEvent('DeviceMotionEvent', Event);
__tbExposeEvent('DeviceOrientationEvent', Event);
__tbExposeEvent('CompositionEvent', UIEvent);
__tbExposeEvent('FocusEvent', UIEvent);
__tbExposeEvent('TextEvent', UIEvent);
__tbExposeEvent('TouchEvent', UIEvent);
__tbExposeEvent('DragEvent', MouseEvent);

// Touch Events extends GlobalEventHandlers with these handlers. createEvent's
// touch rows run only when the document exposes one
// (https://w3c.github.io/touch-events/#extensions-to-the-globaleventhandlers-mixin).
if (typeof Document !== 'undefined') {
  for (const name of ['ontouchstart', 'ontouchend', 'ontouchmove', 'ontouchcancel']) {
    Object.defineProperty(Document.prototype, name, {
      get() { return host.__tbGetNodeHandler(this, name); },
      set(value) { host.__tbSetNodeHandler(this, name, value); },
      enumerable: true, configurable: true,
    });
  }
}
