// https://dom.spec.whatwg.org/#interface-element
// Rust, RustValue, RustSetFromJs, and RustFromJs are build-time implementation
// mappings. `Element` shares the `JsNode` payload with `Node`.
[Exposed=Window, Rust=JsNode]
partial interface Element {
    [Rust=get_elements_by_tag_name] HTMLCollection getElementsByTagName([RustFromJs=WebIdlString] DOMString qualifiedName);
    [Rust=get_elements_by_tag_name_ns] HTMLCollection getElementsByTagNameNS([RustFromJs=OptString] DOMString? namespace, [RustFromJs=WebIdlString] DOMString localName);
    [RustValue, Rust=get_elements_by_class_name] HTMLCollection getElementsByClassName([RustFromJs=WebIdlString] DOMString classNames);
    [Rust=matches] boolean matches([RustFromJs=WebIdlString] DOMString selectors);
    [RustValue, Rust=closest] Element? closest([RustFromJs=WebIdlString] DOMString selectors);

    // The `style` attribute is readonly with `[PutForwards=cssText]` in CSSOM;
    // here the platform setter reflects the content attribute, which is the
    // same observable behavior.
    [SameObject, RustValue, Rust=style, RustSet=set_style, RustSetFromJs=WebIdlString] attribute CSSStyleDeclaration style;
    [CEReactions, Rust=inner_html, RustSet=set_inner_html, RustSetFromJs=LegacyNullString] attribute [LegacyNullToEmptyString] DOMString innerHTML;
    [CEReactions, Rust=outer_html, RustSet=set_outer_html, RustSetFromJs=LegacyNullString] attribute [LegacyNullToEmptyString] DOMString outerHTML;
    [CEReactions, Rust=insert_adjacent_html] undefined insertAdjacentHTML([RustFromJs=WebIdlString] DOMString position, [RustFromJs=WebIdlString] DOMString text);

    // https://drafts.csswg.org/cssom-view/#extension-to-the-element-interface
    [RustValue, Rust=get_bounding_client_rect] DOMRect getBoundingClientRect();
    [RustValue, Rust=get_client_rects] DOMRectList getClientRects();
    [Rust=scroll_into_view] undefined scrollIntoView(optional [RustValue] (boolean or ScrollIntoViewOptions) arg);
    [Rust=scroll_left, RustSet=set_scroll_left] attribute double scrollLeft;
    [Rust=scroll_top, RustSet=set_scroll_top] attribute double scrollTop;

    // https://dom.spec.whatwg.org/#ref-for-dom-element-attachshadow
    [RustValue, Rust=attach_shadow] ShadowRoot attachShadow([RustValue] ShadowRootInit init);
    [RustValue, Rust=shadow_root] readonly attribute ShadowRoot? shadowRoot;

    [Rust=tag_name] readonly attribute DOMString tagName;
    [Rust=local_name] readonly attribute DOMString localName;
    [RustValue, Rust=prefix] readonly attribute DOMString? prefix;
    [RustValue, Rust=namespace_uri] readonly attribute DOMString? namespaceURI;
    [CEReactions, Rust=id, RustSet=set_id, RustSetFromJs=WebIdlString] attribute DOMString id;
    [CEReactions, Rust=class_name, RustSet=set_class_name, RustSetFromJs=WebIdlString] attribute DOMString className;
    [SameObject, PutForwards=value, RustValue, Rust=class_list] readonly attribute DOMTokenList classList;

    [Rust=get_attribute] DOMString? getAttribute([RustFromJs=WebIdlString] DOMString qualifiedName);
    [CEReactions, Rust=set_attribute] undefined setAttribute([RustFromJs=WebIdlString] DOMString qualifiedName, [RustFromJs=WebIdlString] (TrustedType or DOMString) value);
    [Rust=has_attribute] boolean hasAttribute([RustFromJs=WebIdlString] DOMString qualifiedName);
    [CEReactions, Rust=remove_attribute] undefined removeAttribute([RustFromJs=WebIdlString] DOMString qualifiedName);
    [CEReactions, Rust=toggle_attribute] boolean toggleAttribute([RustFromJs=WebIdlString] DOMString qualifiedName, optional boolean force);
    [Rust=get_attribute_ns] DOMString? getAttributeNS([RustFromJs=OptString] DOMString? namespace, [RustFromJs=WebIdlString] DOMString localName);
    [CEReactions, Rust=set_attribute_ns] undefined setAttributeNS([RustFromJs=OptString] DOMString? namespace, [RustFromJs=WebIdlString] DOMString qualifiedName, [RustFromJs=WebIdlString] (TrustedType or DOMString) value);
    [Rust=has_attribute_ns] boolean hasAttributeNS([RustFromJs=OptString] DOMString? namespace, [RustFromJs=WebIdlString] DOMString localName);
    [CEReactions, Rust=remove_attribute_ns] undefined removeAttributeNS([RustFromJs=OptString] DOMString? namespace, [RustFromJs=WebIdlString] DOMString localName);
    [Rust=get_attribute_names] sequence<DOMString> getAttributeNames();
    [SameObject, RustValue, Rust=attributes] readonly attribute NamedNodeMap attributes;
    [Rust=has_attributes] boolean hasAttributes();
    [Rust=get_attribute_node] Attr? getAttributeNode([RustFromJs=WebIdlString] DOMString qualifiedName);
    [Rust=get_attribute_node_ns] Attr? getAttributeNodeNS([RustFromJs=OptString] DOMString? namespace, [RustFromJs=WebIdlString] DOMString localName);
    [CEReactions, Rust=set_attribute_node] Attr? setAttributeNode([RustFromJs=AttrArgument] Attr attr);
    [CEReactions, Rust=set_attribute_node_ns] Attr? setAttributeNodeNS([RustFromJs=AttrArgument] Attr attr);
    [CEReactions, Rust=remove_attribute_node] Attr removeAttributeNode([RustFromJs=AttrArgument] Attr attr);
};
