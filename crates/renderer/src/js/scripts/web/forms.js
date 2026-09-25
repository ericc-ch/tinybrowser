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
})();
