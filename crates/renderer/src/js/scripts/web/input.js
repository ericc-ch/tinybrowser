
// The WebDriver Perform Actions sequence
// (<https://w3c.github.io/webdriver/#perform-actions>). Element origins arrive
// rewritten by the WebDriver crate as `{__tbRemote: <number>}`; ticks from
// every source run in order. Durations are treated as zero: the engine has no
// per-frame interpolation yet.
(function() {
  const pointerStates = new Map();
  const pointerState = (id, pointerType) => {
    if (!pointerStates.has(id)) {
      pointerStates.set(id, { x: 0, y: 0, buttons: 0, target: null, pointerType: pointerType || 'mouse' });
    }
    return pointerStates.get(id);
  };
  const at = (x, y) => {
    const found = document.elementFromPoint ? document.elementFromPoint(x, y) : null;
    return found || document.documentElement || document.body;
  };
  const centerOf = origin => {
    if (!origin || typeof origin !== 'object' || typeof origin.__tbRemote !== 'number') return null;
    const element = globalThis.__tb_webdriver_element(origin.__tbRemote);
    if (!element || typeof element.getBoundingClientRect !== 'function') return null;
    const rect = element.getBoundingClientRect();
    return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
  };
  const mouseInit = (x, y, buttons, extra) => Object.assign({
    bubbles: true, cancelable: true, composed: true, view: globalThis,
    clientX: x, clientY: y, screenX: x, screenY: y, buttons: buttons || 0,
  }, extra || {});
  // Pointer events carry the source's pointer type; the extra argument can be
  // either a buttons value (ignored) or an init object, matching how the
  // mouse init is called above.
  const pointerInit = (state, x, y, buttonsOrExtra, maybeExtra) => {
    const extra = maybeExtra === undefined ? null : maybeExtra;
    return Object.assign(mouseInit(x, y, state.buttons, extra), {
      pointerId: 1, isPrimary: true, pointerType: state.pointerType,
    });
  };
  const fire = (node, event) => { if (node) node.dispatchEvent(event); };
  const pointerItem = (state, item) => {
    if (item.type === 'pointerMove') {
      const center = centerOf(item.origin);
      let x, y;
      if (center) { x = center.x + (item.x || 0); y = center.y + (item.y || 0); }
      else if (item.origin === 'pointer') { x = state.x + (item.x || 0); y = state.y + (item.y || 0); }
      else { x = item.x || 0; y = item.y || 0; }
      const next = at(x, y);
      if (state.target && state.target !== next) {
        fire(state.target, new PointerEvent('pointerout', pointerInit(state, state.x, state.y, state.buttons)));
        fire(state.target, new PointerEvent('pointerleave', pointerInit(state, state.x, state.y, state.buttons)));
        fire(state.target, new MouseEvent('mouseout', mouseInit(state.x, state.y, state.buttons)));
        fire(state.target, new MouseEvent('mouseleave', mouseInit(state.x, state.y, state.buttons)));
        fire(next, new PointerEvent('pointerover', pointerInit(state, x, y, state.buttons)));
        fire(next, new PointerEvent('pointerenter', pointerInit(state, x, y, state.buttons)));
        fire(next, new MouseEvent('mouseover', mouseInit(x, y, state.buttons)));
        fire(next, new MouseEvent('mouseenter', mouseInit(x, y, state.buttons)));
      }
      state.x = x; state.y = y; state.target = next;
      fire(next, new PointerEvent('pointermove', pointerInit(state, x, y, state.buttons)));
      fire(next, new MouseEvent('mousemove', mouseInit(x, y, state.buttons)));
    } else if (item.type === 'pointerDown') {
      const button = item.button || 0;
      state.buttons |= 1 << button;
      const node = at(state.x, state.y);
      state.target = node;
      if (node && typeof node.focus === 'function') { try { node.focus(); } catch (error) {} }
      fire(node, new PointerEvent('pointerdown', pointerInit(state, state.x, state.y, state.buttons, { button: button })));
      fire(node, new MouseEvent('mousedown', mouseInit(state.x, state.y, state.buttons, { button: button })));
    } else if (item.type === 'pointerUp') {
      const button = item.button || 0;
      const node = at(state.x, state.y);
      fire(node, new PointerEvent('pointerup', pointerInit(state, state.x, state.y, state.buttons, { button: button })));
      fire(node, new MouseEvent('mouseup', mouseInit(state.x, state.y, state.buttons, { button: button })));
      if (button === 0) fire(node, new MouseEvent('click', mouseInit(state.x, state.y, 0, { button: 0 })));
      state.buttons &= ~(1 << button);
    } else if (item.type === 'pointerCancel') {
      fire(at(state.x, state.y), new PointerEvent('pointercancel', pointerInit(state, state.x, state.y, state.buttons)));
      state.buttons = 0;
    }
  };
  const specialKeys = {
    '\uE003': ['Backspace', 'Backspace'], '\uE004': ['Tab', 'Tab'],
    '\uE006': ['Enter', 'Enter'], '\uE007': ['Enter', 'Enter'],
    '\uE008': ['Shift', 'ShiftLeft'], '\uE00C': ['Escape', 'Escape'],
    '\uE00D': [' ', 'Space'], '\uE00E': ['PageUp', 'PageUp'], '\uE00F': ['PageDown', 'PageDown'],
    '\uE010': ['End', 'End'], '\uE011': ['Home', 'Home'],
    '\uE012': ['ArrowLeft', 'ArrowLeft'], '\uE013': ['ArrowUp', 'ArrowUp'],
    '\uE014': ['ArrowRight', 'ArrowRight'], '\uE015': ['ArrowDown', 'ArrowDown'],
    '\uE017': ['Delete', 'Delete'],
  };
  const keyOf = value => {
    if (specialKeys[value]) return { key: specialKeys[value][0], code: specialKeys[value][1] };
    if (value.length === 1) {
      const code = value >= 'a' && value <= 'z' ? 'Key' + value.toUpperCase()
        : value >= 'A' && value <= 'Z' ? 'Key' + value
        : value >= '0' && value <= '9' ? 'Digit' + value
        : '';
      return { key: value, code: code };
    }
    return { key: value, code: value };
  };
  const insertText = text => {
    const element = document.activeElement;
    if (!element) return;
    const tag = element.tagName;
    if (tag === 'INPUT' || tag === 'TEXTAREA') {
      const start = element.selectionStart === undefined || element.selectionStart === null ? element.value.length : element.selectionStart;
      const end = element.selectionEnd === undefined || element.selectionEnd === null ? start : element.selectionEnd;
      if (typeof element.setRangeText === 'function') element.setRangeText(text, start, end, 'end');
      else element.value = element.value.slice(0, start) + text + element.value.slice(end);
      fire(element, new InputEvent('beforeinput', { bubbles: true, cancelable: true, inputType: 'insertText', data: text }));
      fire(element, new InputEvent('input', { bubbles: true, inputType: 'insertText', data: text }));
    } else if (element.isContentEditable) {
      element.textContent = (element.textContent || '') + text;
      fire(element, new InputEvent('input', { bubbles: true, inputType: 'insertText', data: text }));
    }
  };
  const keyItem = (item, down) => {
    const value = item.value === undefined ? '' : String(item.value);
    const info = keyOf(value);
    const node = document.activeElement || document.body || document.documentElement;
    if (down) {
      fire(node, new KeyboardEvent('keydown', { bubbles: true, cancelable: true, key: info.key, code: info.code }));
      if (value.length === 1 || value === '\uE00D') insertText(value === '\uE00D' ? ' ' : value);
      if (value.length === 1) fire(node, new KeyboardEvent('keypress', { bubbles: true, cancelable: true, key: info.key, code: info.code }));
    } else {
      fire(node, new KeyboardEvent('keyup', { bubbles: true, cancelable: true, key: info.key, code: info.code }));
    }
  };
  const wheelItem = item => {
    const x = item.x || 0;
    const y = item.y || 0;
    const node = at(x, y);
    const accepted = fire(node, new WheelEvent('wheel', mouseInit(x, y, 0, {
      deltaX: item.deltaX || 0, deltaY: item.deltaY || 0,
    })));
    if (typeof node.scrollBy === 'function') node.scrollBy(item.deltaX || 0, item.deltaY || 0);
  };
  globalThis.__tbWebDriverActions = function(actions) {
    let ticks = 0;
    for (const source of actions) {
      const count = source.actions ? source.actions.length : 0;
      if (count > ticks) ticks = count;
    }
    for (let tick = 0; tick < ticks; tick++) {
      for (const source of actions) {
        const item = source.actions ? source.actions[tick] : null;
        if (!item || item.type === 'pause') continue;
        if (source.type === 'pointer') {
          const parameters = source.parameters || {};
          pointerItem(pointerState(source.id, parameters.pointerType), item);
        } else if (source.type === 'key') {
          if (item.type === 'insertText') insertText(String(item.value === undefined ? '' : item.value));
          else keyItem(item, item.type === 'keyDown');
        } else if (source.type === 'wheel') wheelItem(item);
      }
    }
    return true;
  };
  Object.defineProperty(globalThis, '__tbWebDriverActions', {
    writable: false, configurable: false, enumerable: false,
  });
})();
