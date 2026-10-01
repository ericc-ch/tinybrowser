// https://dom.spec.whatwg.org/#interface-document
// https://html.spec.whatwg.org/multipage/dom.html#the-document-object
// Rust and RustValue are build-time implementation mappings. `Document` shares
// the `JsNode` payload with `Node`, `Element`, and the other node kinds.
[Exposed=Window, Rust=JsNode]
partial interface Document {
    [RustValue, Rust=implementation] readonly attribute DOMImplementation implementation;
    [RustValue, Rust=document_element] readonly attribute Element? documentElement;
    [RustValue, Rust=body] readonly attribute HTMLElement? body;
    [RustValue, Rust=head] readonly attribute HTMLElement? head;
    [RustValue, Rust=current_script] readonly attribute HTMLOrSVGScriptElement? currentScript;
    [RustValue, Rust=doctype] readonly attribute DocumentType? doctype;
    [Rust=document_uri] readonly attribute USVString documentURI;
    [Rust=url] readonly attribute USVString URL;
    [Rust=compat_mode] readonly attribute DOMString compatMode;
    [Rust=character_set] readonly attribute DOMString characterSet;
    [Rust=charset] readonly attribute DOMString charset;
    [Rust=input_encoding] readonly attribute DOMString inputEncoding;
    [Rust=content_type] readonly attribute DOMString contentType;
    [Rust=ready_state] readonly attribute DOMString readyState;
    [RustValue, Rust=default_view] readonly attribute object? defaultView;
    [RustValue, Rust=location] readonly attribute Location? location;
    [Rust=has_focus] boolean hasFocus();
};
