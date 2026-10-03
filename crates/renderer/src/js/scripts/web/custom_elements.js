// Autonomous custom elements backed by the agent's native reaction stack.
// https://html.spec.whatwg.org/multipage/custom-elements.html#custom-elements-core-concepts
{
  const definitions = new __tbPrivateMap();
  const constructors = new __tbPrivateMap();
  const upgraded = host.slots();
  const connected = host.slots();
  // Shadow roots keyed by host. `host.shadowRoot` is null in closed mode, but
  // lifecycle traversal still has to reach those descendants.
  const shadowRoots = host.slots();
  const pending = new __tbPrivateMap();
  let definitionRunning = false;
  const enqueueReaction = host.__tbEnqueueCustomReaction;
  const pushReactions = host.__tbPushCustomReactions;
  const popReactions = host.__tbPopCustomReactions;
  const nativeApply = Reflect.apply;
  const nativeConstruct = Reflect.construct;
  const MapConstructor = Map;
  const mapGet = Map.prototype.get;
  const mapSet = Map.prototype.set;
  const mapHas = Map.prototype.has;
  const arrayIncludes = Array.prototype.includes;
  const createObject = Object.create;
  const arrayFrom = Array.from;
  const observe = MutationObserver.prototype.observe;
  const callbackNames = ['connectedCallback', 'disconnectedCallback', 'connectedMoveCallback', 'adoptedCallback', 'attributeChangedCallback'];
  const getAttribute = Element.prototype.getAttribute;
  const getAttributeNS = Element.prototype.getAttributeNS;
  const hasAttribute = Element.prototype.hasAttribute;
  const localNameGetter = Object.getOwnPropertyDescriptor(Element.prototype, 'localName').get;
  const connectedGetter = Object.getOwnPropertyDescriptor(Node.prototype, 'isConnected').get;
  const nodeTypeGetter = Object.getOwnPropertyDescriptor(Node.prototype, 'nodeType').get;
  const firstChildGetter = Object.getOwnPropertyDescriptor(Node.prototype, 'firstChild').get;
  const nextSiblingGetter = Object.getOwnPropertyDescriptor(Node.prototype, 'nextSibling').get;
  const shadowRootGetter = Object.getOwnPropertyDescriptor(Element.prototype, 'shadowRoot').get;
  const listLengthGetter = Object.getOwnPropertyDescriptor(NodeList.prototype, 'length').get;
  const listItem = NodeList.prototype.item;
  const recordGetters = Object.create(null);
  for (const name of ['type', 'target', 'removedNodes', 'addedNodes', 'attributeName', 'attributeNamespace', 'oldValue']) {
    recordGetters[name] = Object.getOwnPropertyDescriptor(MutationRecord.prototype, name).get;
  }

  function localNameOf(element) {
    return nativeApply(localNameGetter, element, []);
  }

  function isConnected(element) {
    return nativeApply(connectedGetter, element, []);
  }

  function shadowRootOf(host) {
    return nativeApply(shadowRootGetter, host, []) || shadowRoots.get(host) || null;
  }

  // Shadow-including element descendants, in tree order. A host's shadow tree
  // is visited before its light children
  // (<https://dom.spec.whatwg.org/#concept-shadow-including-tree-order>).
  // Fragments and shadow roots walk their element children.
  function visitElements(root, callback) {
    if (!root) return;
    const type = nativeApply(nodeTypeGetter, root, []);
    if (type === 1) {
      callback(root);
      const shadow = shadowRootOf(root);
      if (shadow) visitElements(shadow, callback);
    } else if (type !== 9 && type !== 11) {
      return;
    }
    for (let child = nativeApply(firstChildGetter, root, []); child; child = nativeApply(nextSiblingGetter, child, [])) {
      visitElements(child, callback);
    }
  }

  // Connection reactions: fire once per connection transition, whatever
  // mutation path caused it. The WeakSet makes the synchronous mutation
  // hooks and the MutationObserver idempotent.
  function notifyConnected(root) {
    visitElements(root, function(element) {
      if (!upgraded.has(element) || connected.has(element) || !isConnected(element)) return;
      connected.add(element);
      invoke(element, 'connectedCallback', []);
    });
  }

  function notifyDisconnected(root) {
    visitElements(root, function(element) {
      if (!upgraded.has(element) || !connected.has(element)) return;
      connected.delete(element);
      invoke(element, 'disconnectedCallback', []);
    });
  }

  function validName(name) {
    return typeof name === 'string' && name.includes('-') &&
      name === name.toLowerCase() && /^[a-z][.0-9_a-z\-]*$/.test(name);
  }

  function invoke(element, name, args) {
    const callback = definitions.get(localNameOf(element))?.callbacks[name];
    if (typeof callback === 'function') {
      enqueueReaction(element, callback, args);
    }
  }

  function deliverUpgrade(definition) {
    upgradeElement(this, definition);
  }

  function enqueueUpgrade(element, definition) {
    if (upgraded.has(element)) return;
    enqueueReaction(element, deliverUpgrade, [definition]);
  }

  function upgradeElement(element, definition) {
    if (upgraded.has(element) || localNameOf(element) !== definition.name) return;
    upgraded.add(element);
    nativeApply(observe, observer, [element, observerOptions]);
    try {
      host.__tbPushCustomConstruction(element);
      let constructed;
      try {
        constructed = nativeConstruct(definition.constructor, [], definition.constructor);
      } finally {
        // A constructor that fails before `super()` must not leave its
        // candidate on the construction stack for the next upgrade.
        host.__tbDiscardCustomConstruction(element);
      }
      if (constructed !== element) throw new TypeError('custom element constructor returned another object');
      for (const name of definition.observed) {
        if (nativeApply(hasAttribute, element, [name])) {
          invoke(element, 'attributeChangedCallback', [name, null, nativeApply(getAttribute, element, [name]), null]);
        }
      }
      if (isConnected(element)) {
        connected.add(element);
        invoke(element, 'connectedCallback', []);
      }
    } catch (error) {
      console.error(error);
    }
  }

  function upgradeTree(root) {
    visitElements(root, function(element) {
      const definition = definitions.get(localNameOf(element));
      if (definition) enqueueUpgrade(element, definition);
    });
  }

  class CustomElementRegistry {
    define(name, constructor, options) {
      name = String(name);
      if (typeof constructor !== 'function') throw new TypeError('constructor must be callable');
      const extendsValue = options == null ? undefined : options.extends;
      const extendsName = extendsValue === undefined ? undefined : String(extendsValue);
      pushReactions();
      try {
        if (!validName(name)) throw new DOMException('invalid custom element name', 'SyntaxError');
        if (extendsName !== undefined) {
          throw new DOMException('customized built-in elements are not supported', 'NotSupportedError');
        }
        if (definitions.has(name) || constructors.has(constructor)) {
          throw new DOMException('custom element is already defined', 'NotSupportedError');
        }
        // https://html.spec.whatwg.org/multipage/custom-elements.html#dom-customelementregistry-define
        if (definitionRunning) throw new DOMException('custom element definition is running', 'NotSupportedError');
        let callbacks;
        let observed;
        definitionRunning = true;
        try {
          callbacks = createObject(null);
          observed = [];
          const prototype = constructor.prototype;
          if (prototype === null || (typeof prototype !== 'object' && typeof prototype !== 'function')) {
            throw new TypeError('custom element prototype must be an object');
          }
          for (let index = 0; index < callbackNames.length; ++index) {
            const key = callbackNames[index];
            const callback = prototype[key];
            if (callback !== undefined && typeof callback !== 'function') throw new TypeError('custom element callback must be callable');
            callbacks[key] = callback;
          }
          if (callbacks.attributeChangedCallback !== undefined) {
            const attributes = constructor.observedAttributes;
            if (attributes !== undefined) observed = arrayFrom(attributes, value => String(value));
          }
        } finally {
          definitionRunning = false;
        }
        const definition = { name, constructor, observed, callbacks };
        definitions.set(name, definition);
        constructors.set(constructor, name);
        upgradeTree(document);
        const waiter = pending.get(name);
        if (waiter) {
          waiter.resolve(constructor);
          pending.delete(name);
        }
      } finally { popReactions(); }
    }
    get(name) { return definitions.get(String(name))?.constructor; }
    getName(constructor) { return constructors.get(constructor) ?? null; }
    whenDefined(name) {
      name = String(name);
      if (!validName(name)) return Promise.reject(new DOMException('invalid custom element name', 'SyntaxError'));
      const definition = definitions.get(name);
      if (definition) return Promise.resolve(definition.constructor);
      let waiter = pending.get(name);
      if (!waiter) {
        let resolve;
        const promise = new Promise(done => { resolve = done; });
        waiter = { promise, resolve };
        pending.set(name, waiter);
      }
      return waiter.promise;
    }
    upgrade(root) {
      pushReactions();
      try { upgradeTree(root); } finally { popReactions(); }
    }
  }

  const registry = new CustomElementRegistry();
  Object.defineProperty(globalThis, 'CustomElementRegistry', {
    value: CustomElementRegistry, writable: true, configurable: true,
  });
  Object.defineProperty(globalThis, 'customElements', {
    value: registry, writable: false, configurable: true,
  });

  const nativeCreateElement = Document.prototype.createElement;
  Document.prototype.createElement = function(name, options) {
    const element = nativeCreateElement.call(this, name, options);
    const definition = definitions.get(localNameOf(element));
    if (definition) {
      pushReactions();
      try { enqueueUpgrade(element, definition); } finally { popReactions(); }
    }
    return element;
  };

  // The engine adopts a cross-document node by materializing a copy
  // (<crates/renderer/src/js/bindings/clone.rs>), while Web IDL adoption keeps
  // the same object. Reactions the observer enqueues for the copy never reach
  // the original wrapper the page holds, so upgrade the arguments of the
  // insertion operations as well
  // (<https://dom.spec.whatwg.org/#concept-node-adopt>).
  function insertedNodes(node) {
    if (nativeApply(nodeTypeGetter, node, []) !== 11) return [node];
    const children = [];
    for (let child = nativeApply(firstChildGetter, node, []); child; child = nativeApply(nextSiblingGetter, child, [])) {
      children.push(child);
    }
    return children;
  }
  for (const name of ['appendChild', 'insertBefore', 'replaceChild']) {
    const native = Node.prototype[name];
    const insertion = function() {
      const inserted = insertedNodes(arguments[0]);
      const result = nativeApply(native, this, arguments);
      pushReactions();
      try { for (const node of inserted) upgradeTree(node); } finally { popReactions(); }
      return result;
    };
    Object.defineProperty(insertion, 'length', { value: native.length });
    Object.defineProperty(insertion, 'name', { value: native.name });
    Node.prototype[name] = insertion;
  }

  // https://html.spec.whatwg.org/multipage/custom-elements.html#enqueue-a-custom-element-callback-reaction
  function collectRecords(records) {
    const snapshots = createObject(null);
    snapshots.length = records.length;
    for (let index = 0; index < records.length; ++index) {
      const record = records[index];
      snapshots[index] = {
        type: nativeApply(recordGetters.type, record, []),
        target: nativeApply(recordGetters.target, record, []),
        removedNodes: nativeApply(recordGetters.removedNodes, record, []),
        addedNodes: nativeApply(recordGetters.addedNodes, record, []),
        attributeName: nativeApply(recordGetters.attributeName, record, []),
        attributeNamespace: nativeApply(recordGetters.attributeNamespace, record, []),
        oldValue: nativeApply(recordGetters.oldValue, record, []),
      };
    }
    records = snapshots;
    const values = new MapConstructor();
    const newValues = new MapConstructor();
    for (let index = records.length - 1; index >= 0; --index) {
      const record = records[index];
      if (record.type !== 'attributes') continue;
      let namespaces = nativeApply(mapGet, values, [record.target]);
      if (!namespaces) nativeApply(mapSet, values, [record.target, namespaces = new MapConstructor()]);
      let attributes = nativeApply(mapGet, namespaces, [record.attributeNamespace]);
      if (!attributes) nativeApply(mapSet, namespaces, [record.attributeNamespace, attributes = new MapConstructor()]);
      const key = record.attributeName;
      const value = nativeApply(mapHas, attributes, [key]) ? nativeApply(mapGet, attributes, [key]) : nativeApply(getAttributeNS, record.target, [record.attributeNamespace, key]);
      nativeApply(mapSet, newValues, [record, value]);
      nativeApply(mapSet, attributes, [key, record.oldValue]);
    }
    for (let index = 0; index < records.length; ++index) {
      const record = records[index];
      if (record.type === 'childList') {
        const removed = record.removedNodes;
        for (let index = 0, length = nativeApply(listLengthGetter, removed, []); index < length; ++index) {
          notifyDisconnected(nativeApply(listItem, removed, [index]));
        }
        const added = record.addedNodes;
        for (let index = 0, length = nativeApply(listLengthGetter, added, []); index < length; ++index) {
          const node = nativeApply(listItem, added, [index]);
          upgradeTree(node);
          notifyConnected(node);
        }
      } else if (record.type === 'attributes' && upgraded.has(record.target)) {
        const definition = definitions.get(localNameOf(record.target));
        if (definition && nativeApply(arrayIncludes, definition.observed, [record.attributeName])) {
          invoke(record.target, 'attributeChangedCallback', [
            record.attributeName, record.oldValue, nativeApply(mapGet, newValues, [record]), record.attributeNamespace,
          ]);
        }
      }
    }
  }
  const observer = new MutationObserver(collectRecords);
  const takeRecords = MutationObserver.prototype.takeRecords;
  host.__tbCollectCustomReactions = function() {
    collectRecords(nativeApply(takeRecords, observer, []));
  };
  const observerOptions = {
    subtree: true, childList: true, attributes: true, attributeOldValue: true,
  };
  nativeApply(observe, observer, [document, observerOptions]);
  const nativeAttachShadow = Element.prototype.attachShadow;
  Element.prototype.attachShadow = function(init) {
    const root = nativeAttachShadow.call(this, init);
    shadowRoots.set(this, root);
    nativeApply(observe, observer, [root, observerOptions]);
    return root;
  };
}
