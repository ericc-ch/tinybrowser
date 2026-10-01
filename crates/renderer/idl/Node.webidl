// https://dom.spec.whatwg.org/#interface-node
dictionary GetRootNodeOptions {
    [Rust=composed] boolean composed = false;
};
[Exposed=Window, Rust=JsNode, RustAlternate=JsAttr, RustAlternateLifetime]
partial interface Node {
    // https://dom.spec.whatwg.org/#interface-eventtarget
    [Rust=add_event_listener] undefined addEventListener(DOMString type, [RustValue] EventListener? callback, [RustValue] optional (AddEventListenerOptions or boolean) options = {});
    [Rust=remove_event_listener] undefined removeEventListener(DOMString type, [RustValue] EventListener? callback, [RustValue] optional (EventListenerOptions or boolean) options = {});
    [Rust=dispatch_event] boolean dispatchEvent([RustValue] Event event);
    [Rust=node_type] readonly attribute unsigned short nodeType;
    [Rust=node_name] readonly attribute DOMString nodeName;
    [Rust=base_uri] readonly attribute USVString baseURI;
    [Rust=is_connected] readonly attribute boolean isConnected;
    [Rust=owner_document] readonly attribute Document? ownerDocument;
    [Rust=parent_element] readonly attribute Element? parentElement;
    [SameObject, Rust=child_nodes] readonly attribute NodeList childNodes;
    [Rust=first_child] readonly attribute Node? firstChild;
    [Rust=last_child] readonly attribute Node? lastChild;
    [Rust=next_sibling] readonly attribute Node? nextSibling;
    [Rust=previous_sibling] readonly attribute Node? previousSibling;
    [Rust=parent_node] readonly attribute Node? parentNode;
    [CEReactions, Rust=node_value, RustSet=set_node_value] attribute DOMString? nodeValue;
    [CEReactions, Rust=text_content, RustSet=set_text_content] attribute DOMString? textContent;
    [CEReactions, Rust=append_child] Node appendChild(Node node);
    [CEReactions, Rust=insert_before] Node insertBefore(Node node, Node? child);
    [CEReactions, Rust=remove_child] Node removeChild(Node child);
    [CEReactions, Rust=replace_child] Node replaceChild(Node node, Node child);
    [Rust=has_child_nodes] boolean hasChildNodes();
    [Rust=get_root_node] Node getRootNode(optional GetRootNodeOptions options = {});
    [CEReactions, Rust=normalize] undefined normalize();
    [NewObject, CEReactions, Rust=clone_node] Node cloneNode(optional boolean deep = false);
    [Rust=is_equal_node] boolean isEqualNode(Node? otherNode);
    [Rust=is_same_node] boolean isSameNode(Node? otherNode);
    [Rust=compare_document_position] unsigned short compareDocumentPosition(Node other);
    [Rust=contains] boolean contains(Node? other);
    [Rust=lookup_prefix] DOMString? lookupPrefix(DOMString? namespace);
    [Rust=lookup_namespace_uri] DOMString? lookupNamespaceURI(DOMString? prefix);
    [Rust=is_default_namespace] boolean isDefaultNamespace(DOMString? namespace);
};
