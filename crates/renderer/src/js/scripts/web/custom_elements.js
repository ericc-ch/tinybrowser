// Autonomous custom elements. Definition upgrades the existing document
// synchronously, and later tree/attribute reactions are delivered at the DOM
// mutation-observer microtask checkpoint.
// https://html.spec.whatwg.org/multipage/custom-elements.html#custom-elements-core-concepts
{
  const definitions = new Map();
  const constructors = new Map();
  const upgraded = new WeakSet();
  const connected = new WeakSet();
  // Shadow roots keyed by host. `host.shadowRoot` is null in closed mode, but
  // lifecycle traversal still has to reach those descendants.
  const shadowRoots = new WeakMap();
  const pending = new Map();

  function shadowRootOf(host) {
    return host.shadowRoot || shadowRoots.get(host) || null;
  }

  // Shadow-including element descendants, in tree order. A host's shadow tree
  // is visited before its light children
  // (<https://dom.spec.whatwg.org/#concept-shadow-including-tree-order>).
  // Fragments and shadow roots walk their element children.
  function visitElements(root, callback) {
    if (!root) return;
    if (root.nodeType === 11) {
      for (const child of root.children || []) visitElements(child, callback);
      return;
    }
    if (root.nodeType !== 1) return;
    callback(root);
    const shadow = shadowRootOf(root);
    if (shadow) {
      for (const child of shadow.children || []) visitElements(child, callback);
    }
    for (const child of root.children || []) visitElements(child, callback);
  }

  // Connection reactions: fire once per connection transition, whatever
  // mutation path caused it. The WeakSet makes the synchronous mutation
  // hooks and the MutationObserver idempotent.
  function notifyConnected(root) {
    visitElements(root, function(element) {
      if (!upgraded.has(element) || connected.has(element) || !element.isConnected) return;
      connected.add(element);
      invoke(element, 'connectedCallback', []);
    });
  }

  function notifyDisconnected(root) {
    visitElements(root, function(element) {
      if (!upgraded.has(element) || !connected.has(element) || element.isConnected) return;
      connected.delete(element);
      invoke(element, 'disconnectedCallback', []);
    });
  }

  // A connected move removes and re-inserts the node, so both reactions fire
  // even though the node stays connected
  // (<https://dom.spec.whatwg.org/#concept-node-insert>).
  function notifyMoved(root) {
    visitElements(root, function(element) {
      if (!upgraded.has(element) || !connected.has(element) || !element.isConnected) return;
      connected.delete(element);
      invoke(element, 'disconnectedCallback', []);
      connected.add(element);
      invoke(element, 'connectedCallback', []);
    });
  }

  function validName(name) {
    return typeof name === 'string' && name.includes('-') &&
      name === name.toLowerCase() && /^[a-z][.0-9_a-z\-]*$/.test(name);
  }

  function candidates(root, name) {
    const result = [];
    if (root && root.nodeType === 1 && root.localName === name) result.push(root);
    if (root && typeof root.querySelectorAll === 'function') {
      for (const element of root.querySelectorAll(name)) result.push(element);
    }
    return result;
  }

  function invoke(element, name, args) {
    const callback = element[name];
    if (typeof callback === 'function') {
      try { callback.apply(element, args); } catch (error) { console.error(error); }
    }
  }

  function upgradeElement(element, definition) {
    if (upgraded.has(element) || element.localName !== definition.name) return;
    upgraded.add(element);
    try {
      __tbPushCustomConstruction(element);
      let constructed;
      try {
        constructed = Reflect.construct(definition.constructor, [], definition.constructor);
      } finally {
        // A constructor that fails before `super()` must not leave its
        // candidate on the construction stack for the next upgrade.
        __tbDiscardCustomConstruction(element);
      }
      if (constructed !== element) throw new TypeError('custom element constructor returned another object');
      for (const name of definition.observed) {
        if (element.hasAttribute(name)) {
          invoke(element, 'attributeChangedCallback', [name, null, element.getAttribute(name)]);
        }
      }
      if (element.isConnected) {
        connected.add(element);
        invoke(element, 'connectedCallback', []);
      }
    } catch (error) {
      console.error(error);
    }
  }

  function upgradeTree(root) {
    for (const definition of definitions.values()) {
      for (const element of candidates(root, definition.name)) upgradeElement(element, definition);
    }
  }

  class CustomElementRegistry {
    define(name, constructor, options) {
      name = String(name);
      if (!validName(name)) throw new DOMException('invalid custom element name', 'SyntaxError');
      if (typeof constructor !== 'function') throw new TypeError('constructor must be callable');
      if (options && options.extends !== undefined) {
        throw new DOMException('customized built-in elements are not supported', 'NotSupportedError');
      }
      if (definitions.has(name) || constructors.has(constructor)) {
        throw new DOMException('custom element is already defined', 'NotSupportedError');
      }
      let observed = [];
      if (constructor.observedAttributes !== undefined) {
        observed = Array.from(constructor.observedAttributes, value => String(value));
      }
      const definition = { name, constructor, observed };
      definitions.set(name, definition);
      constructors.set(constructor, name);
      upgradeTree(document);
      const waiter = pending.get(name);
      if (waiter) {
        waiter.resolve(constructor);
        pending.delete(name);
      }
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
    upgrade(root) { upgradeTree(root); }
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
    const definition = definitions.get(element.localName);
    if (definition) upgradeElement(element, definition);
    return element;
  };

  // Insertion runs custom-element reactions before returning. Fragment
  // insertion snapshots its children because the native operation empties the
  // fragment. A node that was already connected is a move: it gets the
  // disconnect and connect reactions.
  // https://html.spec.whatwg.org/multipage/custom-elements.html#enqueue-a-custom-element-upgrade-reaction
  function insertedChildren(node) {
    return node.nodeType === 11 ? Array.from(node.childNodes) : [node];
  }

  function reactToInsertion(added, moved) {
    for (const child of added) {
      upgradeTree(child);
      if (moved.includes(child)) notifyMoved(child);
      notifyConnected(child);
    }
  }

  const nativeAppendChild = Node.prototype.appendChild;
  Node.prototype.appendChild = function(node) {
    const added = insertedChildren(node);
    const moved = added.filter(child => child.isConnected);
    const result = nativeAppendChild.call(this, node);
    reactToInsertion(added, moved);
    return result;
  };
  const nativeInsertBefore = Node.prototype.insertBefore;
  Node.prototype.insertBefore = function(node, child) {
    const added = insertedChildren(node);
    const moved = added.filter(entry => entry.isConnected);
    const result = nativeInsertBefore.call(this, node, child);
    reactToInsertion(added, moved);
    return result;
  };
  const nativeRemoveChild = Node.prototype.removeChild;
  Node.prototype.removeChild = function(node) {
    const result = nativeRemoveChild.call(this, node);
    notifyDisconnected(node);
    return result;
  };
  const nativeReplaceChild = Node.prototype.replaceChild;
  Node.prototype.replaceChild = function(node, child) {
    const added = insertedChildren(node);
    const moved = added.filter(entry => entry.isConnected);
    const result = nativeReplaceChild.call(this, node, child);
    notifyDisconnected(child);
    reactToInsertion(added, moved);
    return result;
  };

  const observer = new MutationObserver(records => {
    for (const record of records) {
      if (record.type === 'childList') {
        for (const node of record.addedNodes) {
          upgradeTree(node);
          notifyConnected(node);
        }
        for (const node of record.removedNodes) notifyDisconnected(node);
      } else if (record.type === 'attributes' && upgraded.has(record.target)) {
        const definition = definitions.get(record.target.localName);
        if (definition && definition.observed.includes(record.attributeName)) {
          invoke(record.target, 'attributeChangedCallback', [
            record.attributeName, record.oldValue, record.target.getAttribute(record.attributeName),
          ]);
        }
      }
    }
  });
  const observerOptions = {
    subtree: true, childList: true, attributes: true, attributeOldValue: true,
  };
  observer.observe(document, observerOptions);
  const nativeAttachShadow = Element.prototype.attachShadow;
  Element.prototype.attachShadow = function(init) {
    const root = nativeAttachShadow.call(this, init);
    shadowRoots.set(this, root);
    observer.observe(root, observerOptions);
    return root;
  };
}
