// https://dom.spec.whatwg.org/#interface-attr
[Exposed=Window, Rust=JsAttr, RustLifetime]
interface Attr : Node {
    [Rust=namespace_uri] readonly attribute DOMString? namespaceURI;
    [Rust=prefix] readonly attribute DOMString? prefix;
    [Rust=local_name] readonly attribute DOMString localName;
    [Rust=name] readonly attribute DOMString name;
    [CEReactions, Rust=value, RustSet=set_value] attribute DOMString value;
    [Rust=owner_element] readonly attribute Element? ownerElement;
    [Rust=specified] readonly attribute boolean specified;
};
