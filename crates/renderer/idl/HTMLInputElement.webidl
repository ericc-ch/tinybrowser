// https://html.spec.whatwg.org/multipage/input.html#htmlinputelement
// Shares the `JsNode` payload. `type` and `files` stay JavaScript shims.
[Exposed=Window, Rust=JsNode, RustOwnedCtx]
partial interface HTMLInputElement {
    [CEReactions, Rust=value, RustSet=set_value, RustSetFromJs=LegacyNullString] attribute [LegacyNullToEmptyString] DOMString value;
    [CEReactions, Rust=default_value, RustSet=set_default_value, RustSetFromJs=WebIdlString] attribute DOMString defaultValue;
    [CEReactions, Rust=disabled, RustSet=set_disabled] attribute boolean disabled;
    [CEReactions, Rust=read_only, RustSet=set_read_only] attribute boolean readOnly;
    [CEReactions, Rust=required, RustSet=set_required] attribute boolean required;
    [CEReactions, Rust=multiple, RustSet=set_multiple] attribute boolean multiple;
    [CEReactions, Rust=checked, RustSet=set_checked] attribute boolean checked;
    [CEReactions, Rust=default_checked, RustSet=set_default_checked] attribute boolean defaultChecked;
    [RustValue, Rust=selection_start, RustSet=set_selection_start, RustSetFromJs=WebIdlUnsignedLong] attribute unsigned long? selectionStart;
    [RustValue, Rust=selection_end, RustSet=set_selection_end, RustSetFromJs=WebIdlUnsignedLong] attribute unsigned long? selectionEnd;
    [RustValue, Rust=selection_direction, RustSet=set_selection_direction, RustSetFromJs=WebIdlString] attribute DOMString? selectionDirection;
    [Rust=indeterminate, RustSet=set_indeterminate] attribute boolean indeterminate;
    [RustValue, Rust=form] readonly attribute HTMLFormElement? form;
    [CEReactions, Rust=form_action, RustSet=set_form_action, RustSetFromJs=WebIdlString] attribute DOMString formAction;
    [CEReactions, Rust=form_method, RustSet=set_form_method, RustSetFromJs=WebIdlString] attribute DOMString formMethod;
    [CEReactions, Rust=form_enctype, RustSet=set_form_enctype, RustSetFromJs=WebIdlString] attribute DOMString formEnctype;
    [CEReactions, Rust=form_target, RustSet=set_form_target, RustSetFromJs=WebIdlString] attribute DOMString formTarget;
    [CEReactions, Rust=form_no_validate, RustSet=set_form_no_validate] attribute boolean formNoValidate;
    [CEReactions, Rust=pattern, RustSet=set_pattern, RustSetFromJs=WebIdlString] attribute DOMString pattern;
    [CEReactions, Rust=min, RustSet=set_min, RustSetFromJs=WebIdlString] attribute DOMString min;
    [CEReactions, Rust=max, RustSet=set_max, RustSetFromJs=WebIdlString] attribute DOMString max;
    [CEReactions, Rust=step, RustSet=set_step, RustSetFromJs=WebIdlString] attribute DOMString step;
};
