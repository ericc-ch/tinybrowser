(function() {
  'use strict';
  const WeakMapConstructor = WeakMap;
  const apply = Reflect.apply;
  const get = WeakMap.prototype.get;
  const set = WeakMap.prototype.set;
  const has = WeakMap.prototype.has;
  const remove = WeakMap.prototype.delete;
  const create = Object.create;
  const freeze = Object.freeze;
  const Uint8ArrayConstructor = Uint8Array;
  const named = create(null);
  function slots(name) {
    if (name !== undefined && named[name] !== undefined) return named[name];
    const map = new WeakMapConstructor();
    const slots = create(null);
    slots.get = key => apply(get, map, [key]);
    slots.set = (key, value) => { apply(set, map, [key, value]); };
    slots.has = key => apply(has, map, [key]);
    slots.delete = key => apply(remove, map, [key]);
    slots.add = key => { apply(set, map, [key, true]); };
    freeze(slots);
    if (name !== undefined) named[name] = slots;
    return slots;
  }
  slots.bytes = source => new Uint8ArrayConstructor(source);
  return freeze(slots);
})()
