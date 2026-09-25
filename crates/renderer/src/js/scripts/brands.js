(function() {
  const native = globalThis.Node.prototype;
  const inputFilesSymbol = Symbol.for('tinybrowser.input.files');
  function illegal() { throw new TypeError('Illegal constructor'); }
  function define(name, parent, members, constructible) {
    const ctor = constructible
      ? function() {
          const value = globalThis.__tb_construct.apply(
            globalThis,
            [name].concat(Array.prototype.slice.call(arguments))
          );
          if (name === 'HTMLElement' && new.target && new.target.prototype) {
            Object.setPrototypeOf(value, new.target.prototype);
          }
          return value;
        }
      : function() { return illegal(); };
    const proto = Object.create(parent ? parent.prototype : Object.prototype);
    for (const member of members) {
      const descriptor = Object.getOwnPropertyDescriptor(native, member);
      if (descriptor) {
        descriptor.configurable = true;
        Object.defineProperty(proto, member, descriptor);
      }
    }
    Object.defineProperty(ctor, 'name', { value: name, configurable: true });
    Object.defineProperty(proto, 'constructor', { value: ctor, writable: true, configurable: true });
    Object.defineProperty(ctor, 'prototype', { value: proto, writable: false });
    // Interface prototypes carry @@toStringTag
    // (<https://webidl.spec.whatwg.org/#es-interface>), so
    // Object.prototype.toString says `[object HTMLDivElement]`.
    Object.defineProperty(proto, Symbol.toStringTag, {
      value: name, writable: false, enumerable: false, configurable: true,
    });
    Object.defineProperty(globalThis, name, { value: ctor, writable: true, configurable: true });
    return ctor;
  }
  const EventTargetInterface = globalThis.EventTarget;
  const NodeInterface = define('Node', EventTargetInterface, [
    'addEventListener', 'removeEventListener', 'dispatchEvent',
    'nodeType', 'nodeName', 'firstChild', 'lastChild', 'nextSibling',
    'previousSibling', 'parentNode', 'childNodes', 'appendChild',
    'ownerDocument', 'hasChildNodes', 'nodeValue', 'textContent', 'isSameNode',
    'isEqualNode', 'contains', 'getRootNode', 'isConnected', 'cloneNode',
    'insertBefore', 'removeChild', 'replaceChild', 'lookupNamespaceURI',
    'lookupPrefix', 'isDefaultNamespace', 'normalize',
    'compareDocumentPosition', 'parentElement'
  ]);
  for (const [name, value] of [
    ['ELEMENT_NODE', 1],
    ['ATTRIBUTE_NODE', 2],
    ['TEXT_NODE', 3],
    ['CDATA_SECTION_NODE', 4],
    ['ENTITY_REFERENCE_NODE', 5],
    ['ENTITY_NODE', 6],
    ['PROCESSING_INSTRUCTION_NODE', 7],
    ['COMMENT_NODE', 8],
    ['DOCUMENT_NODE', 9],
    ['DOCUMENT_TYPE_NODE', 10],
    ['DOCUMENT_FRAGMENT_NODE', 11],
    ['NOTATION_NODE', 12],
    ['DOCUMENT_POSITION_DISCONNECTED', 1],
    ['DOCUMENT_POSITION_PRECEDING', 2],
    ['DOCUMENT_POSITION_FOLLOWING', 4],
    ['DOCUMENT_POSITION_CONTAINS', 8],
    ['DOCUMENT_POSITION_CONTAINED_BY', 16],
    ['DOCUMENT_POSITION_IMPLEMENTATION_SPECIFIC', 32],
  ]) {
    Object.defineProperty(NodeInterface, name, { value, writable: false, configurable: true });
    Object.defineProperty(NodeInterface.prototype, name, { value, writable: false, configurable: true });
  }
  const DocumentInterface = define('Document', NodeInterface, [
    'createElement', 'createElementNS', 'createTextNode', 'createComment',
    'createProcessingInstruction', 'createCDATASection', 'createAttribute',
    'createAttributeNS', 'createDocumentFragment', 'createEvent', 'open', 'write', 'close',
    'getElementById', 'getElementsByTagName',
    'getElementsByTagNameNS', 'getElementsByClassName',
    'body', 'head', 'documentElement', 'doctype', 'readyState', 'implementation',
    'children', 'firstElementChild', 'lastElementChild', 'childElementCount',
    'append', 'prepend', 'replaceChildren', 'querySelector', 'querySelectorAll',
    'URL', 'documentURI', 'baseURI', 'location', 'characterSet', 'charset',
    'inputEncoding', 'contentType', 'compatMode', 'title',
    'getElementsByName', 'importNode', 'currentScript', 'activeElement',
    'elementFromPoint', 'elementsFromPoint', 'defaultView', 'hasFocus'
  ], true);
  const ElementInterface = define('Element', NodeInterface, [
    'getElementsByTagName', 'getElementsByTagNameNS', 'getElementsByClassName',
    'getAttribute',
    'setAttribute', 'hasAttribute', 'getAttributeNS', 'setAttributeNS',
    'hasAttributeNS', 'removeAttribute', 'removeAttributeNS',
    'getAttributeNames', 'toggleAttribute', 'attributes', 'hasAttributes',
    'getAttributeNode', 'getAttributeNodeNS', 'setAttributeNode',
    'setAttributeNodeNS', 'removeAttributeNode', 'matches', 'closest',
    'children',
    'firstElementChild', 'lastElementChild', 'childElementCount', 'append',
    'prepend', 'replaceChildren', 'querySelector', 'querySelectorAll',
    'before', 'after', 'replaceWith', 'previousElementSibling',
    'nextElementSibling', 'tagName', 'localName', 'prefix', 'namespaceURI',
    'className', 'classList', 'dataset', 'id', 'src', 'href', 'name', 'content', 'outerHTML', 'innerHTML', 'style',
    'remove', 'getBoundingClientRect', 'getClientRects', 'scrollIntoView',
    'insertAdjacentHTML',
    'scrollLeft', 'scrollTop',
    'attachShadow', 'shadowRoot'
  ]);
  // classList is `[PutForwards=value]`: assigning to it sets `.value`
  // (<https://dom.spec.whatwg.org/#dom-element-classlist>).
  {
    const descriptor = Object.getOwnPropertyDescriptor(ElementInterface.prototype, 'classList');
    Object.defineProperty(ElementInterface.prototype, 'classList', {
      get: descriptor.get,
      set: function(value) { this.classList.value = value; },
      enumerable: true,
      configurable: true,
    });
  }
  const XMLDocumentInterface = define('XMLDocument', DocumentInterface, []);
  const CharacterDataInterface = define('CharacterData', NodeInterface, [
    'data', 'length', 'substringData', 'appendData', 'insertData', 'deleteData',
    'replaceData', 'remove', 'before', 'after', 'replaceWith',
    'previousElementSibling', 'nextElementSibling'
  ]);
  const TextInterface = define('Text', CharacterDataInterface, [], true);
  const CDATASectionInterface = define('CDATASection', TextInterface, []);
  const ProcessingInstructionInterface = define('ProcessingInstruction', CharacterDataInterface, [
    'target'
  ]);
  const CommentInterface = define('Comment', CharacterDataInterface, [], true);
  const DocumentTypeInterface = define('DocumentType', NodeInterface, [
    'remove', 'before', 'after', 'replaceWith', 'name', 'publicId', 'systemId'
  ]);
  const DocumentFragmentInterface = define('DocumentFragment', NodeInterface, [
    'children', 'firstElementChild', 'lastElementChild', 'childElementCount',
    'append', 'prepend', 'replaceChildren', 'querySelector', 'querySelectorAll',
    'getElementById'
  ], true);
  const ShadowRootInterface = define('ShadowRoot', DocumentFragmentInterface, [
    'host', 'mode', 'innerHTML', 'activeElement'
  ]);
  const HTMLElementInterface = define('HTMLElement', ElementInterface, ['click', 'focus', 'blur'], true);
  const HTMLUnknownElementInterface = define('HTMLUnknownElement', HTMLElementInterface, []);
  const HTMLMediaElementInterface = define('HTMLMediaElement', HTMLElementInterface, []);
  const SVGElementInterface = define('SVGElement', ElementInterface, ['click', 'focus', 'blur']);
  const table = {
    Document: DocumentInterface.prototype,
    XMLDocument: XMLDocumentInterface.prototype,
    Element: ElementInterface.prototype,
    CharacterData: CharacterDataInterface.prototype,
    Text: TextInterface.prototype,
    CDATASection: CDATASectionInterface.prototype,
    ProcessingInstruction: ProcessingInstructionInterface.prototype,
    Comment: CommentInterface.prototype,
    DocumentType: DocumentTypeInterface.prototype,
    DocumentFragment: DocumentFragmentInterface.prototype,
    ShadowRoot: ShadowRootInterface.prototype,
    HTMLElement: HTMLElementInterface.prototype,
    HTMLUnknownElement: HTMLUnknownElementInterface.prototype,
    HTMLMediaElement: HTMLMediaElementInterface.prototype,
    SVGElement: SVGElementInterface.prototype,
  };
  // Every element interface chains to HTMLElement except the media pair,
  // which chains through HTMLMediaElement, and SVG, which chains to Element.
  // Per-interface members copied from the native wrapper prototype.
  // `type` is defined per interface below; the form-control states live
  // here so each interface exposes exactly the reflecting attributes the
  // spec gives it (<https://html.spec.whatwg.org/#the-disabled-attribute>).
  const interfaceMembers = {
    HTMLIFrameElement: ['contentDocument', 'contentWindow'],
    HTMLImageElement: ['naturalWidth', 'naturalHeight', 'complete', 'currentSrc'],
    HTMLFormElement: ['reset', 'action', 'method', 'enctype', 'encoding', 'target', 'noValidate', 'acceptCharset'],
    HTMLInputElement: ['value', 'defaultValue', 'disabled', 'readOnly', 'required', 'multiple',
      'checked', 'defaultChecked', 'selectionStart', 'selectionEnd', 'selectionDirection',
      'indeterminate',
      'formAction', 'formMethod', 'formEnctype', 'formTarget', 'formNoValidate',
      'pattern', 'min', 'max', 'step'],
    HTMLTextAreaElement: ['value', 'defaultValue', 'textLength', 'disabled', 'readOnly', 'required',
      'selectionStart', 'selectionEnd', 'selectionDirection'],
    HTMLOptionElement: ['value', 'selected', 'defaultSelected', 'text', 'index', 'disabled',
      'label'],
    HTMLSelectElement: ['value', 'selectedIndex', 'options', 'length', 'multiple', 'disabled',
      'required', 'name', 'selectedOptions'],
    HTMLButtonElement: ['disabled',
      'formAction', 'formMethod', 'formEnctype', 'formTarget', 'formNoValidate'],
    HTMLFieldSetElement: ['disabled'],
    HTMLOptGroupElement: ['disabled'],
  };
  for (const [name, parent] of [
    ['HTMLAnchorElement', HTMLElementInterface],
    ['HTMLAreaElement', HTMLElementInterface],
    ['HTMLAudioElement', HTMLMediaElementInterface],
    ['HTMLBaseElement', HTMLElementInterface],
    ['HTMLBodyElement', HTMLElementInterface],
    ['HTMLBRElement', HTMLElementInterface],
    ['HTMLButtonElement', HTMLElementInterface],
    ['HTMLCanvasElement', HTMLElementInterface],
    ['HTMLDataElement', HTMLElementInterface],
    ['HTMLDataListElement', HTMLElementInterface],
    ['HTMLDialogElement', HTMLElementInterface],
    ['HTMLDirectoryElement', HTMLElementInterface],
    ['HTMLDivElement', HTMLElementInterface],
    ['HTMLDListElement', HTMLElementInterface],
    ['HTMLEmbedElement', HTMLElementInterface],
    ['HTMLFieldSetElement', HTMLElementInterface],
    ['HTMLFontElement', HTMLElementInterface],
    ['HTMLFormElement', HTMLElementInterface],
    ['HTMLFrameElement', HTMLElementInterface],
    ['HTMLFrameSetElement', HTMLElementInterface],
    ['HTMLHeadingElement', HTMLElementInterface],
    ['HTMLHeadElement', HTMLElementInterface],
    ['HTMLHRElement', HTMLElementInterface],
    ['HTMLHtmlElement', HTMLElementInterface],
    ['HTMLIFrameElement', HTMLElementInterface],
    ['HTMLImageElement', HTMLElementInterface],
    ['HTMLInputElement', HTMLElementInterface],
    ['HTMLLabelElement', HTMLElementInterface],
    ['HTMLLIElement', HTMLElementInterface],
    ['HTMLLegendElement', HTMLElementInterface],
    ['HTMLLinkElement', HTMLElementInterface],
    ['HTMLMapElement', HTMLElementInterface],
    ['HTMLMetaElement', HTMLElementInterface],
    ['HTMLMeterElement', HTMLElementInterface],
    ['HTMLModElement', HTMLElementInterface],
    ['HTMLObjectElement', HTMLElementInterface],
    ['HTMLOListElement', HTMLElementInterface],
    ['HTMLOptGroupElement', HTMLElementInterface],
    ['HTMLOptionElement', HTMLElementInterface],
    ['HTMLOutputElement', HTMLElementInterface],
    ['HTMLParagraphElement', HTMLElementInterface],
    ['HTMLParamElement', HTMLElementInterface],
    ['HTMLPreElement', HTMLElementInterface],
    ['HTMLProgressElement', HTMLElementInterface],
    ['HTMLQuoteElement', HTMLElementInterface],
    ['HTMLScriptElement', HTMLElementInterface],
    ['HTMLSelectElement', HTMLElementInterface],
    ['HTMLSourceElement', HTMLElementInterface],
    ['HTMLSlotElement', HTMLElementInterface],
    ['HTMLSpanElement', HTMLElementInterface],
    ['HTMLStyleElement', HTMLElementInterface],
    ['HTMLTableCaptionElement', HTMLElementInterface],
    ['HTMLTableCellElement', HTMLElementInterface],
    ['HTMLTableColElement', HTMLElementInterface],
    ['HTMLTableElement', HTMLElementInterface],
    ['HTMLTableRowElement', HTMLElementInterface],
    ['HTMLTableSectionElement', HTMLElementInterface],
    ['HTMLTemplateElement', HTMLElementInterface],
    ['HTMLTextAreaElement', HTMLElementInterface],
    ['HTMLTimeElement', HTMLElementInterface],
    ['HTMLTitleElement', HTMLElementInterface],
    ['HTMLTrackElement', HTMLElementInterface],
    ['HTMLUListElement', HTMLElementInterface],
    ['HTMLVideoElement', HTMLMediaElementInterface],
  ]) {
    table[name] = define(name, parent, interfaceMembers[name] ?? []).prototype;
  }
  // URL decomposition IDL attributes
  // (<https://html.spec.whatwg.org/multipage/links.html#url-decomposition-idl-attributes>).
  // A href that fails to parse makes `protocol` ":" and every other getter
  // empty; setting a component that the parse or the component rejects
  // leaves the attribute untouched.
  {
    const parts = {
      protocol: 0, username: 1, password: 2, host: 3, hostname: 4,
      port: 5, pathname: 6, search: 7, hash: 8, origin: 9,
    };
    const hrefValue = function() {
      const value = this.getAttribute('href');
      return value === null ? null : globalThis.__tbUSVString(value);
    };
    const base = function() { return globalThis.__tbUSVString(document.baseURI); };
    for (const proto of [table.HTMLAnchorElement, table.HTMLAreaElement]) {
      for (const name of Object.keys(parts)) {
        const index = parts[name];
        const descriptor = {
          get: function() {
            // An absent `href` reports the URL-decomposition defaults, not
            // the document URL.
            const href = hrefValue.call(this);
            if (href === null) return name === 'protocol' ? ':' : '';
            const values = __tbUrlParts(href, base.call(this));
            if (values === null || values === undefined) {
              return name === 'protocol' ? ':' : '';
            }
            return values[index];
          },
          enumerable: true,
          configurable: true,
        };
        if (name !== 'origin') {
          descriptor.set = function(value) {
            // Setting a component with no `href` attribute is a no-op.
            const href = hrefValue.call(this);
            if (href === null) return;
            const result = __tbUrlSetPart(
              href, base.call(this), index, globalThis.__tbUSVString(value));
            if (result !== null && result !== undefined) this.setAttribute('href', result);
          };
        }
        Object.defineProperty(proto, name, descriptor);
      }
    }
  }
  // `type` reflects the content attribute, limited to only known values
  // (<https://html.spec.whatwg.org/multipage/common-dom-interfaces.html#limited-to-only-known-values>,
  // <https://html.spec.whatwg.org/multipage/input.html#dom-input-type>,
  // <https://html.spec.whatwg.org/multipage/form-elements.html#dom-button-type>).
  function reflectType(keywords, fallback) {
    return {
      get: function() {
        const value = this.getAttribute('type');
        if (value === null) return fallback;
        const lowered = String(value).toLowerCase();
        return keywords.has(lowered) ? lowered : fallback;
      },
      set: function(value) { this.setAttribute('type', String(value)); },
      enumerable: true,
      configurable: true,
    };
  }
  const INPUT_TYPE_KEYWORDS = new Set([
    'hidden', 'text', 'search', 'tel', 'url', 'email', 'password',
    'date', 'month', 'week', 'time', 'datetime-local', 'number', 'range',
    'color', 'checkbox', 'radio', 'file', 'submit', 'image', 'reset', 'button',
  ]);
  Object.defineProperty(table.HTMLInputElement, 'type', {
    get: function() {
      const value = this.getAttribute('type');
      if (value === null) return 'text';
      const lowered = String(value).toLowerCase();
      return INPUT_TYPE_KEYWORDS.has(lowered) ? lowered : 'text';
    },
    set: function(value) {
      // The type-change steps run before the attribute lands, so the old state
      // can migrate its value
      // (<https://html.spec.whatwg.org/multipage/input.html#the-input-element:type-change-state>).
      const lowered = String(value).toLowerCase();
      const next = INPUT_TYPE_KEYWORDS.has(lowered) ? lowered : 'text';
      globalThis.__tbInputTypeChange(this, next);
      this.setAttribute('type', String(value));
    },
    enumerable: true,
    configurable: true,
  });
  // `<input type=file>` exposes a (possibly empty) FileList
  // (<https://html.spec.whatwg.org/multipage/input.html#dom-input-files>).
  Object.defineProperty(table.HTMLInputElement, 'files', {
    get: function() {
      let list = this[inputFilesSymbol];
      if (list === undefined) {
        list = globalThis.__tbCreateFileList([]);
        Object.defineProperty(this, inputFilesSymbol, {
          value: list, writable: false, enumerable: false, configurable: false,
        });
      }
      return list;
    },
    set: function(value) {
      // `input.files = fileList` replaces the list a script set
      // (<https://html.spec.whatwg.org/multipage/input.html#dom-input-files>).
      Object.defineProperty(this, inputFilesSymbol, {
        value: value, writable: true, enumerable: false, configurable: true,
      });
      globalThis.__tbSetInputFiles(this, value);
    },
    enumerable: true,
    configurable: true,
  });
  Object.defineProperty(table.HTMLButtonElement, 'type', reflectType(new Set([
    'submit', 'reset', 'button',
  ]), 'submit'));
  // <https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-type>
  Object.defineProperty(table.HTMLTextAreaElement, 'type', {
    get: function() { return 'textarea'; },
    enumerable: true,
    configurable: true,
  });
  Object.defineProperties(table.HTMLSlotElement, {
    assignedNodes: {
      value: function() {
        const host = this.getRootNode()?.host;
        if (!host) return [];
        const name = this.getAttribute('name') || '';
        return Array.from(host.childNodes).filter(node =>
          node.nodeType !== 1 ? name === '' : (node.getAttribute('slot') || '') === name);
      },
      writable: true, configurable: true, enumerable: true,
    },
    assignedElements: {
      value: function() { return this.assignedNodes().filter(node => node.nodeType === 1); },
      writable: true, configurable: true, enumerable: true,
    },
  });
  Object.defineProperty(globalThis, '__tb_brandTable', {
    enumerable: false,
    configurable: true,
    value: table,
  });
})();
