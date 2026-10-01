// https://html.spec.whatwg.org/multipage/common-dom-interfaces.html#htmloptionscollection
[Exposed=Window, LegacyOverrideBuiltIns, Rust=JsOptionsCollection, RustPropertyHooks=IndexedNamed, RustSupportedNames=supported_names]
interface HTMLOptionsCollection : HTMLCollection {
    // Inherited `item` and `namedItem` are repeated so the indexed and named
    // hooks validate on this interface; they share the collection algorithms.
    [Rust=item] getter Element? item(unsigned long index);
    [Rust=named_item] getter Element? namedItem(DOMString name);
    [CEReactions, Rust=length, RustSet=set_length] attribute unsigned long length;
    [CEReactions, Rust=set_indexed] setter undefined (unsigned long index, [RustValue] HTMLOptionElement? option);
    [CEReactions, Rust=add] undefined add([RustValue] (HTMLOptionElement or HTMLOptGroupElement) element, optional [RustValue] (HTMLElement or long)? before = null);
    [CEReactions, Rust=remove] undefined remove(long index);
    [CEReactions, Rust=selected_index, RustSet=set_selected_index] attribute long selectedIndex;
};
