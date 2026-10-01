// https://html.spec.whatwg.org/multipage/form-elements.html#htmlbuttonelement
// `type` stays a JavaScript shim.
[Exposed=Window, Rust=JsNode, RustOwnedCtx]
partial interface HTMLButtonElement {
    [CEReactions, Rust=disabled, RustSet=set_disabled] attribute boolean disabled;
    [RustValue, Rust=form] readonly attribute HTMLFormElement? form;
    [CEReactions, Rust=form_action, RustSet=set_form_action, RustSetFromJs=WebIdlString] attribute DOMString formAction;
    [CEReactions, Rust=form_method, RustSet=set_form_method, RustSetFromJs=WebIdlString] attribute DOMString formMethod;
    [CEReactions, Rust=form_enctype, RustSet=set_form_enctype, RustSetFromJs=WebIdlString] attribute DOMString formEnctype;
    [CEReactions, Rust=form_target, RustSet=set_form_target, RustSetFromJs=WebIdlString] attribute DOMString formTarget;
    [CEReactions, Rust=form_no_validate, RustSet=set_form_no_validate] attribute boolean formNoValidate;
};
