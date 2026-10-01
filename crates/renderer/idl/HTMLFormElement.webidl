// https://html.spec.whatwg.org/multipage/forms.html#htmlformelement
// Shares the `JsNode` payload. `RustOwnedCtx` matches the hand-written
// getters and setters, which take `Ctx` by value.
[Exposed=Window, Rust=JsNode, RustOwnedCtx]
partial interface HTMLFormElement {
    [CEReactions, Rust=reset] undefined reset();
    [CEReactions, Rust=action, RustSet=set_action, RustSetFromJs=WebIdlString] attribute DOMString action;
    [CEReactions, Rust=method, RustSet=set_method, RustSetFromJs=WebIdlString] attribute DOMString method;
    [CEReactions, Rust=enctype, RustSet=set_enctype, RustSetFromJs=WebIdlString] attribute DOMString enctype;
    [CEReactions, Rust=encoding, RustSet=set_encoding, RustSetFromJs=WebIdlString] attribute DOMString encoding;
    [CEReactions, Rust=target, RustSet=set_target, RustSetFromJs=WebIdlString] attribute DOMString target;
    [CEReactions, Rust=no_validate, RustSet=set_no_validate] attribute boolean noValidate;
    [CEReactions, Rust=accept_charset, RustSet=set_accept_charset, RustSetFromJs=WebIdlString] attribute DOMString acceptCharset;
};
