// https://dom.spec.whatwg.org/#interface-nodelist
// collections.js installs the value iterator.
[Exposed=Window, Rust=JsNodeList, RustPropertyHooks=Indexed]
interface NodeList {
    [Rust=item] getter Node? item(unsigned long index);
    [Rust=length] readonly attribute unsigned long length;
    [RustIterable=JavaScript] iterable<Node>;
};
