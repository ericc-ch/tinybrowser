// https://html.spec.whatwg.org/multipage/dom.html#htmlorsvgormathmlelement
[Exposed=Window, Rust=JsNode]
partial interface MathMLElement {
    [SameObject, RustValue, Rust=dataset] readonly attribute DOMStringMap dataset;
};
