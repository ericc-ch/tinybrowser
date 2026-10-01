// https://html.spec.whatwg.org/multipage/dom.html#htmlorsvgormathmlelement
[Exposed=Window, Rust=JsNode]
partial interface HTMLElement {
    [SameObject, RustValue, Rust=dataset] readonly attribute DOMStringMap dataset;
};
