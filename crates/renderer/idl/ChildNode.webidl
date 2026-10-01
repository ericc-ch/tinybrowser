// https://dom.spec.whatwg.org/#interface-childnode
// The spec's `ChildNode` mixin, shared across the interfaces that include it.
[Exposed=Window, Rust=JsNode, RustInstall="Element,CharacterData,DocumentType"]
partial interface ChildNode {
    [CEReactions, Rust=before] undefined before([RustValue] (Node or DOMString)... nodes);
    [CEReactions, Rust=after] undefined after([RustValue] (Node or DOMString)... nodes);
    [CEReactions, Rust=replace_with] undefined replaceWith([RustValue] (Node or DOMString)... nodes);
    [CEReactions, Rust=remove] undefined remove();
};
