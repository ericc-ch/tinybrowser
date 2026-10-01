// https://dom.spec.whatwg.org/#interface-htmlcollection
[Exposed=Window, LegacyUnenumerableNamedProperties, Rust=JsHtmlCollection, RustAlternate=JsOptionsCollection, RustPropertyHooks=IndexedNamed, RustSupportedNames=supported_names]
interface HTMLCollection {
    [Rust=length] readonly attribute unsigned long length;
    [Rust=item] getter Element? item(unsigned long index);
    [Rust=named_item] getter Element? namedItem(DOMString name);
};
