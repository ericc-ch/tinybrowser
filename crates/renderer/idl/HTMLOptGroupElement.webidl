// https://html.spec.whatwg.org/multipage/form-elements.html#htmloptgroupelement
[Exposed=Window, Rust=JsNode, RustOwnedCtx]
partial interface HTMLOptGroupElement {
    [CEReactions, Rust=disabled, RustSet=set_disabled] attribute boolean disabled;
};
