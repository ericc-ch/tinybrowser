// https://dom.spec.whatwg.org/#interface-documenttype
[Exposed=Window, Rust=JsNode]
partial interface DocumentType {
    [Rust=document_type_name] readonly attribute DOMString name;
    [Rust=public_id] readonly attribute DOMString publicId;
    [Rust=system_id] readonly attribute DOMString systemId;
};
