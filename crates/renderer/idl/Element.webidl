// https://dom.spec.whatwg.org/#interface-element
// Rust, RustValue, RustSetFromJs, and RustFromJs are build-time implementation
// mappings. `Element` shares the `JsNode` payload with `Node`.
[Exposed=Window, Rust=JsNode]
partial interface Element {
    [Rust=get_elements_by_tag_name] HTMLCollection getElementsByTagName([RustFromJs=WebIdlString] DOMString qualifiedName);
    [Rust=get_elements_by_tag_name_ns] HTMLCollection getElementsByTagNameNS([RustFromJs=OptString] DOMString? namespace, [RustFromJs=WebIdlString] DOMString localName);

    [Rust=tag_name] readonly attribute DOMString tagName;
    [Rust=local_name] readonly attribute DOMString localName;
    [RustValue, Rust=prefix] readonly attribute DOMString? prefix;
    [RustValue, Rust=namespace_uri] readonly attribute DOMString? namespaceURI;
    [CEReactions, Rust=id, RustSet=set_id, RustSetFromJs=WebIdlString] attribute DOMString id;
    [CEReactions, Rust=class_name, RustSet=set_class_name, RustSetFromJs=WebIdlString] attribute DOMString className;
    [SameObject, PutForwards=value, RustValue, Rust=class_list] readonly attribute DOMTokenList classList;

    [Rust=get_attribute] DOMString? getAttribute([RustFromJs=WebIdlString] DOMString qualifiedName);
    [CEReactions, Rust=set_attribute] undefined setAttribute([RustFromJs=WebIdlString] DOMString qualifiedName, [RustFromJs=WebIdlString] (TrustedType or DOMString) value);
    [Rust=has_attribute] boolean hasAttribute([RustFromJs=WebIdlString] DOMString qualifiedName);
    [CEReactions, Rust=remove_attribute] undefined removeAttribute([RustFromJs=WebIdlString] DOMString qualifiedName);
    [CEReactions, Rust=toggle_attribute] boolean toggleAttribute([RustFromJs=WebIdlString] DOMString qualifiedName, optional boolean force);
    [Rust=get_attribute_ns] DOMString? getAttributeNS([RustFromJs=OptString] DOMString? namespace, [RustFromJs=WebIdlString] DOMString localName);
    [CEReactions, Rust=set_attribute_ns] undefined setAttributeNS([RustFromJs=OptString] DOMString? namespace, [RustFromJs=WebIdlString] DOMString qualifiedName, [RustFromJs=WebIdlString] (TrustedType or DOMString) value);
    [Rust=has_attribute_ns] boolean hasAttributeNS([RustFromJs=OptString] DOMString? namespace, [RustFromJs=WebIdlString] DOMString localName);
    [CEReactions, Rust=remove_attribute_ns] undefined removeAttributeNS([RustFromJs=OptString] DOMString? namespace, [RustFromJs=WebIdlString] DOMString localName);
    [Rust=get_attribute_names] sequence<DOMString> getAttributeNames();
    [SameObject, RustValue, Rust=attributes] readonly attribute NamedNodeMap attributes;
    [Rust=has_attributes] boolean hasAttributes();
    [Rust=get_attribute_node] Attr? getAttributeNode([RustFromJs=WebIdlString] DOMString qualifiedName);
    [Rust=get_attribute_node_ns] Attr? getAttributeNodeNS([RustFromJs=OptString] DOMString? namespace, [RustFromJs=WebIdlString] DOMString localName);
    [CEReactions, Rust=set_attribute_node] Attr? setAttributeNode([RustFromJs=AttrArgument] Attr attr);
    [CEReactions, Rust=set_attribute_node_ns] Attr? setAttributeNodeNS([RustFromJs=AttrArgument] Attr attr);
    [CEReactions, Rust=remove_attribute_node] Attr removeAttributeNode([RustFromJs=AttrArgument] Attr attr);
};
