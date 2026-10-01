// https://dom.spec.whatwg.org/#interface-element
// Rust and RustFromJs are build-time implementation mappings. `Element` shares
// the `JsNode` payload with `Node`.
[Exposed=Window, Rust=JsNode]
partial interface Element {
    [Rust=get_elements_by_tag_name] HTMLCollection getElementsByTagName([RustFromJs=WebIdlString] DOMString qualifiedName);
    [Rust=get_elements_by_tag_name_ns] HTMLCollection getElementsByTagNameNS([RustFromJs=OptString] DOMString? namespace, [RustFromJs=WebIdlString] DOMString localName);
};
