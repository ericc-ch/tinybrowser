// https://dom.spec.whatwg.org/#interface-parentnode
// The spec's `ParentNode` mixin. `RustInstall` installs the same members on
// every interface that includes the mixin; the payload is the shared `JsNode`.
[Exposed=Window, Rust=JsNode, RustInstall="Element,Document,DocumentFragment"]
partial interface ParentNode {
    [SameObject, RustValue, Rust=children] readonly attribute HTMLCollection children;
    [RustValue, Rust=first_element_child] readonly attribute Element? firstElementChild;
    [RustValue, Rust=last_element_child] readonly attribute Element? lastElementChild;
    [Rust=child_element_count] readonly attribute unsigned long childElementCount;
    [CEReactions, Rust=append] undefined append([RustValue] (Node or DOMString)... nodes);
    [CEReactions, Rust=prepend] undefined prepend([RustValue] (Node or DOMString)... nodes);
    [CEReactions, Rust=replace_children] undefined replaceChildren([RustValue] (Node or DOMString)... nodes);
    [RustValue, Rust=query_selector] Element? querySelector([RustFromJs=WebIdlString] DOMString selectors);
    [RustValue, Rust=query_selector_all] NodeList querySelectorAll([RustFromJs=WebIdlString] DOMString selectors);
};
