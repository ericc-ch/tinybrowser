
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
  const fire = (node, event) => node ? node.dispatchEvent(event) : false;
  const pointerItem = (state, item) => {
    if (item.type === 'pointerMove') {
      const center = centerOf(item.origin);
      let x, y;
      if (center) { x = center.x + (item.x || 0); y = center.y + (item.y || 0); }
      else if (item.origin === 'pointer') { x = state.x + (item.x || 0); y = state.y + (item.y || 0); }
      else if (item.origin === undefined || item.origin === 'viewport') { x = item.x || 0; y = item.y || 0; }
      // An element origin that resolves to nothing is gone: the WebDriver
      // behavior is a stale element error, never a silent viewport move
      // (<https://w3c.github.io/webdriver/#dfn-pointer-move>).
      else throw new Error('stale element reference: pointer origin did not resolve');
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
      // The released button clears before the events fire: `mouseup` and
      // `pointerup` report the buttons still held
      // (<https://w3c.github.io/uievents/#dom-mouseevent-buttons>).
      state.buttons &= ~(1 << button);
      const node = at(state.x, state.y);
      fire(node, new PointerEvent('pointerup', pointerInit(state, state.x, state.y, state.buttons, { button: button })));
      fire(node, new MouseEvent('mouseup', mouseInit(state.x, state.y, state.buttons, { button: button })));
      if (button === 0) fire(node, new MouseEvent('click', mouseInit(state.x, state.y, 0, { button: 0 })));
    } else if (item.type === 'pointerCancel') {
      fire(at(state.x, state.y), new PointerEvent('pointercancel', pointerInit(state, state.x, state.y, state.buttons)));
      state.buttons = 0;
    } else throw new TypeError('unknown pointer action type: ' + item.type);
  };
  const specialKeys = {
    // No key state is tracked, so NULL has no keys to release; it fires like
    // any other non-printable key instead of leaking the PUA codepoint.
    '\uE000': ['Unidentified', ''], '\uE003': ['Backspace', 'Backspace'], '\uE004': ['Tab', 'Tab'],
    '\uE006': ['Enter', 'Enter'], '\uE007': ['Enter', 'Enter'],
    '\uE008': ['Shift', 'ShiftLeft'], '\uE009': ['Control', 'ControlLeft'],
    '\uE00A': ['Alt', 'AltLeft'], '\uE00C': ['Escape', 'Escape'],
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
  const textControl = () => {
    const element = document.activeElement;
    if (!element) return null;
    const tag = element.tagName;
    if (tag !== 'INPUT' && tag !== 'TEXTAREA') return null;
    if (element.disabled || element.readOnly) return null;
    return element;
  };
  const fireInput = (element, inputType, data) => {
    fire(element, new InputEvent('input', { bubbles: true, inputType: inputType, data: data }));
  };
  // Replaces the inclusive range with `text` through `setRangeText`, gating on
  // the cancelable `beforeinput` event first
  // (<https://w3c.github.io/uievents/#events-inputevents>).
  const replaceRange = (element, text, start, end, inputType, data) => {
    const before = new InputEvent('beforeinput', {
      bubbles: true, cancelable: true, inputType: inputType, data: data,
    });
    if (!element.dispatchEvent(before)) return;
    if (typeof element.setRangeText === 'function') element.setRangeText(text, start, end, 'end');
    else element.value = element.value.slice(0, start) + text + element.value.slice(end);
    fireInput(element, inputType, data);
  };
  const insertText = text => {
    const element = textControl();
    if (!element) return;
    const start = element.selectionStart === undefined || element.selectionStart === null
      ? element.value.length : element.selectionStart;
    const end = element.selectionEnd === undefined || element.selectionEnd === null
      ? start : element.selectionEnd;
    replaceRange(element, text, start, end, 'insertText', text);
  };
  const deleteText = backwards => {
    const element = textControl();
    if (!element) return;
    const start = element.selectionStart;
    const end = element.selectionEnd;
    if (start === end) {
      if (backwards && start === 0) return;
      if (!backwards && end === element.value.length) return;
      const from = backwards ? start - 1 : start;
      const to = backwards ? end : end + 1;
      replaceRange(element, '', from, to, 'deleteContentBackward', null);
    } else {
      replaceRange(element, '', start, end, 'deleteContentBackward', null);
    }
  };
  const moveCaret = to => {
    const element = textControl();
    if (!element) return;
    const length = element.value.length;
    const start = element.selectionStart;
    const end = element.selectionEnd;
    let position;
    if (to === 'home') position = 0;
    else if (to === 'end') position = length;
    else if (to === 'left') position = start === end ? Math.max(0, start - 1) : start;
    else position = start === end ? Math.min(length, end + 1) : end;
    element.setSelectionRange(position, position);
  };
  const keyItem = (item, down) => {
    const value = item.value === undefined ? '' : String(item.value);
    const info = keyOf(value);
    const node = document.activeElement || document.body || document.documentElement;
    if (down) {
      const accepted = fire(node, new KeyboardEvent('keydown', {
        bubbles: true, cancelable: true, key: info.key, code: info.code,
      }));
      if (accepted) {
        if (value.length === 1) insertText(value);
        else if (value === '\uE00D') insertText(' ');
        else if (value === '\uE003') deleteText(true);
        else if (value === '\uE017') deleteText(false);
        else if (value === '\uE011') moveCaret('home');
        else if (value === '\uE010') moveCaret('end');
        else if (value === '\uE012') moveCaret('left');
        else if (value === '\uE014') moveCaret('right');
      }
      if (value.length === 1) fire(node, new KeyboardEvent('keypress', { bubbles: true, cancelable: true, key: info.key, code: info.code }));
    } else {
      fire(node, new KeyboardEvent('keyup', { bubbles: true, cancelable: true, key: info.key, code: info.code }));
    }
  };
  const wheelItem = item => {
    const x = item.x || 0;
    const y = item.y || 0;
    const node = at(x, y);
    if (!node) return;
    // A canceled `wheel` event performs no scroll
    // (<https://w3c.github.io/uievents/#events-wheel>).
    const accepted = fire(node, new WheelEvent('wheel', mouseInit(x, y, 0, {
      deltaX: item.deltaX || 0, deltaY: item.deltaY || 0,
    })));
    if (accepted && typeof node.scrollBy === 'function') node.scrollBy(item.deltaX || 0, item.deltaY || 0);
  };
  // The WebDriver "element send keys" command: focus the element and feed each
  // character through the same editing path as Perform Actions, so special
  // keys like Backspace edit instead of appending their private-use codepoint
  // (<https://w3c.github.io/webdriver/#element-send-keys>).
  const hookChangeOnBlur = element => {
    if (element.__tbChangeHooked) return;
    Object.defineProperty(element, '__tbChangeHooked', { value: true, configurable: true });
    element.addEventListener('blur', () => {
      if (element.value !== element.__tbChangeBaseline) {
        element.dispatchEvent(new Event('change', { bubbles: true }));
      }
    });
  };
  globalThis.__tbWebDriverSendKeys = function(element, text) {
    if (!element || typeof element.focus !== 'function') return false;
    try { element.focus(); } catch (error) {}
    hookChangeOnBlur(element);
    element.__tbChangeBaseline = element.value;
    for (const character of String(text)) {
      const code = character.charCodeAt(0);
      if (character.length === 1 && code >= 0xE000 && code <= 0xE05D) {
        if (character === '\uE003') deleteText(true);
        else if (character === '\uE017') deleteText(false);
        else if (character === '\uE011') moveCaret('home');
        else if (character === '\uE010') moveCaret('end');
        else if (character === '\uE012') moveCaret('left');
        else if (character === '\uE014') moveCaret('right');
        else if (character === '\uE00D') insertText(' ');
      } else {
        insertText(character);
      }
    }
    return true;
  };
  Object.defineProperty(globalThis, '__tbWebDriverSendKeys', {
    writable: false, configurable: false, enumerable: false,
  });
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
          if (item.type === 'keyDown') keyItem(item, true);
          else if (item.type === 'keyUp') keyItem(item, false);
          else if (item.type === 'insertText') insertText(String(item.value === undefined ? '' : item.value));
          else throw new TypeError('unknown key action type: ' + item.type);
        } else if (source.type === 'wheel') {
          if (item.type !== 'scroll') throw new TypeError('unknown wheel action type: ' + item.type);
          wheelItem(item);
        } else throw new TypeError('unknown action source type: ' + source.type);
      }
    }
    return true;
  };
  Object.defineProperty(globalThis, '__tbWebDriverActions', {
    writable: false, configurable: false, enumerable: false,
  });
})();
