// https://html.spec.whatwg.org/multipage/form-elements.html#htmlselectelement
// `length`, `item`, `namedItem`, and `size` stay JavaScript shims in forms.js.
[Exposed=Window, Rust=JsNode, RustOwnedCtx]
partial interface HTMLSelectElement {
    [CEReactions, Rust=value, RustSet=set_value, RustSetFromJs=LegacyNullString] attribute [LegacyNullToEmptyString] DOMString value;
    [Rust=selected_index, RustSet=set_selected_index] attribute long selectedIndex;
    [SameObject, RustValue, Rust=options] readonly attribute HTMLOptionsCollection options;
    [CEReactions, Rust=multiple, RustSet=set_multiple] attribute boolean multiple;
    [CEReactions, Rust=disabled, RustSet=set_disabled] attribute boolean disabled;
    [CEReactions, Rust=required, RustSet=set_required] attribute boolean required;
    [SameObject, RustValue, Rust=selected_options] readonly attribute HTMLCollection selectedOptions;
    [RustValue, Rust=form] readonly attribute HTMLFormElement? form;
};
