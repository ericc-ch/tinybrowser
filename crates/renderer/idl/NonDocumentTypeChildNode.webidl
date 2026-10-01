// https://dom.spec.whatwg.org/#interface-nondocumenttypechildnode
// The spec's `NonDocumentTypeChildNode` mixin.
[Exposed=Window, Rust=JsNode, RustInstall="Element,CharacterData"]
partial interface NonDocumentTypeChildNode {
    [RustValue, Rust=previous_element_sibling] readonly attribute Element? previousElementSibling;
    [RustValue, Rust=next_element_sibling] readonly attribute Element? nextElementSibling;
};
