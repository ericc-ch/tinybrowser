// https://html.spec.whatwg.org/multipage/dom.html#htmlorsvgormathmlelement
[Exposed=Window, Rust=JsNode]
partial interface SVGElement {
    [SameObject, RustValue, Rust=dataset] readonly attribute DOMStringMap dataset;
};
