// Form-control infrastructure that is pure Web IDL sugar: length reflection
// and the constraint validation API. Host state that touches the tree stays a
// Rust binding (docs/researches/engine-source.md), so this file only reads
// attributes and the `value` IDL attribute.
(function() {
  const ASCII_WHITESPACE = /^[\t\n\f\r ]+|[\t\n\f\r ]+$/g;

  // Rules for parsing non-negative integers
  // (<https://html.spec.whatwg.org/multipage/common-microsyntaxes.html#rules-for-parsing-non-negative-integers>).
  const parseNonNegative = value => {
    const text = String(value).replace(ASCII_WHITESPACE, '');
    if (text === '' || !/^[0-9]+$/.test(text)) return null;
    return Number(text);
  };

  // Web IDL `long` conversion (<https://webidl.spec.whatwg.org/#es-long>).
  const toLong = value => {
    let number = Number(value);
    if (!Number.isFinite(number)) number = 0;
    return Math.trunc(number) | 0;
  };

  // Content attributes whose absence is `-1` and whose setter rejects a
  // negative value (<https://html.spec.whatwg.org/multipage/input.html#dom-input-maxlength>).
  const lengthProperty = name => ({
    get: function() {
      const raw = this.getAttribute(name);
      if (raw === null) return -1;
      const parsed = parseNonNegative(raw);
      return parsed === null ? -1 : parsed;
    },
    set: function(value) {
      const number = toLong(value);
      if (number < 0) {
        throw new DOMException(
          'The length is negative: ' + number, 'IndexSizeError');
      }
      this.setAttribute(name, String(number));
    },
    enumerable: true,
    configurable: true,
  });

  const customErrors = new WeakMap();
  const validityStates = new WeakMap();

  const VALUE_MODE_TYPES = new Set([
    'text', 'search', 'tel', 'url', 'email', 'password', 'date', 'month',
    'week', 'time', 'datetime-local', 'number',
  ]);
  const BARRED_TYPES = new Set(['hidden', 'button', 'reset', 'image']);

  const isInput = element => element.tagName === 'INPUT';
  const isTextControl = element =>
    element.tagName === 'TEXTAREA' || (isInput(element) && VALUE_MODE_TYPES.has(element.type));

  // <https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#barred-from-constraint-validation>
  const barredFromValidation = element => {
    if (element.disabled) return true;
    switch (element.tagName) {
      case 'INPUT':
        return BARRED_TYPES.has(element.type) || element.readOnly;
      case 'TEXTAREA':
        return element.readOnly;
      case 'SELECT':
        return false;
      case 'BUTTON':
        return element.type !== 'submit';
      default:
        return true;
    }
  };

  const anyRadioChecked = element => {
    if (element.checked) return true;
    const name = element.name;
    if (!name) return false;
    const root = element.form || element.ownerDocument;
    if (!root || typeof root.getElementsByTagName !== 'function') return false;
    for (const radio of root.getElementsByTagName('input')) {
      if (radio !== element && radio.type === 'radio' && radio.name === name && radio.checked) {
        return true;
      }
    }
    return false;
  };

  // <https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#suffering-from-being-missing>
  const valueMissing = element => {
    if (!element.required) return false;
    switch (element.tagName) {
      case 'INPUT':
        switch (element.type) {
          case 'checkbox': return !element.checked;
          case 'radio': return !anyRadioChecked(element);
          case 'file': return element.files.length === 0;
          case 'hidden': return false;
          default: return isTextControl(element) && element.value === '';
        }
      case 'TEXTAREA':
        return element.value === '';
      case 'SELECT':
        return element.value === '';
      default:
        return false;
    }
  };

  const tooLong = element => {
    if (!isTextControl(element)) return false;
    const maximum = element.maxLength;
    return maximum >= 0 && element.value.length > maximum;
  };

  const tooShort = element => {
    if (!isTextControl(element)) return false;
    const minimum = element.minLength;
    return minimum > 0 && element.value.length !== 0 && element.value.length < minimum;
  };

  const FLAGS = {
    valueMissing,
    tooLong,
    tooShort,
    customError: element => (customErrors.get(element) || '') !== '',
  };

  const isValid = element => !Object.values(FLAGS).some(check => check(element));

  class ValidityState {
    constructor() {
      throw new TypeError('Illegal constructor');
    }
  }
  Object.defineProperty(ValidityState.prototype, Symbol.toStringTag, {
    value: 'ValidityState', writable: false, enumerable: false, configurable: true,
  });
  Object.defineProperty(globalThis, 'ValidityState', {
    value: ValidityState, writable: true, enumerable: false, configurable: true,
  });

  const validityOf = element => {
    let state = validityStates.get(element);
    if (state !== undefined) return state;
    const descriptors = {};
    const names = [
      'valueMissing', 'typeMismatch', 'patternMismatch', 'tooLong', 'tooShort',
      'rangeUnderflow', 'rangeOverflow', 'stepMismatch', 'badInput',
      'customError', 'valid',
    ];
    for (const name of names) {
      descriptors[name] = {
        get() {
          if (name === 'valid') return isValid(element);
          const check = FLAGS[name];
          return check === undefined ? false : check(element);
        },
        enumerable: true,
        configurable: true,
      };
    }
    state = Object.create(ValidityState.prototype, descriptors);
    validityStates.set(element, state);
    return state;
  };

  const validationMessageOf = element => {
    const custom = customErrors.get(element) || '';
    if (custom !== '') return custom;
    return isValid(element) ? '' : 'Constraints not satisfied';
  };

  const fireInvalid = element => {
    element.dispatchEvent(new Event('invalid', { bubbles: false, cancelable: true }));
  };

  const validationMember = {
    willValidate: {
      get() { return !barredFromValidation(this); },
      enumerable: true,
      configurable: true,
    },
    validity: {
      get() { return validityOf(this); },
      enumerable: true,
      configurable: true,
    },
    validationMessage: {
      get() { return validationMessageOf(this); },
      enumerable: true,
      configurable: true,
    },
    checkValidity: {
      value: function() {
        if (barredFromValidation(this) || isValid(this)) return true;
        fireInvalid(this);
        return false;
      },
      writable: true,
      enumerable: true,
      configurable: true,
    },
    reportValidity: {
      value: function() {
        if (barredFromValidation(this) || isValid(this)) return true;
        fireInvalid(this);
        return false;
      },
      writable: true,
      enumerable: true,
      configurable: true,
    },
    setCustomValidity: {
      value: function(message) {
        customErrors.set(this, String(message));
      },
      writable: true,
      enumerable: true,
      configurable: true,
    },
  };

  for (const name of [
    'HTMLInputElement', 'HTMLTextAreaElement', 'HTMLSelectElement',
    'HTMLButtonElement', 'HTMLFieldSetElement', 'HTMLOutputElement',
  ]) {
    const constructor = globalThis[name];
    if (!constructor || !constructor.prototype) continue;
    Object.defineProperties(constructor.prototype, validationMember);
  }

  for (const name of ['HTMLInputElement', 'HTMLTextAreaElement']) {
    const constructor = globalThis[name];
    if (!constructor || !constructor.prototype) continue;
    Object.defineProperties(constructor.prototype, {
      maxLength: lengthProperty('maxlength'),
      minLength: lengthProperty('minlength'),
    });
  }
})();
