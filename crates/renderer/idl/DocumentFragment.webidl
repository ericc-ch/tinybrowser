// https://dom.spec.whatwg.org/#interface-documentfragment
// Rust and RustFromJs are build-time implementation mappings. `DocumentFragment`
// shares the `JsNode` payload with `Node`.
[Exposed=Window, Rust=JsNode]
partial interface DocumentFragment {
    [Rust=get_element_by_id] Element? getElementById([RustFromJs=WebIdlString] DOMString elementId);
};
