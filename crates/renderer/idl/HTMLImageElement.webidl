// https://html.spec.whatwg.org/multipage/embedded-content.html#htmlimageelement
[Exposed=Window, Rust=JsNode, RustOwnedCtx]
partial interface HTMLImageElement {
    [Rust=natural_width] readonly attribute unsigned long naturalWidth;
    [Rust=natural_height] readonly attribute unsigned long naturalHeight;
    [Rust=complete] readonly attribute boolean complete;
    [Rust=current_src] readonly attribute DOMString currentSrc;
};
