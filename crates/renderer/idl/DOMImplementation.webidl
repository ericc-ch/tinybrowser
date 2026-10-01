// https://dom.spec.whatwg.org/#interface-domimplementation
// Rust is a build-time implementation mapping.
[Exposed=Window, Rust=JsImplementation]
interface DOMImplementation {
    [NewObject, Rust=create_document_type] DocumentType createDocumentType(DOMString name, DOMString publicId, DOMString systemId);
    [NewObject, Rust=create_document] XMLDocument createDocument(DOMString? namespace, [LegacyNullToEmptyString] DOMString qualifiedName, optional DocumentType? doctype = null);
    [NewObject, Rust=create_html_document] Document createHTMLDocument(optional DOMString title);
    [Rust=has_feature] boolean hasFeature();
};
