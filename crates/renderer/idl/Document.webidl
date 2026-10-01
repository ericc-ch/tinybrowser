// https://dom.spec.whatwg.org/#interface-document
// https://html.spec.whatwg.org/multipage/dom.html#the-document-object
// Rust, RustValue, and RustFromJs are build-time implementation mappings.
// `Document` shares the `JsNode` payload with `Node`, `Element`, and the other
// node kinds.
[Exposed=Window, Rust=JsNode]
partial interface Document {
    [RustValue, Rust=implementation] readonly attribute DOMImplementation implementation;
    [RustValue, Rust=document_element] readonly attribute Element? documentElement;
    [RustValue, Rust=body] readonly attribute HTMLElement? body;
    [RustValue, Rust=head] readonly attribute HTMLElement? head;
    [RustValue, Rust=current_script] readonly attribute HTMLOrSVGScriptElement? currentScript;
    [RustValue, Rust=doctype] readonly attribute DocumentType? doctype;
    [Rust=document_uri] readonly attribute USVString documentURI;
    [Rust=url] readonly attribute USVString URL;
    [Rust=compat_mode] readonly attribute DOMString compatMode;
    [Rust=character_set] readonly attribute DOMString characterSet;
    [Rust=charset] readonly attribute DOMString charset;
    [Rust=input_encoding] readonly attribute DOMString inputEncoding;
    [Rust=content_type] readonly attribute DOMString contentType;
    [Rust=ready_state] readonly attribute DOMString readyState;
    [RustValue, Rust=default_view] readonly attribute object? defaultView;
    [RustValue, Rust=location] readonly attribute Location? location;
    [Rust=has_focus] boolean hasFocus();
    [RustValue, Rust=active_element] readonly attribute Element? activeElement;
    [CEReactions, Rust=title, RustSet=set_title, RustSetFromJs=WebIdlCodeUnits] attribute DOMString title;
    [CEReactions, Rust=write] undefined write([RustFromJs=WebIdlString] DOMString... text);
    [RustValue, Rust=get_elements_by_class_name] HTMLCollection getElementsByClassName([RustFromJs=WebIdlString] DOMString classNames);

    [Rust=create_event] Event createEvent([RustValue] DOMString eventInterface);
    [Rust=create_element] Element createElement([RustFromJs=WebIdlString] DOMString localName);
    [Rust=create_element_ns] Element createElementNS([RustFromJs=OptString] DOMString? namespace, [RustFromJs=WebIdlString] DOMString qualifiedName);
    [Rust=create_text_node] Text createTextNode([RustFromJs=WebIdlCodeUnits] DOMString data);
    [Rust=create_comment] Comment createComment([RustFromJs=WebIdlCodeUnits] DOMString data);
    [Rust=create_processing_instruction] ProcessingInstruction createProcessingInstruction([RustFromJs=WebIdlString] DOMString target, [RustFromJs=WebIdlCodeUnits] DOMString data);
    [Rust=create_cdata_section] CDATASection createCDATASection([RustFromJs=WebIdlCodeUnits] DOMString data);
    [Rust=create_attribute] Attr createAttribute([RustFromJs=WebIdlString] DOMString localName);
    [Rust=create_attribute_ns] Attr createAttributeNS([RustFromJs=OptString] DOMString? namespace, [RustFromJs=WebIdlString] DOMString qualifiedName);
    [Rust=create_document_fragment] DocumentFragment createDocumentFragment();
    [Rust=import_node] Node importNode([RustValue] Node node, optional boolean deep = false);
    [Rust=open_document] Document open();
    [Rust=close_document] undefined close();
    [Rust=get_element_by_id] Element? getElementById([RustFromJs=WebIdlString] DOMString elementId);
    [Rust=get_elements_by_tag_name] HTMLCollection getElementsByTagName([RustFromJs=WebIdlString] DOMString qualifiedName);
    [Rust=get_elements_by_tag_name_ns] HTMLCollection getElementsByTagNameNS([RustFromJs=OptString] DOMString? namespace, [RustFromJs=WebIdlString] DOMString localName);
    [Rust=get_elements_by_name] NodeList getElementsByName([RustFromJs=WebIdlString] DOMString elementName);
    [Rust=element_from_point] Element? elementFromPoint(double x, double y);
    [Rust=elements_from_point] sequence<Element> elementsFromPoint(double x, double y);
};
