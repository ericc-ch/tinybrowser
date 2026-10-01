// https://html.spec.whatwg.org/multipage/form-elements.html#htmltextareaelement
[Exposed=Window, Rust=JsNode, RustOwnedCtx]
partial interface HTMLTextAreaElement {
    [CEReactions, Rust=value, RustSet=set_value, RustSetFromJs=LegacyNullString] attribute [LegacyNullToEmptyString] DOMString value;
    [CEReactions, Rust=default_value, RustSet=set_default_value, RustSetFromJs=WebIdlString] attribute DOMString defaultValue;
    [Rust=text_length] readonly attribute unsigned long textLength;
    [CEReactions, Rust=disabled, RustSet=set_disabled] attribute boolean disabled;
    [CEReactions, Rust=read_only, RustSet=set_read_only] attribute boolean readOnly;
    [CEReactions, Rust=required, RustSet=set_required] attribute boolean required;
    [RustValue, Rust=selection_start, RustSet=set_selection_start, RustSetFromJs=WebIdlUnsignedLong] attribute unsigned long? selectionStart;
    [RustValue, Rust=selection_end, RustSet=set_selection_end, RustSetFromJs=WebIdlUnsignedLong] attribute unsigned long? selectionEnd;
    [RustValue, Rust=selection_direction, RustSet=set_selection_direction, RustSetFromJs=WebIdlString] attribute DOMString? selectionDirection;
    [RustValue, Rust=form] readonly attribute HTMLFormElement? form;
};
