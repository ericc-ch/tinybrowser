// https://dom.spec.whatwg.org/#interface-node
[Exposed=Window, Rust=JsNode]
partial interface Node {
    [Rust=node_type] readonly attribute unsigned short nodeType;
    [Rust=node_name] readonly attribute DOMString nodeName;
    [Rust=first_child] readonly attribute Node? firstChild;
    [Rust=last_child] readonly attribute Node? lastChild;
    [Rust=next_sibling] readonly attribute Node? nextSibling;
    [Rust=previous_sibling] readonly attribute Node? previousSibling;
    [Rust=parent_node] readonly attribute Node? parentNode;
    [Rust=append_child] Node appendChild(Node node);
    [Rust=insert_before] Node insertBefore(Node node, Node? child);
    [Rust=remove_child] Node removeChild(Node child);
    [Rust=replace_child] Node replaceChild(Node node, Node child);
};
