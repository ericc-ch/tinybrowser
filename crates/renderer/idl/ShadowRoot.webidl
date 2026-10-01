// https://dom.spec.whatwg.org/#interface-shadowroot
// Shares the `JsNode` payload with the other node kinds; `ShadowRoot`
// includes `DocumentOrShadowRoot`, so `activeElement` lives here.
[Exposed=Window, Rust=JsNode]
partial interface ShadowRoot {
    [Rust=host] readonly attribute Element host;
    [Rust=mode] readonly attribute DOMString mode;
    [CEReactions, Rust=inner_html, RustSet=set_inner_html, RustSetFromJs=LegacyNullString] attribute [LegacyNullToEmptyString] DOMString innerHTML;
    [RustValue, Rust=active_element] readonly attribute Element? activeElement;
};
