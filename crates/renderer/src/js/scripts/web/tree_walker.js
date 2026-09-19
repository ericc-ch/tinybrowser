// https://dom.spec.whatwg.org/#interface-treewalker
{
  const FILTER_ACCEPT = 1;
  const FILTER_REJECT = 2;
  const FILTER_SKIP = 3;

  function filterResult(filter, node) {
    if (filter === null || filter === undefined) return FILTER_ACCEPT;
    if (typeof filter === 'function') return Number(filter(node));
    return Number(filter.acceptNode(node));
  }

  class TreeWalker {
    constructor(root, whatToShow, filter) {
      this.root = root;
      this.whatToShow = whatToShow >>> 0;
      this.filter = filter ?? null;
      this.currentNode = root;
    }

    nextNode() {
      let node = this.currentNode;
      let skipChildren = false;
      while (node) {
        if (!skipChildren && node.firstChild) {
          node = node.firstChild;
        } else {
          skipChildren = false;
          while (node && node !== this.root && !node.nextSibling) node = node.parentNode;
          if (!node || node === this.root) return null;
          node = node.nextSibling;
        }
        const shown = (this.whatToShow & (1 << (node.nodeType - 1))) !== 0;
        if (!shown) continue;
        const result = filterResult(this.filter, node);
        if (result === FILTER_ACCEPT) {
          this.currentNode = node;
          return node;
        }
        if (result === FILTER_REJECT) skipChildren = true;
      }
      return null;
    }
  }

  const NodeFilter = {
    FILTER_ACCEPT, FILTER_REJECT, FILTER_SKIP,
    SHOW_ALL: 0xFFFFFFFF,
    SHOW_ELEMENT: 0x1,
    SHOW_ATTRIBUTE: 0x2,
    SHOW_TEXT: 0x4,
    SHOW_CDATA_SECTION: 0x8,
    SHOW_ENTITY_REFERENCE: 0x10,
    SHOW_ENTITY: 0x20,
    SHOW_PROCESSING_INSTRUCTION: 0x40,
    SHOW_COMMENT: 0x80,
    SHOW_DOCUMENT: 0x100,
    SHOW_DOCUMENT_TYPE: 0x200,
    SHOW_DOCUMENT_FRAGMENT: 0x400,
    SHOW_NOTATION: 0x800,
  };
  Object.defineProperty(globalThis, 'NodeFilter', {
    value: Object.freeze(NodeFilter), writable: true, configurable: true,
  });
  Object.defineProperty(globalThis, 'TreeWalker', {
    value: TreeWalker, writable: true, configurable: true,
  });
  Object.defineProperty(Document.prototype, 'createTreeWalker', {
    value: function(root, whatToShow = 0xFFFFFFFF, filter = null) {
      if (!(root instanceof Node)) throw new TypeError('root is not a Node');
      return new TreeWalker(root, Number(whatToShow), filter);
    },
    writable: true, configurable: true, enumerable: true,
  });
  const selection = {
    anchorNode: null, anchorOffset: 0, focusNode: null, focusOffset: 0,
    isCollapsed: true, rangeCount: 0, type: 'None',
    getRangeAt() { throw new DOMException('no range at index', 'IndexSizeError'); },
    removeAllRanges() {}, addRange() {}, collapse() {},
    toString() { return ''; },
  };
  Object.defineProperty(Document.prototype, 'getSelection', {
    value: function() { return selection; },
    writable: true, configurable: true, enumerable: true,
  });
  Object.defineProperty(globalThis, 'getSelection', {
    value: function() { return selection; }, writable: true, configurable: true,
  });
}
