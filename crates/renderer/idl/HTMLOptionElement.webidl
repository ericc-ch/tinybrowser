// https://html.spec.whatwg.org/multipage/form-elements.html#htmloptionelement
[Exposed=Window, Rust=JsNode, RustOwnedCtx]
partial interface HTMLOptionElement {
    [CEReactions, Rust=value, RustSet=set_value, RustSetFromJs=LegacyNullString] attribute [LegacyNullToEmptyString] DOMString value;
    [CEReactions, Rust=selected, RustSet=set_selected] attribute boolean selected;
    [CEReactions, Rust=default_selected, RustSet=set_default_selected] attribute boolean defaultSelected;
    [CEReactions, Rust=text, RustSet=set_text, RustSetFromJs=WebIdlCodeUnits] attribute DOMString text;
    [Rust=index] readonly attribute long index;
    [CEReactions, Rust=disabled, RustSet=set_disabled] attribute boolean disabled;
    [CEReactions, Rust=label, RustSet=set_label, RustSetFromJs=WebIdlString] attribute DOMString label;
    [RustValue, Rust=form] readonly attribute HTMLFormElement? form;
};
