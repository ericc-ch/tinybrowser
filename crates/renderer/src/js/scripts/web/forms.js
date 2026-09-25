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
  // Controls whose value was last changed by a user edit. `maxlength` and
  // `minlength` apply only then
  // (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#suffering-from-being-too-long>).
  const userEdited = new WeakSet();
  // The input/typing paths call this so `maxlength`/`minlength` can tell a
  // user edit from a script assignment
  // (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#suffering-from-being-too-long>).
  Object.defineProperty(globalThis, '__tbMarkUserEdited', {
    value: element => { userEdited.add(element); },
    writable: false, configurable: false, enumerable: false,
  });

  const VALUE_MODE_TYPES = new Set([
    'text', 'search', 'tel', 'url', 'email', 'password', 'date', 'month',
    'week', 'time', 'datetime-local', 'number',
  ]);
  // The text-selection APIs apply only to these input types (plus textarea);
  // notably email and number are excluded
  // (<https://html.spec.whatwg.org/multipage/input.html#do-not-apply>).
  const SELECTION_TYPES = new Set(['text', 'search', 'tel', 'url', 'password']);
  const BARRED_TYPES = new Set(['hidden', 'button', 'reset', 'image']);

  const isInput = element => element.tagName === 'INPUT';
  const isTextControl = element =>
    element.tagName === 'TEXTAREA' || (isInput(element) && VALUE_MODE_TYPES.has(element.type));

  // <https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#barred-from-constraint-validation>
  // A control is disabled by a disabled fieldset ancestor, unless it is inside
  // the fieldset's first legend
  // (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#concept-fe-disabled>).
  const fieldsetDisabled = element => {
    for (let ancestor = element.parentElement; ancestor; ancestor = ancestor.parentElement) {
      if (ancestor.tagName === 'FIELDSET' && ancestor.hasAttribute('disabled')) {
        const legend = Array.from(ancestor.children).find(child => child.tagName === 'LEGEND');
        if (legend === undefined || !legend.contains(element)) return true;
        return false;
      }
    }
    return false;
  };

  const barredFromValidation = element => {
    if (element.disabled || fieldsetDisabled(element)) return true;
    // A control inside a `datalist` is barred
    // (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#barred-from-constraint-validation>).
    for (let ancestor = element.parentElement; ancestor; ancestor = ancestor.parentElement) {
      if (ancestor.tagName === 'DATALIST') return true;
    }
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

  // The scope a radio button group lives in: its form owner, or the root of
  // its tree (which may be detached)
  // (<https://html.spec.whatwg.org/multipage/input.html#radio-button-group>).
  const radioRoot = element => {
    if (element.form) return element.form;
    if (typeof element.getRootNode === 'function') return element.getRootNode();
    return element.ownerDocument;
  };
  const radiosIn = root => {
    if (!root) return [];
    if (typeof root.querySelectorAll === 'function') return Array.from(root.querySelectorAll('input'));
    if (typeof root.getElementsByTagName === 'function') return Array.from(root.getElementsByTagName('input'));
    return [];
  };

  const anyRadioChecked = element => {
    if (element.checked) return true;
    const name = element.name;
    if (!name) return false;
    for (const radio of radiosIn(radioRoot(element))) {
      if (radio !== element && radio.type === 'radio' && radio.name === name
          && radio.form === element.form && radio.checked) {
        return true;
      }
    }
    return false;
  };

  // A radio button group is required when any member has the `required`
  // attribute; every member then suffers from being missing while none is
  // checked
  // (<https://html.spec.whatwg.org/multipage/input.html#radio-button-state-(type=radio):suffering-from-being-missing>).
  const radioGroupRequired = element => {
    if (element.required) return true;
    const name = element.name;
    if (!name) return false;
    for (const radio of radiosIn(radioRoot(element))) {
      if (radio.type === 'radio' && radio.name === name
          && radio.form === element.form && radio.required) return true;
    }
    return false;
  };

  // A control is mutable when it is neither disabled nor readonly
  // (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#mutability>).
  const isMutable = element =>
    !element.disabled && !fieldsetDisabled(element) && !element.readOnly;

  // <https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#suffering-from-being-missing>
  const valueMissing = element => {
    // A radio group is missing while no member is checked, for every member
    // of a group that has any required member
    // (<https://html.spec.whatwg.org/multipage/input.html#radio-button-state-(type=radio):suffering-from-being-missing>).
    if (element.tagName === 'INPUT' && element.type === 'radio') {
      // A radio with an empty name is not part of a radio button group, so
      // the radio-group clause does not apply
      // (<https://html.spec.whatwg.org/multipage/input.html#radio-button-state-(type=radio)>).
      if (element.name === '') return false;
      return radioGroupRequired(element) && !anyRadioChecked(element);
    }
    if (!element.required) return false;
    switch (element.tagName) {
      case 'INPUT':
        switch (element.type) {
          // The checkedness and filename clauses do not require a mutable
          // control; only the value-mode clause does.
          case 'checkbox': return !element.checked;
          case 'file': return element.files.length === 0;
          case 'hidden': return false;
          default: return isMutable(element) && isTextControl(element) && element.value === '';
        }
      case 'TEXTAREA':
        return isMutable(element) && element.value === '';
      case 'SELECT':
        // The select clause does not require mutability.
        return element.value === '';
      default:
        return false;
    }
  };

  const tooLong = element => {
    if (!isTextControl(element) || !userEdited.has(element)) return false;
    const maximum = element.maxLength;
    return maximum >= 0 && element.value.length > maximum;
  };

  const tooShort = element => {
    if (!isTextControl(element) || !userEdited.has(element)) return false;
    const minimum = element.minLength;
    return minimum > 0 && element.value.length !== 0 && element.value.length < minimum;
  };

  const inputType = element => (isInput(element) ? element.type : '');

  // An email/url control whose value does not match its type
  // (<https://html.spec.whatwg.org/multipage/input.html#the-email-state-(type=email)>).
  const typeMismatch = element => {
    const value = element.value;
    if (value === '') return false;
    const type = inputType(element);
    if (type === 'email') {
      const address = /^[a-zA-Z0-9.!#$%&'*+/=?^_`{|}~-]+@[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*$/;
      // A comma separates addresses only with the `multiple` attribute
      // (<https://html.spec.whatwg.org/multipage/input.html#attr-input-multiple>).
      if (element.hasAttribute('multiple')) {
        return value.split(',').some(part => !address.test(part.trim()));
      }
      return !address.test(value);
    }
    if (type === 'url') {
      try { new globalThis.URL(value); return false; } catch (error) { return true; }
    }
    return false;
  };

  // QuickJS's `v` implementation rejects a few characters that the spec allows
  // as literals (`/`, and `-` outside a character class); translate them to
  // equivalent escapes so the pattern still compiles
  // (<https://html.spec.whatwg.org/multipage/input.html#the-pattern-attribute>).
  const translatePattern = pattern => {
    let translated = '';
    let depth = 0;
    for (let index = 0; index < pattern.length; index += 1) {
      const character = pattern[index];
      if (character === '\\' && index + 1 < pattern.length) {
        translated += character + pattern[index + 1];
        index += 1;
        continue;
      }
      if (character === '[') depth += 1;
      else if (character === ']' && depth > 0) depth -= 1;
      else if (character === '/') { translated += '\\/'; continue; }
      else if (character === '-' && depth === 0) { translated += '\\u002D'; continue; }
      translated += character;
    }
    return translated;
  };

  // The `pattern` attribute compiled with the `v` flag; a pattern that does
  // not compile is ignored
  // (<https://html.spec.whatwg.org/multipage/input.html#the-pattern-attribute>).
  const compiledPattern = element => {
    const pattern = element.getAttribute('pattern');
    if (pattern === null || pattern === '') return null;
    const translated = translatePattern(pattern);
    try {
      new RegExp(translated, 'v');
    } catch (error) {
      return null;
    }
    return new RegExp(`^(?:${translated})$`, 'v');
  };

  const patternMismatch = element => {
    if (element.value === '') return false;
    const expression = compiledPattern(element);
    if (expression === null) return false;
    // For a multiple email control the pattern applies to each address
    // (<https://html.spec.whatwg.org/multipage/input.html#attr-input-multiple>).
    if (inputType(element) === 'email' && element.hasAttribute('multiple')) {
      return element.value.split(',').some(part => !expression.test(part.trim()));
    }
    return !expression.test(element.value);
  };

  // Value-as-number scales for the date-like states, in each state's step
  // unit: days for date, months for month, weeks for week, seconds for time
  // and datetime-local
  // (<https://html.spec.whatwg.org/multipage/input.html#value-as-a-number>).
  const daysFromCivil = (year, month, day) => {
    const y = year - (month <= 2 ? 1 : 0);
    const era = Math.floor((y >= 0 ? y : y - 399) / 400);
    const yearOfEra = y - era * 400;
    const dayOfYear = Math.floor((153 * (month + (month > 2 ? -3 : 9)) + 2) / 5) + day - 1;
    const dayOfEra = yearOfEra * 365
      + Math.floor(yearOfEra / 4) - Math.floor(yearOfEra / 100) + dayOfYear;
    return era * 146097 + dayOfEra - 719468;
  };
  // Monday is 0 ... Sunday is 6; 1970-01-01 was a Thursday.
  const weekdayFromDays = days => (((days % 7) + 10) % 7);
  const parseDate = value => {
    const match = /^(\d{4,6})-(\d{2})-(\d{2})$/.exec(value);
    return match ? daysFromCivil(+match[1], +match[2], +match[3]) : null;
  };
  const parseMonth = value => {
    const match = /^(\d{4,6})-(\d{2})$/.exec(value);
    return match ? (+match[1] - 1970) * 12 + (+match[2]) - 1 : null;
  };
  const epochMonday = (() => {
    const januaryFirst = daysFromCivil(1970, 1, 1);
    return januaryFirst - weekdayFromDays(januaryFirst);
  })();
  const parseWeek = value => {
    const match = /^(\d{4,6})-W(\d{2})$/.exec(value);
    if (!match) return null;
    const januaryFourth = daysFromCivil(+match[1], 1, 4);
    const mondayOfWeekOne = januaryFourth - weekdayFromDays(januaryFourth);
    return (mondayOfWeekOne - epochMonday) / 7 + (+match[2]) - 1;
  };
  const parseTime = value => {
    const match = /^(\d{2}):(\d{2})(?::(\d{2})(?:\.(\d{1,3}))?)?$/.exec(value);
    if (!match) return null;
    const fraction = match[4] || '';
    return (+match[1]) * 3600 + (+match[2]) * 60 + (+match[3] || 0)
      + (+fraction) / 10 ** fraction.length;
  };
  const parseLocalDateTime = value => {
    const match = /^(\d{4,6}-\d{2}-\d{2})[T ](\d{2}:\d{2}(?::\d{2}(?:\.\d{1,3})?)?)$/.exec(value);
    if (!match) return null;
    const days = parseDate(match[1]);
    const seconds = parseTime(match[2]);
    return days === null || seconds === null ? null : days * 86400 + seconds;
  };
  const parseValue = (type, value) => {
    if (value === null || value === '') return null;
    switch (type) {
      case 'number':
      case 'range': {
        const number = Number(value);
        return Number.isFinite(number) ? number : null;
      }
      case 'date': return parseDate(value);
      case 'month': return parseMonth(value);
      case 'week': return parseWeek(value);
      case 'time': return parseTime(value);
      case 'datetime-local': return parseLocalDateTime(value);
      default: return null;
    }
  };
  const STEP_DEFAULT = {
    date: 1, month: 1, week: 1, time: 60, 'datetime-local': 60, number: 1, range: 1,
  };
  const valueAsNumber = element => parseValue(inputType(element), element.value);
  const rangeUnderflow = element => {
    const value = valueAsNumber(element);
    const type = inputType(element);
    const min = parseValue(type, element.getAttribute('min'));
    const max = parseValue(type, element.getAttribute('max'));
    if (value === null || min === null) return false;
    // A reversed range (min greater than max) accepts everything outside the
    // open interval (max, min).
    if (max !== null && min > max) return value > max && value < min;
    return value < min;
  };
  const rangeOverflow = element => {
    const value = valueAsNumber(element);
    const type = inputType(element);
    const min = parseValue(type, element.getAttribute('min'));
    const max = parseValue(type, element.getAttribute('max'));
    if (value === null || max === null) return false;
    if (min !== null && min > max) return value > max && value < min;
    return value > max;
  };
  // Parses a decimal string into an exact `BigInt` scaled by a power of ten
  // (<https://html.spec.whatwg.org/multipage/common-microsyntaxes.html#rules-for-parsing-floating-point-number-values>).
  const parseDecimal = text => {
    const match = /^([+-]?)(\d*)(?:\.(\d*))?(?:[eE]([+-]?\d+))?$/.exec(text);
    if (match === null) return null;
    const integer = match[2] || '';
    const fraction = match[3] || '';
    if (integer === '' && fraction === '') return null;
    const sign = match[1] === '-' ? -1n : 1n;
    let value = BigInt(integer + fraction) * sign;
    let scale = fraction.length - (match[4] ? Number(match[4]) : 0);
    if (scale < 0) {
      value *= 10n ** BigInt(-scale);
      scale = 0;
    }
    return { value, scale };
  };
  const atScale = (decimal, scale) => decimal.value * 10n ** BigInt(scale - decimal.scale);

  const stepMismatch = element => {
    const type = inputType(element);
    const fallback = STEP_DEFAULT[type];
    if (fallback === undefined) return false;
    const rawStep = element.getAttribute('step');
    if (rawStep !== null && rawStep.trim().toLowerCase() === 'any') return false;
    if (type === 'number' || type === 'range') {
      // Exact decimal arithmetic: a very small step with a large value loses
      // the fraction in f64
      // (<https://html.spec.whatwg.org/multipage/input.html#the-step-attribute>).
      const value = parseDecimal(element.value);
      if (value === null) return false;
      let step = rawStep === null ? null : parseDecimal(rawStep.trim());
      if (step === null || step.value <= 0n) step = { value: 1n, scale: 0 };
      const minText = element.getAttribute('min');
      let min = minText === null ? null : parseDecimal(minText.trim());
      if (min === null) min = { value: 0n, scale: 0 };
      const scale = Math.max(value.scale, min.scale, step.scale);
      return (atScale(value, scale) - atScale(min, scale)) % atScale(step, scale) !== 0n;
    }
    const value = valueAsNumber(element);
    if (value === null) return false;
    let step = rawStep === null ? fallback : Number(rawStep);
    if (Number.isNaN(step) || step <= 0) step = fallback;
    const min = parseValue(type, element.getAttribute('min'));
    const remainder = Math.abs((value - (min === null ? 0 : min)) % step);
    if (remainder === 0) return false;
    return Math.min(remainder, step - remainder) > step * 1e-9;
  };

  const FLAGS = {
    valueMissing,
    typeMismatch,
    patternMismatch,
    tooLong,
    tooShort,
    rangeUnderflow,
    rangeOverflow,
    stepMismatch,
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
    // A barred control has no validation message.
    if (barredFromValidation(element)) return '';
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
    'HTMLObjectElement',
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

  // ── Text control selection ─────────────────────────────────────────────

  // Web IDL `unsigned long` conversion.
  const toUnsignedLong = value => {
    let number = Number(value);
    if (!Number.isFinite(number)) number = 0;
    number = Math.trunc(number);
    return ((number % 4294967296) + 4294967296) % 4294967296;
  };

  const DIRECTION = value =>
    value === 'forward' || value === 'backward' ? value : 'none';

  const selectionApplies = element =>
    element.tagName === 'TEXTAREA' ||
    (isInput(element) && SELECTION_TYPES.has(element.type));

  // "Set the selection range" algorithm
  // (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#set-the-selection-range>).
  // `direction` is optional; a missing or unknown value means "none".
  const applySelection = (element, start, end, direction) => {
    const length = element.value.length;
    let rangeStart = Math.min(toUnsignedLong(start), length);
    let rangeEnd = Math.min(toUnsignedLong(end), length);
    if (rangeEnd <= rangeStart) rangeStart = rangeEnd;
    element.selectionStart = rangeStart;
    element.selectionEnd = rangeEnd;
    element.selectionDirection = direction === undefined ? 'none' : DIRECTION(direction);
  };

  const requireSelection = element => {
    if (!selectionApplies(element)) {
      throw new DOMException(
        'The text selection APIs do not apply to this control', 'InvalidStateError');
    }
  };

  const selectionMethods = {
    setSelectionRange: {
      value: function(start, end, direction) {
        requireSelection(this);
        applySelection(this, start, end, direction);
      },
      writable: true, enumerable: true, configurable: true,
    },
    select: {
      value: function() {
        // A control with no selectable text is a no-op
        // (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-select>).
        if (!selectionApplies(this)) return;
        applySelection(this, 0, this.value.length, 'none');
      },
      writable: true, enumerable: true, configurable: true,
    },
    setRangeText: {
      value: function(replacement, start, end, selectionMode) {
        // No overload takes zero arguments, so the Web IDL layer throws before
        // the method body runs
        // (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-setrangetext>).
        if (arguments.length === 0) {
          throw new TypeError('setRangeText: at least 1 argument required');
        }
        requireSelection(this);
        replacement = String(replacement);
        let rangeStart;
        let rangeEnd;
        if (arguments.length === 1) {
          rangeStart = this.selectionStart;
          rangeEnd = this.selectionEnd;
        } else {
          rangeStart = toUnsignedLong(start);
          rangeEnd = toUnsignedLong(end);
        }
        if (rangeStart > rangeEnd) {
          throw new DOMException('start is greater than end', 'IndexSizeError');
        }
        const length = this.value.length;
        rangeStart = Math.min(rangeStart, length);
        rangeEnd = Math.min(rangeEnd, length);
        const selectionStart = this.selectionStart;
        const selectionEnd = this.selectionEnd;
        const mode = selectionMode === undefined ? 'preserve' : String(selectionMode);
        this.value =
          this.value.slice(0, rangeStart) + replacement + this.value.slice(rangeEnd);
        const newEnd = rangeStart + replacement.length;
        if (mode === 'select') {
          applySelection(this, rangeStart, newEnd, this.selectionDirection);
        } else if (mode === 'start') {
          applySelection(this, rangeStart, rangeStart, this.selectionDirection);
        } else if (mode === 'end') {
          applySelection(this, newEnd, newEnd, this.selectionDirection);
        } else {
          // "preserve": shift the old selection by the replacement's delta.
          const delta = replacement.length - (rangeEnd - rangeStart);
          let nextStart = selectionStart;
          let nextEnd = selectionEnd;
          if (nextStart > rangeEnd) nextStart += delta;
          else if (nextStart > rangeStart) nextStart = rangeStart;
          if (nextEnd > rangeEnd) nextEnd += delta;
          else if (nextEnd > rangeStart) nextEnd = newEnd;
          applySelection(this, nextStart, nextEnd, this.selectionDirection);
        }
      },
      writable: true, enumerable: true, configurable: true,
    },
  };

  for (const name of ['HTMLInputElement', 'HTMLTextAreaElement']) {
    const constructor = globalThis[name];
    if (!constructor || !constructor.prototype) continue;
    Object.defineProperties(constructor.prototype, selectionMethods);
  }

  // ── textarea sizing reflection ─────────────────────────────────────────

  // `[ReflectPositiveWithFallback]` unsigned long: an absent, invalid, or zero
  // attribute reports the fallback
  // (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-rows>).
  const positiveFallback = (name, fallback) => ({
    get: function() {
      const raw = this.getAttribute(name);
      if (raw === null) return fallback;
      const parsed = parseNonNegative(raw);
      return parsed === null || parsed === 0 ? fallback : parsed;
    },
    set: function(value) {
      this.setAttribute(name, String(toUnsignedLong(value)));
    },
    enumerable: true,
    configurable: true,
  });

  if (globalThis.HTMLTextAreaElement && globalThis.HTMLTextAreaElement.prototype) {
    Object.defineProperties(globalThis.HTMLTextAreaElement.prototype, {
      rows: positiveFallback('rows', 2),
      cols: positiveFallback('cols', 20),
      // `[Reflect]`; the enumerated missing value default is Soft
      // (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-wrap>).
      wrap: {
        get: function() {
          const raw = this.getAttribute('wrap');
          return raw === null ? 'soft' : raw;
        },
        set: function(value) {
          this.setAttribute('wrap', String(value));
        },
        enumerable: true,
        configurable: true,
      },
    });
  }

  // ── submit events and submission ───────────────────────────────────────

  const SUBMITTER = Symbol('tb-submit-submitter');
  const FORMDATA = Symbol('tb-formdata-event-data');

  // <https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#submitevent>
  function SubmitEvent(type) {
    if (new.target === undefined) {
      throw new TypeError('Class constructor SubmitEvent cannot be invoked without new');
    }
    const init = arguments[1];
    const event = Reflect.construct(globalThis.Event, arguments, new.target);
    event[SUBMITTER] =
      (init === undefined || init === null || init.submitter === undefined) ? null : init.submitter;
    return event;
  }
  const submitProto = Object.create(globalThis.Event.prototype);
  Object.defineProperty(submitProto, 'constructor', {
    value: SubmitEvent, writable: true, configurable: true,
  });
  Object.defineProperty(submitProto, 'submitter', {
    get() { return this[SUBMITTER]; }, enumerable: true, configurable: true,
  });
  Object.defineProperty(submitProto, Symbol.toStringTag, {
    value: 'SubmitEvent', writable: false, enumerable: false, configurable: true,
  });
  Object.defineProperty(SubmitEvent, 'prototype', { value: submitProto, writable: false });
  Object.defineProperty(globalThis, 'SubmitEvent', {
    value: SubmitEvent, writable: true, configurable: true,
  });

  // <https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#formdataevent>
  function FormDataEvent(type) {
    if (new.target === undefined) {
      throw new TypeError('Class constructor FormDataEvent cannot be invoked without new');
    }
    const init = arguments[1];
    if (init === undefined || init === null || init.formData === undefined) {
      throw new TypeError('FormDataEventInit requires formData');
    }
    const event = Reflect.construct(globalThis.Event, arguments, new.target);
    event[FORMDATA] = init.formData;
    return event;
  }
  const formDataProto = Object.create(globalThis.Event.prototype);
  Object.defineProperty(formDataProto, 'constructor', {
    value: FormDataEvent, writable: true, configurable: true,
  });
  Object.defineProperty(formDataProto, 'formData', {
    get() { return this[FORMDATA]; }, enumerable: true, configurable: true,
  });
  Object.defineProperty(formDataProto, Symbol.toStringTag, {
    value: 'FormDataEvent', writable: false, enumerable: false, configurable: true,
  });
  Object.defineProperty(FormDataEvent, 'prototype', { value: formDataProto, writable: false });
  Object.defineProperty(globalThis, 'FormDataEvent', {
    value: FormDataEvent, writable: true, configurable: true,
  });

  // application/x-www-form-urlencoded: LF is normalized to CRLF, a space
  // becomes `+`, and the `!'()~` set is percent-encoded
  // (<https://url.spec.whatwg.org/#concept-urlencoded-serializer>).
  // The submission encoding: the first label in `accept-charset`, defaulting
  // to UTF-8 (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#attr-fs-accept-charset>).
  const charsetLabel = form => {
    const accept = (form.acceptCharset || '').split(',')[0].trim();
    return accept === '' ? 'UTF-8' : accept;
  };
  // A string encoded in `label`, as bytes. `__tbEncodeForm` applies the
  // Encoding Standard's `encode`, so unrepresentable characters become numeric
  // character references.
  // A JS string with a lone surrogate cannot cross to Rust as UTF-8; replace
  // lone surrogates with U+FFFD, the same replacement the Encoding Standard's
  // `encode` applies
  // (<https://encoding.spec.whatwg.org/#encode>).
  const toWellFormed = text => {
    let out = '';
    for (let index = 0; index < text.length; index++) {
      const code = text.charCodeAt(index);
      if (code >= 0xD800 && code <= 0xDBFF) {
        const next = text.charCodeAt(index + 1);
        if (next >= 0xDC00 && next <= 0xDFFF) {
          out += text[index] + text[index + 1];
          index++;
        } else {
          out += '\uFFFD';
        }
      } else if (code >= 0xDC00 && code <= 0xDFFF) {
        out += '\uFFFD';
      } else {
        out += text[index];
      }
    }
    return out;
  };
  const charsetBytes = (text, label) => {
    const encoded = globalThis.__tbEncodeForm(toWellFormed(String(text)), label);
    const bytes = new Uint8Array(encoded.length);
    for (let index = 0; index < encoded.length; index++) {
      bytes[index] = encoded.charCodeAt(index);
    }
    return bytes;
  };
  // application/x-www-form-urlencoded over the encoded bytes: unreserved bytes
  // stay, a space becomes `+`, everything else is percent-encoded
  // (<https://url.spec.whatwg.org/#concept-urlencoded-serializer>).
  const UNRESERVED = byte =>
    (byte >= 0x30 && byte <= 0x39) || (byte >= 0x41 && byte <= 0x5A) ||
    (byte >= 0x61 && byte <= 0x7A) || byte === 0x2A || byte === 0x2D ||
    byte === 0x2E || byte === 0x5F;
  const percentEncode = bytes => {
    let out = '';
    for (const byte of bytes) {
      if (byte === 0x20) out += '+';
      else if (UNRESERVED(byte)) out += String.fromCharCode(byte);
      else out += `%${byte.toString(16).toUpperCase().padStart(2, '0')}`;
    }
    return out;
  };
  // A `File`/`Blob` entry serializes as its file name
  // (<https://url.spec.whatwg.org/#concept-urlencoded-serializer>).
  const entryValueString = value =>
    (value !== null && typeof value === 'object' && typeof value.name === 'string')
      ? value.name
      : String(value);
  const urlEncode = (formData, label) => {
    const parts = [];
    for (const entry of formData) {
      const name = String(entry[0]).replace(/\r\n|\r|\n/g, '\r\n');
      const value = entryValueString(entry[1]).replace(/\r\n|\r|\n/g, '\r\n');
      parts.push(
        `${percentEncode(charsetBytes(name, label))}=${percentEncode(charsetBytes(value, label))}`,
      );
    }
    return parts.join('&');
  };

  // multipart/form-data with a generated boundary
  // (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#multipart-form-data>).
  // The multipart name/filename escape: CR, LF, and `"` become percent
  // escapes (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#multipart-form-data>).
  const escapeMultipartName = value =>
    String(value)
      .replace(/\r\n|\r|\n/g, '\r\n')
      .replace(/\r/g, '%0D')
      .replace(/\n/g, '%0A')
      .replace(/"/g, '%22');
  // A filename's newlines are not normalized first, only escaped.
  const escapeMultipartFilename = value =>
    String(value)
      .replace(/\r/g, '%0D')
      .replace(/\n/g, '%0A')
      .replace(/"/g, '%22');
  // The navigation body crosses to Rust as a Latin-1 string, one char per
  // byte, so file bytes survive unchanged while text is UTF-8 encoded first.
  const encoder = new TextEncoder();
  const toLatin1 = bytes => {
    let text = '';
    for (let index = 0; index < bytes.length; index++) {
      text += String.fromCharCode(bytes[index]);
    }
    return text;
  };
  const concatBytes = chunks => {
    let total = 0;
    for (const chunk of chunks) total += chunk.length;
    const out = new Uint8Array(total);
    let offset = 0;
    for (const chunk of chunks) {
      out.set(chunk, offset);
      offset += chunk.length;
    }
    return out;
  };

  const encodeMultipart = (formData, label) => {
    const boundary = `----tinybrowser${Math.random().toString(16).slice(2)}`;
    const chunks = [];
    const push = text => chunks.push(encoder.encode(text));
    for (const entry of formData) {
      const name = escapeMultipartName(entry[0]);
      const value = entry[1];
      push(`--${boundary}\r\n`);
      if (value !== null && typeof value === 'object' && typeof value.name === 'string') {
        push(`Content-Disposition: form-data; name="`);
        chunks.push(charsetBytes(name, label));
        push(`"; filename="`);
        chunks.push(charsetBytes(escapeMultipartFilename(value.name), label));
        push(`"\r\nContent-Type: ${value.type || 'application/octet-stream'}\r\n\r\n`);
        const data = value[__tbBlobData];
        chunks.push(data === undefined ? encoder.encode(String(value)) : data.bytes);
        push('\r\n');
      } else {
        push(`Content-Disposition: form-data; name="`);
        chunks.push(charsetBytes(name, label));
        push(`"\r\n\r\n`);
        chunks.push(charsetBytes(String(value).replace(/\r\n|\r|\n/g, '\r\n'), label));
        push('\r\n');
      }
    }
    push(`--${boundary}--\r\n`);
    return {
      body: toLatin1(concatBytes(chunks)),
      contentType: `multipart/form-data; boundary=${boundary}`,
    };
  };

  const encodeTextPlain = (formData, label) => {
    let body = '';
    for (const entry of formData) {
      const name = charsetBytes(String(entry[0]).replace(/\r\n|\r|\n/g, '\r\n'), label);
      const value = charsetBytes(
        entryValueString(entry[1]).replace(/\r\n|\r|\n/g, '\r\n'),
        label,
      );
      body += `${toLatin1(name)}=${toLatin1(value)}\r\n`;
    }
    return body;
  };

  const methodKeyword = raw => {
    const value = String(raw).trim().toLowerCase();
    return (value === 'post' || value === 'dialog') ? value : 'get';
  };
  const enctypeKeyword = raw => {
    const value = String(raw).trim().toLowerCase();
    if (value === 'multipart/form-data' || value === 'text/plain') return value;
    return 'application/x-www-form-urlencoded';
  };
  // A submit button overrides its form's action/method/enctype/target through
  // its `form*` attributes
  // (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#form-submission-algorithm>).
  const submitterAttribute = (submitter, attribute) =>
    submitter !== undefined && submitter !== null && submitter.hasAttribute(attribute)
      ? submitter.getAttribute(attribute)
      : null;

  const submitForm = (form, formData, submitter) => {
    const methodAttr = submitterAttribute(submitter, 'formmethod');
    const method = methodAttr !== null ? methodKeyword(methodAttr) : form.method;
    if (method === 'dialog') return;
    const actionAttr = submitterAttribute(submitter, 'formaction');
    const action = actionAttr !== null
      ? new globalThis.URL(actionAttr, globalThis.document.URL).href
      : form.action;
    const targetAttr = submitterAttribute(submitter, 'formtarget');
    const target = targetAttr !== null ? targetAttr : form.target;
    const enctypeAttr = submitterAttribute(submitter, 'formenctype');
    const enctype = enctypeAttr !== null ? enctypeKeyword(enctypeAttr) : form.enctype;
    const label = charsetLabel(form);
    // A `_charset_` control carries the submission encoding's name
    // (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#attr-fe-name-charset>).
    if (formData.has('_charset_')) formData.set('_charset_', label);
    if (method === 'get') {
      let url = action;
      const encoded = urlEncode(formData, label);
      if (encoded !== '') {
        const hash = url.indexOf('#');
        const base = hash === -1 ? url : url.slice(0, hash);
        const query = base.indexOf('?');
        url = (query === -1 ? base : base.slice(0, query)) + '?' + encoded;
      }
      globalThis.__tbFormNavigate(url, target, 'GET', '', null);
      return;
    }
    // POST: the entries become the request body, encoded per `enctype`
    // (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#submit-mutate-action>).
    let body;
    let contentType;
    if (enctype === 'multipart/form-data') {
      ({ body, contentType } = encodeMultipart(formData, label));
    } else if (enctype === 'text/plain') {
      body = encodeTextPlain(formData, label);
      contentType = 'text/plain';
    } else {
      body = urlEncode(formData, label);
      contentType = 'application/x-www-form-urlencoded';
    }
    globalThis.__tbFormNavigate(action, target, 'POST', body, contentType);
  };

  // The submission algorithm's entry-list step: the FormData constructor
  // fires `formdata`, whose handler may extend the list, then we submit it.
  // `form.submit()` runs this without the `submit` event; `requestSubmit()`
  // fires that first
  // (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#constructing-form-data-set>).
  const runSubmission = (form, submitter) => {
    const formData = new globalThis.FormData(form);
    // A submitter contributes its own name/value to the submitted list.
    if (submitter !== undefined && submitter !== null) {
      const name = submitter.getAttribute('name');
      if (name) {
        formData.append(name, submitter.getAttribute('value') ?? '');
      }
    }
    submitForm(form, formData, submitter);
  };

  // <https://html.spec.whatwg.org/multipage/forms.html#dom-form-submit>
  Object.defineProperty(globalThis.HTMLFormElement.prototype, 'submit', {
    value: function() {
      runSubmission(this);
    },
    writable: true,
    enumerable: true,
    configurable: true,
  });

  // <https://html.spec.whatwg.org/multipage/forms.html#dom-form-checkvalidity>
  const formValidation = form => {
    let valid = true;
    for (const control of form.querySelectorAll('input, textarea, select, button')) {
      if (typeof control.checkValidity !== 'function') continue;
      if (!control.checkValidity()) valid = false;
    }
    return valid;
  };
  Object.defineProperties(globalThis.HTMLFormElement.prototype, {
    checkValidity: {
      value: function() { return formValidation(this); },
      writable: true, enumerable: true, configurable: true,
    },
    reportValidity: {
      value: function() { return formValidation(this); },
      writable: true, enumerable: true, configurable: true,
    },
  });

  // <https://html.spec.whatwg.org/multipage/forms.html#dom-form-requestsubmit>
  Object.defineProperty(globalThis.HTMLFormElement.prototype, 'requestSubmit', {
    value: function(submitter) {
      if (submitter !== undefined && submitter !== null) {
        const tag = submitter.tagName;
        if (tag !== 'BUTTON' && !(tag === 'INPUT' && submitter.type === 'submit')) {
          throw new TypeError('submitter must be a submit button');
        }
      }
      // Interactive validation runs before the submit event and can stop the
      // submission (<https://html.spec.whatwg.org/multipage/forms.html#interactively-validate-the-constraints>).
      if (!this.noValidate && !this.checkValidity()) return;
      const event = new SubmitEvent('submit', {
        submitter: submitter === undefined ? null : submitter,
        bubbles: true,
        cancelable: true,
      });
      if (!this.dispatchEvent(event)) return;
      runSubmission(this, submitter);
    },
    writable: true,
    enumerable: true,
    configurable: true,
  });

  // ── select helpers ─────────────────────────────────────────────────────

  // <https://html.spec.whatwg.org/multipage/form-elements.html#dom-option>
  function Option(text, value, defaultSelected, selected) {
    if (new.target === undefined) {
      throw new TypeError('Class constructor Option cannot be invoked without new');
    }
    const option = globalThis.document.createElement('option');
    if (text !== undefined) option.text = String(text);
    if (value !== undefined) option.value = String(value);
    if (defaultSelected !== undefined) option.defaultSelected = Boolean(defaultSelected);
    // The `selected` argument sets selectedness without the dirty flag, so a
    // later `selected` attribute change still updates it
    // (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-option>).
    globalThis.__tbSetOptionSelectedness(option, selected !== undefined && Boolean(selected));
    return option;
  }
  Object.defineProperty(Option, 'prototype', {
    value: globalThis.HTMLOptionElement.prototype, writable: false,
  });
  Object.defineProperty(globalThis, 'Option', {
    value: Option, writable: true, configurable: true,
  });

  Object.defineProperties(globalThis.HTMLSelectElement.prototype, {
    item: {
      value: function(index) {
        const option = this.options[index];
        return option === undefined ? null : option;
      },
      writable: true, enumerable: true, configurable: true,
    },
    namedItem: {
      value: function(name) {
        const key = String(name);
        for (const option of this.options) {
          if (option.id === key || option.getAttribute('name') === key) return option;
        }
        return null;
      },
      writable: true, enumerable: true, configurable: true,
    },
    // <https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-add>
    add: {
      value: function(element, before) {
        if (element === undefined) throw new TypeError('add requires an element');
        // Adding a node before itself is a no-op
        // (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-add>).
        if (element === before) return;
        if (before === undefined || before === null) {
          this.appendChild(element);
          return;
        }
        if (typeof before === 'number') {
          const options = this.options;
          const index = before < 0 ? options.length : before;
          const reference = options[index];
          if (reference === undefined) this.appendChild(element);
          else this.insertBefore(element, reference);
          return;
        }
        if (before.parentNode !== this) {
          throw new DOMException('reference is not a child of this select', 'NotFoundError');
        }
        this.insertBefore(element, before);
      },
      writable: true, enumerable: true, configurable: true,
    },
    remove: {
      value: function(index) {
        if (arguments.length === 0) {
          globalThis.Element.prototype.remove.call(this);
          return;
        }
        const option = this.options[Number(index)];
        if (option !== undefined) option.remove();
      },
      writable: true, enumerable: true, configurable: true,
    },
  });

  // ── input valueAsNumber / valueAsDate / stepUp / stepDown ──────────────

  const DATE_LIKE = new Set(['date', 'month', 'week', 'time', 'datetime-local']);

  const civilFromDays = days => {
    const z = days + 719468;
    const era = Math.floor((z >= 0 ? z : z - 146096) / 146097);
    const dayOfEra = z - era * 146097;
    const yearOfEra = Math.floor(
      (dayOfEra - Math.floor(dayOfEra / 1460) + Math.floor(dayOfEra / 36524)
        - Math.floor(dayOfEra / 146096)) / 365,
    );
    const year = yearOfEra + era * 400;
    const dayOfYear = dayOfEra - (365 * yearOfEra + Math.floor(yearOfEra / 4) - Math.floor(yearOfEra / 100));
    const mp = Math.floor((5 * dayOfYear + 2) / 153);
    const day = dayOfYear - Math.floor((153 * mp + 2) / 5) + 1;
    const month = mp + (mp < 10 ? 3 : -9);
    return { year: year + (month <= 2 ? 1 : 0), month, day };
  };
  const pad = (number, width) => String(number).padStart(width, '0');
  const formatDate = days => {
    const { year, month, day } = civilFromDays(days);
    return `${pad(year, 4)}-${pad(month, 2)}-${pad(day, 2)}`;
  };
  const formatMonth = index => {
    const months = index + 1970 * 12;
    return `${pad(Math.floor(months / 12), 4)}-${pad((months % 12) + 1, 2)}`;
  };
  const formatWeek = weeks => {
    const days = epochMonday + weeks * 7;
    const { year } = civilFromDays(days + 3);
    const januaryFourth = daysFromCivil(year, 1, 4);
    const mondayOfWeekOne = januaryFourth - weekdayFromDays(januaryFourth);
    return `${pad(year, 4)}-W${pad(Math.round((days - mondayOfWeekOne) / 7) + 1, 2)}`;
  };
  const formatTime = seconds => {
    const whole = Math.floor(seconds);
    const millis = Math.round((seconds - whole) * 1000);
    const base = `${pad(Math.floor(whole / 3600), 2)}:${pad(Math.floor((whole % 3600) / 60), 2)}`;
    if (millis === 0 && whole % 60 === 0) return base;
    const withSeconds = `${base}:${pad(whole % 60, 2)}`;
    return millis === 0 ? withSeconds : `${withSeconds}.${pad(millis, 3).replace(/0+$/, '')}`;
  };

  // The input's value as a number in each state's scale, or NaN
  // (<https://html.spec.whatwg.org/multipage/input.html#value-as-a-number>).
  const valueAsNumberOf = element => {
    const type = inputType(element);
    const value = element.value;
    if (value === '') return NaN;
    switch (type) {
      case 'number': case 'range': {
        const number = Number(value);
        return Number.isNaN(number) ? NaN : number;
      }
      case 'date': {
        const days = parseDate(value);
        return days === null ? NaN : days * 86400000;
      }
      case 'month': {
        const index = parseMonth(value);
        return index === null ? NaN : index;
      }
      case 'week': {
        const weeks = parseWeek(value);
        return weeks === null ? NaN : (epochMonday + weeks * 7) * 86400000;
      }
      case 'time': {
        const seconds = parseTime(value);
        return seconds === null ? NaN : seconds * 1000;
      }
      case 'datetime-local': {
        const seconds = parseLocalDateTime(value);
        return seconds === null ? NaN : seconds * 1000;
      }
      default: return NaN;
    }
  };

  const whole = value => Math.abs(value - Math.round(value)) < 1e-9;
  const formatValueAsNumber = (type, number) => {
    switch (type) {
      case 'number': case 'range': return String(number);
      case 'date': return whole(number / 86400000) ? formatDate(Math.round(number / 86400000)) : '';
      case 'month': return whole(number) ? formatMonth(number) : '';
      case 'week': {
        if (!whole(number / 86400000)) return '';
        return formatWeek((Math.round(number / 86400000) - epochMonday) / 7);
      }
      case 'time': {
        const normalized = ((number % 86400000) + 86400000) % 86400000;
        return formatTime(normalized / 1000);
      }
      case 'datetime-local': {
        const days = Math.floor(number / 86400000);
        return `${formatDate(days)}T${formatTime((number - days * 86400000) / 1000)}`;
      }
      default: return '';
    }
  };

  const setValueAsNumber = (element, value) => {
    const type = inputType(element);
    if (type !== 'number' && type !== 'range' && !DATE_LIKE.has(type)) {
      throw new DOMException('valueAsNumber is not applicable', 'InvalidStateError');
    }
    const number = Number(value);
    element.value = Number.isNaN(number) ? '' : formatValueAsNumber(type, number);
  };

  // `valueAsDate` applies to date, month, week, and time
  // (<https://html.spec.whatwg.org/multipage/input.html#dom-input-valueasdate>).
  const DATE_VALUE_TYPES = new Set(['date', 'month', 'week', 'time']);
  const valueAsDateOf = element => {
    const type = inputType(element);
    if (!DATE_VALUE_TYPES.has(type) || element.value === '') return null;
    let milliseconds = NaN;
    if (type === 'date') {
      const days = parseDate(element.value);
      if (days !== null) milliseconds = days * 86400000;
    } else if (type === 'month') {
      const index = parseMonth(element.value);
      if (index !== null) {
        const months = index + 1970 * 12;
        milliseconds = daysFromCivil(Math.floor(months / 12), (months % 12) + 1, 1) * 86400000;
      }
    } else if (type === 'week') {
      const weeks = parseWeek(element.value);
      if (weeks !== null) milliseconds = (epochMonday + weeks * 7) * 86400000;
    } else {
      const seconds = parseTime(element.value);
      if (seconds !== null) milliseconds = seconds * 1000;
    }
    return Number.isNaN(milliseconds) ? null : new Date(milliseconds);
  };
  const setValueAsDate = (element, value) => {
    const type = inputType(element);
    if (!DATE_VALUE_TYPES.has(type)) {
      throw new DOMException('valueAsDate is not applicable', 'InvalidStateError');
    }
    if (value === null) { element.value = ''; return; }
    if (!(value instanceof globalThis.Date)) {
      throw new TypeError('valueAsDate requires a Date');
    }
    const time = value.getTime();
    if (Number.isNaN(time)) { element.value = ''; return; }
    if (type === 'date') {
      element.value = whole(time / 86400000) ? formatDate(Math.round(time / 86400000)) : '';
    } else if (type === 'month') {
      const { year, month } = civilFromDays(Math.floor(time / 86400000));
      element.value = formatMonth((year - 1970) * 12 + month - 1);
    } else if (type === 'week') {
      element.value = formatWeek((Math.floor(time / 86400000) - epochMonday) / 7);
    } else {
      const normalized = ((time % 86400000) + 86400000) % 86400000;
      element.value = formatTime(normalized / 1000);
    }
  };

  // The value serialized in each state's `parseValue` unit, for `stepUp`.
  const formatValueInUnit = (type, value) => {
    switch (type) {
      case 'number': case 'range': return String(value);
      case 'date': return formatDate(value);
      case 'month': return formatMonth(value);
      case 'week': return formatWeek(value);
      case 'time': return formatTime(value);
      case 'datetime-local': {
        const days = Math.floor(value / 86400);
        return `${formatDate(days)}T${formatTime(value - days * 86400)}`;
      }
      default: return '';
    }
  };

  // Moves the value by `count` steps
  // (<https://html.spec.whatwg.org/multipage/input.html#dom-input-stepup>).
  const stepBy = (element, count, direction) => {
    const type = inputType(element);
    const fallback = STEP_DEFAULT[type];
    if (fallback === undefined) {
      throw new DOMException('stepUp is not applicable', 'InvalidStateError');
    }
    const raw = element.getAttribute('step');
    if (raw !== null && raw.trim().toLowerCase() === 'any') {
      throw new DOMException('step is any', 'InvalidStateError');
    }
    let step = raw === null ? fallback : Number(raw);
    if (Number.isNaN(step) || step <= 0) step = fallback;
    const min = parseValue(type, element.getAttribute('min'));
    const max = parseValue(type, element.getAttribute('max'));
    if (min !== null && max !== null && min > max) return;
    const base = min === null ? 0 : min;
    const alignUp = value => base + Math.ceil((value - base) / step - 1e-9) * step;
    const alignDown = value => base + Math.floor((value - base) / step + 1e-9) * step;
    // Nothing in [min, max] is representable: nothing to do.
    if (min !== null && max !== null && alignUp(min) > max + 1e-9) return;
    const parsed = parseValue(type, element.value);
    const wasEmpty = parsed === null;
    let value = parsed === null ? 0 : parsed;
    const valueBefore = value;
    const quotient = (value - base) / step;
    if (Math.abs(quotient - Math.round(quotient)) > 1e-9) {
      // Snap to the nearest representable value in the stepping direction.
      value = direction > 0 ? alignUp(value) : alignDown(value);
    } else {
      value += direction * step * count;
    }
    if (min !== null && value < min) value = alignUp(min);
    if (max !== null && value > max) value = alignDown(max);
    if (!wasEmpty) {
      if (direction < 0 && value > valueBefore) return;
      if (direction > 0 && value < valueBefore) return;
    }
    element.value = formatValueInUnit(type, value);
  };

  Object.defineProperties(globalThis.HTMLInputElement.prototype, {
    valueAsNumber: {
      get() { return valueAsNumberOf(this); },
      set(value) { setValueAsNumber(this, value); },
      configurable: true,
    },
    valueAsDate: {
      get() { return valueAsDateOf(this); },
      set(value) { setValueAsDate(this, value); },
      configurable: true,
    },
    stepUp: {
      value: function(count) { stepBy(this, count === undefined ? 1 : Number(count), 1); },
      writable: true, enumerable: true, configurable: true,
    },
    stepDown: {
      value: function(count) { stepBy(this, count === undefined ? 1 : Number(count), -1); },
      writable: true, enumerable: true, configurable: true,
    },
  });

  // The `list` attribute's datalist, or null
  // (<https://html.spec.whatwg.org/multipage/input.html#dom-input-list>).
  Object.defineProperty(globalThis.HTMLInputElement.prototype, 'list', {
    get() {
      const id = this.getAttribute('list');
      if (id === null) return null;
      const element = this.ownerDocument.getElementById(id);
      return element !== null && element.tagName === 'DATALIST' ? element : null;
    },
    configurable: true,
  });

  // ── form controls collection ───────────────────────────────────────────

  const LISTED = 'button, fieldset, input, object, output, select, textarea';

  // The form's listed elements, in tree order: every matching control whose
  // form owner is the form
  // (<https://html.spec.whatwg.org/multipage/forms.html#category-listed>).
  const listedElements = form =>
    Array.from(form.ownerDocument.querySelectorAll(LISTED))
      .filter(element => element.form === form);

  // Collection interfaces. The engine may already publish `HTMLCollection`;
  // reuse it so both brands share one prototype chain
  // (<https://html.spec.whatwg.org/multipage/forms.html#htmlformcontrolscollection>).
  const collectionInterface = name => {
    const existing = globalThis[name];
    if (existing !== undefined && existing !== null) return existing;
    const constructor = function() { throw new TypeError('Illegal constructor'); };
    Object.defineProperty(constructor.prototype, Symbol.toStringTag, {
      value: name, configurable: true,
    });
    Object.defineProperty(globalThis, name, {
      value: constructor, writable: true, configurable: true,
    });
    return constructor;
  };
  const HTMLCollectionInterface = collectionInterface('HTMLCollection');
  const FormControlsInterface = collectionInterface('HTMLFormControlsCollection');
  const RadioNodeListInterface = collectionInterface('RadioNodeList');
  Object.setPrototypeOf(FormControlsInterface.prototype, HTMLCollectionInterface.prototype);
  Object.setPrototypeOf(RadioNodeListInterface.prototype, HTMLCollectionInterface.prototype);

  // A live, named, indexed collection. `getList` recomputes on each access so
  // the collection tracks DOM changes; the proxy's prototype reports the right
  // interface for `instanceof`.
  const collectionOf = (getList, prototype, radioList) =>
    new Proxy({}, {
      get(_target, property) {
        const list = getList();
        if (property === 'length') return list.length;
        if (property === 'item') return index => list[Number(index)] ?? null;
        if (property === 'namedItem') return name => namedItemIn(getList(), String(name), prototype);
        if (property === 'value' && radioList) {
          const checked = list.find(element => element.type === 'radio' && element.checked);
          return checked ? checked.value : '';
        }
        if (property === Symbol.iterator) return function* () { yield* list; };
        if (typeof property === 'string') {
          if (/^\d+$/.test(property)) return list[Number(property)];
          const named = namedItemIn(list, property, prototype);
          return named === null ? undefined : named;
        }
        return undefined;
      },
      getPrototypeOf() { return prototype; },
      has(_target, property) {
        const list = getList();
        if (property === 'length' || property === 'item' || property === 'namedItem') return true;
        if (typeof property === 'string' && /^\d+$/.test(property)) return list[Number(property)] !== undefined;
        return namedItemIn(list, String(property), prototype) !== null;
      },
    });

  const namedItemIn = (list, name, prototype) => {
    const matches = list.filter(
      element => element.id === name || element.getAttribute('name') === name,
    );
    if (matches.length === 0) return null;
    if (matches.length === 1) return matches[0];
    return collectionOf(() => matches, prototype, true);
  };

  const elementsCache = new WeakMap();
  Object.defineProperties(globalThis.HTMLFormElement.prototype, {
    elements: {
      get() {
        let collection = elementsCache.get(this);
        if (collection === undefined) {
          collection = collectionOf(
            () => listedElements(this), FormControlsInterface.prototype, false,
          );
          elementsCache.set(this, collection);
        }
        return collection;
      },
      configurable: true,
    },
    length: {
      get() { return listedElements(this).length; },
      configurable: true,
    },
  });
})();
