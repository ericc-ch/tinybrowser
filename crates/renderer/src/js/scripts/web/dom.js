globalThis.__tbMakeDataset = element => new Proxy(Object.create(null), {
  get(_target, property) {
    if (property === Symbol.toStringTag) return 'DOMStringMap';
    if (typeof property !== 'string') return undefined;
    const name = 'data-' + property.replace(/[A-Z]/g, letter => '-' + letter.toLowerCase());
    const value = element.getAttribute(name);
    return value === null ? undefined : value;
  },
  set(_target, property, value) {
    property = String(property);
    if (/-[a-z]/.test(property)) throw new DOMException('invalid dataset property', 'SyntaxError');
    const name = 'data-' + property.replace(/[A-Z]/g, letter => '-' + letter.toLowerCase());
    element.setAttribute(name, String(value));
    return true;
  },
  deleteProperty(_target, property) {
    property = String(property);
    const name = 'data-' + property.replace(/[A-Z]/g, letter => '-' + letter.toLowerCase());
    element.removeAttribute(name);
    return true;
  },
  ownKeys() {
    return element.getAttributeNames().filter(name =>
      name.startsWith('data-') && !/[A-Z]/.test(name.slice(5))
    ).map(name => name.slice(5).replace(/-([a-z])/g, (_match, letter) => letter.toUpperCase()));
  },
  getOwnPropertyDescriptor(_target, property) {
    const value = this.get(_target, property);
    if (value === undefined) return undefined;
    return { configurable: true, enumerable: true, writable: true, value };
  }
});

globalThis.__tbMakeStyle = element => {
  const splitDeclarations = value => {
    const parts = []; let start = 0; let quote = '';
    for (let i = 0; i < value.length; i++) {
      const character = value[i];
      if (quote) { if (character === quote && value[i - 1] !== '\\') quote = ''; }
      else if (character === String.fromCharCode(34) || character === String.fromCharCode(39)) quote = character;
      else if (character === ';') { parts.push(value.slice(start, i)); start = i + 1; }
    }
    parts.push(value.slice(start));
    return parts;
  };
  const read = () => {
    const declarations = [];
    for (const part of splitDeclarations(element.getAttribute('style') || '')) {
      const separator = part.indexOf(':');
      if (separator < 0) continue;
      const name = part.slice(0, separator).trim().toLowerCase();
      if (name) declarations.push([name, part.slice(separator + 1).trim()]);
    }
    return declarations;
  };
  const write = declarations => {
    const value = declarations.map(pair => pair[0] + ': ' + pair[1] + ';').join(' ');
    if (value) element.setAttribute('style', value);
    else element.removeAttribute('style');
  };
  const propertyName = property =>
    String(property).replace(/[A-Z]/g, letter => '-' + letter.toLowerCase());
  const target = {
    get cssText() { return element.getAttribute('style') || ''; },
    set cssText(value) { element.setAttribute('style', String(value)); },
    get length() { return read().length; },
    item(index) {
      const pair = read()[Number(index)];
      return pair ? pair[0] : '';
    },
    getPropertyValue(name) {
      const pair = read().find(item => item[0] === String(name).toLowerCase());
      return pair ? pair[1] : '';
    },
    setProperty(name, value) {
      name = String(name).toLowerCase();
      value = String(value);
      const declarations = read().filter(pair => pair[0] !== name);
      if (value) declarations.push([name, value]);
      write(declarations);
    },
    removeProperty(name) {
      name = String(name).toLowerCase();
      const old = this.getPropertyValue(name);
      write(read().filter(pair => pair[0] !== name));
      return old;
    }
  };
  return new Proxy(target, {
    get(target, property, receiver) {
      if (Reflect.has(target, property)) return Reflect.get(target, property, receiver);
      if (typeof property !== 'string') return undefined;
      return target.getPropertyValue(propertyName(property));
    },
    set(target, property, value, receiver) {
      if (Reflect.has(target, property)) return Reflect.set(target, property, value, receiver);
      target.setProperty(propertyName(property), value);
      return true;
    }
  });
};
