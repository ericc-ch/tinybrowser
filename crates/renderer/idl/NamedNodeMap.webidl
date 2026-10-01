// https://dom.spec.whatwg.org/#interface-namednodemap
// collections.js implements the indexed and named property hooks.
[Exposed=Window, LegacyUnenumerableNamedProperties, Rust=JsNamedNodeMap, RustPropertyHooks=JavaScript]
interface NamedNodeMap {
    [Rust=length] readonly attribute unsigned long length;
    [Rust=item] getter Attr? item(unsigned long index);
    [Rust=get_named_item] getter Attr? getNamedItem([RustFromJs=WebIdlString] DOMString qualifiedName);
    [Rust=get_named_item_ns] Attr? getNamedItemNS([RustFromJs=OptString] DOMString? namespace, [RustFromJs=WebIdlString] DOMString localName);
    [CEReactions, Rust=set_named_item] Attr? setNamedItem([RustFromJs=AttrArgument] Attr attr);
    [CEReactions, Rust=set_named_item_ns] Attr? setNamedItemNS([RustFromJs=AttrArgument] Attr attr);
    [CEReactions, Rust=remove_named_item] Attr removeNamedItem([RustFromJs=WebIdlString] DOMString qualifiedName);
    [CEReactions, Rust=remove_named_item_ns] Attr removeNamedItemNS([RustFromJs=OptString] DOMString? namespace, [RustFromJs=WebIdlString] DOMString localName);
};
