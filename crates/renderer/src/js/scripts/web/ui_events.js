
// ── uievents ───────────────────────────────────────────────────────────────
// Event interfaces for input dispatch. The engine fires its own input events
// from Rust; these classes exist so pages can construct events with the
// spec's properties.

const __tbUIEventData = Symbol.for('tinybrowser.uievent.data');
globalThis.UIEvent = class UIEvent extends Event {
  constructor(type, init) {
    if (arguments.length < 1) {
      throw new TypeError("Failed to construct 'UIEvent': 1 argument required, but only 0 present.");
    }
    init = init || {};
    super(type, init);
    Object.defineProperty(this, __tbUIEventData, {
      value: { view: init.view || null, detail: init.detail || 0 },
      writable: false, enumerable: false, configurable: false,
    });
  }
  get view() { return __tbBrand(this, __tbUIEventData).view; }
  get detail() { return __tbBrand(this, __tbUIEventData).detail; }
};
Object.defineProperty(globalThis.UIEvent.prototype, Symbol.toStringTag, { value: 'UIEvent', writable: false, enumerable: false, configurable: true });

const __tbMouseEventData = Symbol.for('tinybrowser.mouseevent.data');
globalThis.MouseEvent = class MouseEvent extends UIEvent {
  constructor(type, init) {
    if (arguments.length < 1) {
      throw new TypeError("Failed to construct 'MouseEvent': 1 argument required, but only 0 present.");
    }
    init = init || {};
    super(type, init);
    const button = init.button === undefined ? 0 : init.button;
    Object.defineProperty(this, __tbMouseEventData, {
      value: {
        screenX: init.screenX || 0, screenY: init.screenY || 0,
        clientX: init.clientX || 0, clientY: init.clientY || 0,
        ctrlKey: !!init.ctrlKey, shiftKey: !!init.shiftKey,
        altKey: !!init.altKey, metaKey: !!init.metaKey,
        button: button, buttons: init.buttons === undefined ? 0 : init.buttons,
        relatedTarget: init.relatedTarget || null,
      },
      writable: false, enumerable: false, configurable: false,
    });
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

const __tbPointerEventData = Symbol.for('tinybrowser.pointerevent.data');
globalThis.PointerEvent = class PointerEvent extends MouseEvent {
  constructor(type, init) {
    if (arguments.length < 1) {
      throw new TypeError("Failed to construct 'PointerEvent': 1 argument required, but only 0 present.");
    }
    init = init || {};
    super(type, init);
    Object.defineProperty(this, __tbPointerEventData, {
      value: {
        pointerId: init.pointerId === undefined ? 1 : init.pointerId,
        width: init.width || 1, height: init.height || 1,
        pressure: init.pressure === undefined ? 0 : init.pressure,
        tangentialPressure: init.tangentialPressure || 0,
        tiltX: init.tiltX || 0, tiltY: init.tiltY || 0, twist: init.twist || 0,
        pointerType: init.pointerType || 'mouse',
        isPrimary: init.isPrimary === undefined ? true : !!init.isPrimary,
      },
      writable: false, enumerable: false, configurable: false,
    });
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

const __tbWheelEventData = Symbol.for('tinybrowser.wheelevent.data');
globalThis.WheelEvent = class WheelEvent extends MouseEvent {
  constructor(type, init) {
    if (arguments.length < 1) {
      throw new TypeError("Failed to construct 'WheelEvent': 1 argument required, but only 0 present.");
    }
    init = init || {};
    super(type, init);
    Object.defineProperty(this, __tbWheelEventData, {
      value: {
        deltaX: init.deltaX || 0, deltaY: init.deltaY || 0, deltaZ: init.deltaZ || 0,
        deltaMode: init.deltaMode || 0,
      },
      writable: false, enumerable: false, configurable: false,
    });
  }
  get deltaX() { return __tbBrand(this, __tbWheelEventData).deltaX; }
  get deltaY() { return __tbBrand(this, __tbWheelEventData).deltaY; }
  get deltaZ() { return __tbBrand(this, __tbWheelEventData).deltaZ; }
  get deltaMode() { return __tbBrand(this, __tbWheelEventData).deltaMode; }
};
Object.defineProperty(globalThis.WheelEvent.prototype, Symbol.toStringTag, { value: 'WheelEvent', writable: false, enumerable: false, configurable: true });

const __tbKeyboardEventData = Symbol.for('tinybrowser.keyboardevent.data');
globalThis.KeyboardEvent = class KeyboardEvent extends UIEvent {
  constructor(type, init) {
    if (arguments.length < 1) {
      throw new TypeError("Failed to construct 'KeyboardEvent': 1 argument required, but only 0 present.");
    }
    init = init || {};
    super(type, init);
    const key = init.key === undefined ? '' : String(init.key);
    Object.defineProperty(this, __tbKeyboardEventData, {
      value: {
        key: key,
        code: init.code === undefined ? '' : String(init.code),
        location: init.location || 0,
        ctrlKey: !!init.ctrlKey, shiftKey: !!init.shiftKey,
        altKey: !!init.altKey, metaKey: !!init.metaKey,
        repeat: !!init.repeat, isComposing: !!init.isComposing,
        keyCode: init.keyCode === undefined ? key.charCodeAt(0) : init.keyCode,
        charCode: init.charCode || 0,
      },
      writable: false, enumerable: false, configurable: false,
    });
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

const __tbInputEventData = Symbol.for('tinybrowser.inputevent.data');
globalThis.InputEvent = class InputEvent extends UIEvent {
  constructor(type, init) {
    if (arguments.length < 1) {
      throw new TypeError("Failed to construct 'InputEvent': 1 argument required, but only 0 present.");
    }
    init = init || {};
    super(type, init);
    Object.defineProperty(this, __tbInputEventData, {
      value: {
        data: init.data === undefined ? null : init.data,
        inputType: init.inputType === undefined ? '' : String(init.inputType),
        isComposing: !!init.isComposing,
        dataTransfer: init.dataTransfer || null,
      },
      writable: false, enumerable: false, configurable: false,
    });
  }
  get data() { return __tbBrand(this, __tbInputEventData).data; }
  get inputType() { return __tbBrand(this, __tbInputEventData).inputType; }
  get isComposing() { return __tbBrand(this, __tbInputEventData).isComposing; }
  get dataTransfer() { return __tbBrand(this, __tbInputEventData).dataTransfer; }
};
Object.defineProperty(globalThis.InputEvent.prototype, Symbol.toStringTag, { value: 'InputEvent', writable: false, enumerable: false, configurable: true });
