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
  // The text-selection APIs apply only to these input types (plus textarea);
  // notably email and number are excluded
  // (<https://html.spec.whatwg.org/multipage/input.html#do-not-apply>).
  const SELECTION_TYPES = new Set(['text', 'search', 'tel', 'url', 'password']);
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
        if (this.tagName === 'INPUT' && !isTextControl(this)) return;
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

  // application/x-www-form-urlencoded: LF is normalized to CRLF and a space
  // becomes `+` (<https://url.spec.whatwg.org/#concept-urlencoded-serializer>).
  const urlEncodePart = value =>
    encodeURIComponent(value.replace(/\r\n|\r|\n/g, '\r\n')).replace(/%20/g, '+');
  const urlEncode = formData => {
    const parts = [];
    for (const entry of formData) {
      parts.push(`${urlEncodePart(String(entry[0]))}=${urlEncodePart(String(entry[1]))}`);
    }
    return parts.join('&');
  };

  const submitForm = (form, formData) => {
    const method = form.method;
    if (method === 'dialog') return;
    let action = form.action;
    if (method === 'get') {
      const encoded = urlEncode(formData);
      if (encoded !== '') {
        const hash = action.indexOf('#');
        const base = hash === -1 ? action : action.slice(0, hash);
        const query = base.indexOf('?');
        action = (query === -1 ? base : base.slice(0, query)) + '?' + encoded;
      }
    }
    // POST needs the request-body protocol; submit as GET until it lands
    // (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#submit-mutate-action>).
    globalThis.__tbFormNavigate(action, form.target);
  };

  // <https://html.spec.whatwg.org/multipage/forms.html#dom-form-submit>
  Object.defineProperty(globalThis.HTMLFormElement.prototype, 'submit', {
    value: function() {
      submitForm(this, new globalThis.FormData(this));
    },
    writable: true,
    enumerable: true,
    configurable: true,
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
      const formData = new globalThis.FormData(this);
      const event = new SubmitEvent('submit', {
        submitter: submitter === undefined ? null : submitter,
        bubbles: true,
        cancelable: true,
      });
      if (!this.dispatchEvent(event)) return;
      this.dispatchEvent(new FormDataEvent('formdata', { formData: formData }));
      submitForm(this, formData);
    },
    writable: true,
    enumerable: true,
    configurable: true,
  });
})();
