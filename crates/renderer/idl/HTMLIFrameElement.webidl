// https://html.spec.whatwg.org/multipage/iframe-embed-object.html#htmliframeelement
[Exposed=Window, Rust=JsNode, RustOwnedCtx]
partial interface HTMLIFrameElement {
    [RustValue, Rust=content_document] readonly attribute Document? contentDocument;
    [RustValue, Rust=content_window] readonly attribute Window? contentWindow;
};
