// https://html.spec.whatwg.org/multipage/form-elements.html#htmlfieldsetelement
[Exposed=Window, Rust=JsNode, RustOwnedCtx]
partial interface HTMLFieldSetElement {
    [CEReactions, Rust=disabled, RustSet=set_disabled] attribute boolean disabled;
    [RustValue, Rust=form] readonly attribute HTMLFormElement? form;
};
