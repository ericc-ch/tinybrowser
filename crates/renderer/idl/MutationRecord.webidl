// https://dom.spec.whatwg.org/#interface-mutationrecord
[Exposed=Window, Rust=JsMutationRecord, RustLifetime]
interface MutationRecord {
    [Rust=record_type] readonly attribute DOMString type;
    [SameObject, RustField=target] readonly attribute Node target;
    [SameObject, RustField=added_nodes] readonly attribute NodeList addedNodes;
    [SameObject, RustField=removed_nodes] readonly attribute NodeList removedNodes;
    [Rust=previous_sibling] readonly attribute Node? previousSibling;
    [Rust=next_sibling] readonly attribute Node? nextSibling;
    [Rust=attribute_name] readonly attribute DOMString? attributeName;
    [Rust=attribute_namespace] readonly attribute DOMString? attributeNamespace;
    [Rust=old_value] readonly attribute DOMString? oldValue;
};
