//! DOM subtree serialization: HTML and XML.
//!
//! Two algorithms live here. The HTML one is the HTML fragment
//! serialization algorithm
//! (<https://html.spec.whatwg.org/multipage/parsing.html#serialising-html-fragments>),
//! used by `innerHTML` and `outerHTML` on HTML documents. The XML one is
//! DOM Parsing's XML serialization
//! (<https://w3c.github.io/DOM-Parsing/#xml-serialization>), used by
//! `XMLSerializer` and by `innerHTML`/`outerHTML` on XML documents.
//!
//! The XML serializer follows the DOM Parsing algorithm with the
//! behaviors the WPT suite `domparsing/XMLSerializer-serializeToString.html`
//! pins down, which the August 2026 spec revision changed under browsers:
//! attribute prefixes are not preserved (a new `ns<n>` prefix is
//! generated instead), generated prefixes do not avoid local conflicts,
//! and an element's namespace is consulted for a prefix before its own
//! default declaration is considered.

use std::fmt;

use dom::{
    Attribute, Dom, LocalName, Namespace, NodeId, NodeKind, QualName, html_namespace,
    svg_namespace, xlink_namespace, xml_namespace, xmlns_namespace,
};

use crate::xml::is_valid_ncname;

/// The `MathML` namespace URL, used by the HTML serialization name rules.
const MATHML_NS: &str = "http://www.w3.org/1998/Math/MathML";

// ── HTML serialization ──────────────────────────────────────────────────────

/// Serializes the children of `element` (or of its template contents) with
/// the HTML fragment serialization algorithm.
///
/// <https://html.spec.whatwg.org/multipage/parsing.html#serialising-html-fragments>
pub(crate) fn serialize_html_fragment(dom: &Dom, element: NodeId) -> String {
    let root = dom.template_contents(element).unwrap_or(element);
    let parent = match dom.kind(element) {
        Some(NodeKind::Element { name, .. }) => Some((name.ns.clone(), name.local.clone())),
        _ => None,
    };
    let mut output = String::new();
    for child in children(dom, root) {
        serialize_html_node(
            dom,
            child,
            parent.as_ref().map(|(namespace, local)| (namespace, local)),
            &mut output,
        );
    }
    output
}

/// Serializes one element with the HTML fragment serialization algorithm.
pub(crate) fn serialize_html_element(
    dom: &Dom,
    id: NodeId,
    name: &QualName,
    attributes: &[Attribute],
    output: &mut String,
) {
    output.push('<');
    push_html_element_name(output, name);
    for attribute in attributes {
        output.push(' ');
        push_html_attribute_name(output, attribute);
        output.push_str("=\"");
        push_escaped_html_attribute(output, &attribute.value);
        output.push('"');
    }
    output.push('>');

    if serializes_as_void(name) {
        return;
    }

    let child_root = dom.template_contents(id).unwrap_or(id);
    for child in children(dom, child_root) {
        serialize_html_node(dom, child, Some((&name.ns, &name.local)), output);
    }
    output.push_str("</");
    push_html_element_name(output, name);
    output.push('>');
}

fn serialize_html_node(
    dom: &Dom,
    id: NodeId,
    parent: Option<(&Namespace, &LocalName)>,
    output: &mut String,
) {
    let Some(kind) = dom.kind(id).cloned() else {
        return;
    };
    match kind {
        NodeKind::Document | NodeKind::Fragment => {
            for child in children(dom, id) {
                serialize_html_node(dom, child, parent, output);
            }
        }
        NodeKind::Doctype { name, .. } => {
            output.push_str("<!DOCTYPE ");
            output.push_str(&name);
            output.push('>');
        }
        NodeKind::Text { data } => {
            let raw_text = parent.is_some_and(|(namespace, local)| {
                namespace == &html_namespace()
                    && matches!(
                        local.as_ref(),
                        "style"
                            | "script"
                            | "xmp"
                            | "iframe"
                            | "noembed"
                            | "noscript"
                            | "noframes"
                            | "plaintext"
                    )
            });
            if raw_text {
                output.push_str(&data);
            } else {
                push_escaped_html_text(output, &data);
            }
        }
        NodeKind::CDataSection { data } => push_escaped_html_text(output, &data),
        NodeKind::ProcessingInstruction { target, data } => {
            output.push_str("<?");
            output.push_str(&target);
            output.push(' ');
            output.push_str(&data);
            output.push('>');
        }
        NodeKind::Comment { data } => {
            output.push_str("<!--");
            output.push_str(&data);
            output.push_str("-->");
        }
        NodeKind::Element { name, attributes } => {
            serialize_html_element(dom, id, &name, &attributes, output);
        }
    }
}

/// Whether an element serializes as void
/// (<https://html.spec.whatwg.org/multipage/parsing.html#serialises-as-void>).
fn serializes_as_void(name: &QualName) -> bool {
    name.ns == html_namespace()
        && matches!(
            name.local.as_ref(),
            "area"
                | "base"
                | "basefont"
                | "bgsound"
                | "br"
                | "col"
                | "embed"
                | "frame"
                | "hr"
                | "img"
                | "input"
                | "keygen"
                | "link"
                | "menuitem"
                | "meta"
                | "param"
                | "source"
                | "track"
                | "wbr"
        )
}

/// The element's serialized name: the local name for HTML, `MathML`, and
/// SVG elements, the qualified name otherwise
/// (<https://html.spec.whatwg.org/multipage/parsing.html#serialising-html-fragments>).
fn push_html_element_name(output: &mut String, name: &QualName) {
    if name.ns == html_namespace() || name.ns == svg_namespace() || name.ns.as_ref() == MATHML_NS {
        output.push_str(name.local.as_ref());
    } else {
        push_qualified_name(output, name.prefix.as_ref(), &name.local);
    }
}

/// The attribute's serialized name
/// (<https://html.spec.whatwg.org/multipage/parsing.html#attribute-s-serialized-name>).
fn push_html_attribute_name(output: &mut String, attribute: &Attribute) {
    let name = &attribute.name;
    if name.ns.is_empty() {
        output.push_str(name.local.as_ref());
    } else if name.ns == xml_namespace() {
        output.push_str("xml:");
        output.push_str(name.local.as_ref());
    } else if name.ns == xmlns_namespace() {
        if name.local.as_ref() == "xmlns" {
            output.push_str("xmlns");
        } else {
            output.push_str("xmlns:");
            output.push_str(name.local.as_ref());
        }
    } else if name.ns == xlink_namespace() {
        output.push_str("xlink:");
        output.push_str(name.local.as_ref());
    } else {
        push_qualified_name(output, name.prefix.as_ref(), &name.local);
    }
}

fn push_qualified_name(output: &mut String, prefix: Option<&dom::Prefix>, local: &LocalName) {
    if let Some(prefix) = prefix {
        output.push_str(prefix.as_ref());
        output.push(':');
    }
    output.push_str(local.as_ref());
}

fn push_escaped_html_text(output: &mut String, text: &str) {
    for character in text.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '\u{00a0}' => output.push_str("&nbsp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            _ => output.push(character),
        }
    }
}

fn push_escaped_html_attribute(output: &mut String, value: &str) {
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '\u{00a0}' => output.push_str("&nbsp;"),
            '"' => output.push_str("&quot;"),
            _ => output.push(character),
        }
    }
}

// ── XML serialization ───────────────────────────────────────────────────────

const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";

/// Why a subtree cannot be serialized to XML with the well-formedness
/// checks enabled. Callers map this to `InvalidStateError`.
#[derive(Debug)]
pub(crate) struct XmlSerializeError;

impl fmt::Display for XmlSerializeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the node cannot be serialized to well-formed XML")
    }
}

impl std::error::Error for XmlSerializeError {}

/// XML-serializes `node` itself, the entry point `XMLSerializer` and
/// `outerHTML` share.
///
/// <https://w3c.github.io/DOM-Parsing/#xml-serialization>
pub(crate) fn serialize_xml(
    dom: &Dom,
    node: NodeId,
    require_well_formed: bool,
) -> Result<String, XmlSerializeError> {
    serialize_in_context(dom, require_well_formed, |serializer, map, output| {
        serializer.node(node, None, map, output)
    })
}

/// XML-serializes the children of `parent` with no namespace in scope,
/// the entry point `innerHTML` uses on XML documents.
pub(crate) fn serialize_xml_children(
    dom: &Dom,
    parent: NodeId,
    require_well_formed: bool,
) -> Result<String, XmlSerializeError> {
    serialize_in_context(dom, require_well_formed, |serializer, map, output| {
        for child in children(dom, parent) {
            serializer.node(child, None, map, output)?;
        }
        Ok(())
    })
}

/// Runs one XML serialization with the reserved `xml` prefix bound, the
/// initial prefix map of every entry point
/// (<https://w3c.github.io/DOM-Parsing/#dfn-xml-serialization-algorithm>).
fn serialize_in_context(
    dom: &Dom,
    require_well_formed: bool,
    write: impl FnOnce(&mut XmlSerializer<'_>, &PrefixMap, &mut String) -> Result<(), XmlSerializeError>,
) -> Result<String, XmlSerializeError> {
    let mut serializer = XmlSerializer::new(dom, require_well_formed);
    let mut map = PrefixMap::default();
    map.add(Some(XML_NS), "xml");
    let mut output = String::new();
    write(&mut serializer, &map, &mut output)?;
    Ok(output)
}

/// What the element step already decided about the element's default
/// namespace declaration, which tells attribute serialization how to treat
/// the `xmlns` declarations in the attribute list.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DefaultDeclarationHandling {
    /// Serialize declarations from the attribute list as they are.
    Keep,
    /// The element is in the context namespace; a matching declaration is
    /// redundant and dropped.
    Ignore,
    /// The element step emitted a replacement declaration; every
    /// declaration in the attribute list is dropped to avoid a duplicate.
    Replace,
}

struct XmlSerializer<'a> {
    dom: &'a Dom,
    require_well_formed: bool,
    prefix_index: u32,
}

impl<'a> XmlSerializer<'a> {
    fn new(dom: &'a Dom, require_well_formed: bool) -> Self {
        Self {
            dom,
            require_well_formed,
            prefix_index: 1,
        }
    }

    /// The XML serialization algorithm
    /// (<https://w3c.github.io/DOM-Parsing/#dfn-xml-serialization-algorithm>).
    fn node(
        &mut self,
        id: NodeId,
        context: Option<&str>,
        map: &PrefixMap,
        output: &mut String,
    ) -> Result<(), XmlSerializeError> {
        let Some(kind) = self.dom.kind(id).cloned() else {
            return Err(XmlSerializeError);
        };
        match kind {
            NodeKind::Document => {
                if self.require_well_formed && !has_document_element(self.dom, id) {
                    return Err(XmlSerializeError);
                }
                for child in children(self.dom, id) {
                    self.node(child, context, map, output)?;
                }
                Ok(())
            }
            NodeKind::Fragment => {
                for child in children(self.dom, id) {
                    self.node(child, context, map, output)?;
                }
                Ok(())
            }
            NodeKind::Element { name, attributes } => {
                self.element(id, &name, &attributes, context, map, output)
            }
            NodeKind::Text { data } => {
                if self.require_well_formed && !data.chars().all(is_xml_char) {
                    return Err(XmlSerializeError);
                }
                push_escaped_xml_text(output, &data);
                Ok(())
            }
            NodeKind::CDataSection { data } => {
                output.push_str("<![CDATA[");
                output.push_str(&data);
                output.push_str("]]>");
                Ok(())
            }
            NodeKind::Comment { data } => {
                if self.require_well_formed
                    && (!data.chars().all(is_xml_char)
                        || data.contains("--")
                        || data.ends_with('-'))
                {
                    return Err(XmlSerializeError);
                }
                output.push_str("<!--");
                output.push_str(&data);
                output.push_str("-->");
                Ok(())
            }
            NodeKind::ProcessingInstruction { target, data } => {
                if self.require_well_formed
                    && (target.contains(':')
                        || target.eq_ignore_ascii_case("xml")
                        || !data.chars().all(is_xml_char)
                        || data.contains("?>"))
                {
                    return Err(XmlSerializeError);
                }
                output.push_str("<?");
                output.push_str(&target);
                if !data.is_empty() {
                    output.push(' ');
                    output.push_str(&data);
                }
                output.push_str("?>");
                Ok(())
            }
            NodeKind::Doctype {
                name,
                public_id,
                system_id,
            } => {
                if self.require_well_formed
                    && (!public_id.chars().all(is_pubid_char)
                        || !system_id.chars().all(is_xml_char)
                        || (system_id.contains('"') && system_id.contains('\'')))
                {
                    return Err(XmlSerializeError);
                }
                output.push_str("<!DOCTYPE ");
                output.push_str(&name);
                if !public_id.is_empty() {
                    output.push_str(" PUBLIC ");
                    push_xml_identifier(output, &public_id);
                    if !system_id.is_empty() {
                        output.push(' ');
                        push_xml_identifier(output, &system_id);
                    }
                } else if !system_id.is_empty() {
                    output.push_str(" SYSTEM ");
                    push_xml_identifier(output, &system_id);
                }
                output.push('>');
                Ok(())
            }
        }
    }

    /// XML serializing an Element node, including its attributes and
    /// children (<https://w3c.github.io/DOM-Parsing/#xml-serializing-an-element-node>).
    fn element(
        &mut self,
        id: NodeId,
        name: &QualName,
        attributes: &[Attribute],
        context: Option<&str>,
        inherited_map: &PrefixMap,
        output: &mut String,
    ) -> Result<(), XmlSerializeError> {
        if self.require_well_formed && !is_valid_ncname(&name.local) {
            return Err(XmlSerializeError);
        }
        let ns = namespace_str(&name.ns);
        let mut scope = ElementNamespaces {
            map: inherited_map.clone(),
            local_prefixes: Vec::new(),
        };
        let local_default =
            record_namespace_information(attributes, &mut scope.map, &mut scope.local_prefixes);
        let mut child_context = context.map(str::to_owned);
        let mut defaults = DefaultDeclarationHandling::Keep;
        let qualified;

        output.push('<');
        if context == ns {
            // The node is in the current default namespace; its prefix is
            // dropped and any matching default declaration becomes
            // ignorable.
            defaults = if local_default.is_some() {
                DefaultDeclarationHandling::Ignore
            } else {
                DefaultDeclarationHandling::Keep
            };
            if ns == Some(XML_NS) {
                qualified = format!("xml:{}", name.local);
            } else {
                qualified = name.local.to_string();
            }
            output.push_str(&qualified);
        } else {
            let prefix = name
                .prefix
                .as_ref()
                .map(dom::Prefix::as_ref)
                .filter(|prefix| !prefix.is_empty());
            let candidate = if ns.is_some() {
                scope.map.retrieve(ns, prefix)
            } else {
                None
            };
            match candidate {
                Some(candidate) => {
                    qualified = format!("{candidate}:{}", name.local);
                    output.push_str(&qualified);
                    if let Some(value) = &local_default
                        && value != XML_NS
                    {
                        child_context = inherit_default(value);
                    }
                }
                None => {
                    if let Some(prefix) = prefix {
                        let mut resolved = prefix.to_owned();
                        if scope
                            .local_prefixes
                            .iter()
                            .any(|(name, _)| *name == resolved)
                        {
                            resolved = self.generate_prefix(&mut scope.map, ns);
                        } else {
                            scope.map.add(ns, &resolved);
                        }
                        qualified = format!("{resolved}:{}", name.local);
                        output.push_str(&qualified);
                        push_xmlns(output, Some(&resolved), ns.unwrap_or(""));
                        if let Some(value) = &local_default {
                            child_context = inherit_default(value);
                        }
                    } else {
                        let declared = local_default
                            .as_deref()
                            .is_some_and(|value| normalize(value) == ns);
                        qualified = name.local.to_string();
                        output.push_str(&qualified);
                        if declared {
                            child_context = ns.map(str::to_owned);
                        } else {
                            push_xmlns(output, None, ns.unwrap_or(""));
                            child_context = ns.map(str::to_owned);
                            defaults = DefaultDeclarationHandling::Replace;
                        }
                    }
                }
            }
        }

        self.attributes(attributes, &mut scope, defaults, ns, output)?;
        let serialized = SerializedElement {
            name,
            qualified: &qualified,
            namespace: ns,
            context: child_context.as_deref(),
        };
        self.finish_element(id, &serialized, &scope.map, output)
    }

    /// XML serialization of the attributes
    /// (<https://w3c.github.io/DOM-Parsing/#serializing-an-element-s-attributes>).
    fn attributes(
        &mut self,
        attributes: &[Attribute],
        scope: &mut ElementNamespaces,
        defaults: DefaultDeclarationHandling,
        element_ns: Option<&str>,
        output: &mut String,
    ) -> Result<(), XmlSerializeError> {
        for attribute in attributes {
            let attribute_ns = namespace_str(&attribute.name.ns);
            let is_xmlns = attribute.name.ns == xmlns_namespace();
            let local = attribute.name.local.as_ref();
            if self.require_well_formed
                && (!is_valid_ncname(local) || (attribute_ns.is_none() && local == "xmlns"))
            {
                return Err(XmlSerializeError);
            }

            if is_default_declaration(attribute) {
                let value_ns = normalize(&attribute.value);
                let dropped = match defaults {
                    DefaultDeclarationHandling::Keep => false,
                    DefaultDeclarationHandling::Replace => true,
                    DefaultDeclarationHandling::Ignore => {
                        // A redundant declaration is dropped, but one that
                        // carries the element's own namespace stays when a
                        // prefix declaration on the same element is bound to
                        // no namespace (the WPT suite requires
                        // `xmlns="" xmlns:foo=""` to round-trip).
                        value_ns != element_ns || scope.local_prefixes.is_empty()
                    }
                };
                if dropped {
                    continue;
                }
                push_xmlns(output, None, &attribute.value);
                continue;
            }

            let prefix = attribute
                .name
                .prefix
                .as_ref()
                .map(dom::Prefix::as_ref)
                .filter(|prefix| !prefix.is_empty());
            let mut candidate = if is_xmlns {
                if attribute.value == XML_NS {
                    continue;
                }
                if self.require_well_formed
                    && attribute.name.local.as_ref() != "xmlns"
                    && (attribute.value.is_empty() || attribute.value == xmlns_namespace().as_ref())
                {
                    return Err(XmlSerializeError);
                }
                let declared_here = scope.local_prefixes.iter().any(|(name, ns)| {
                    name == local && ns.as_deref() == normalize(&attribute.value)
                });
                if scope
                    .map
                    .contains_prefix(normalize(&attribute.value), local)
                    && !declared_here
                {
                    continue;
                }
                Some("xmlns".to_owned())
            } else if let Some(attribute_ns) = attribute_ns {
                scope.map.retrieve(Some(attribute_ns), prefix)
            } else {
                None
            };
            if let Some(attribute_ns) = attribute_ns
                && candidate.is_none()
            {
                // The WPT suite pins two attribute-prefix behaviors that no
                // single browser implements: a new attribute namespace is
                // renamed to a generated `ns<n>` prefix when the element's
                // namespace scope already declares prefixes
                // (`Check if the prefix of an attribute is NOT preserved…`),
                // but keeps the attribute's own prefix when the scope holds
                // nothing but the reserved `xml` binding (`Check if no
                // special handling for XLink namespace…`). Follow the suite.
                let generated = match prefix.filter(|_| scope.map.binds_only_xml()) {
                    Some(preferred) => {
                        scope.map.add(Some(attribute_ns), preferred);
                        preferred.to_owned()
                    }
                    None => self.generate_prefix(&mut scope.map, Some(attribute_ns)),
                };
                scope
                    .local_prefixes
                    .push((generated.clone(), Some(attribute_ns.to_owned())));
                push_xmlns(output, Some(&generated), attribute_ns);
                candidate = Some(generated);
            }
            output.push(' ');
            if let Some(candidate) = candidate {
                output.push_str(&candidate);
                output.push(':');
            }
            output.push_str(local);
            output.push_str("=\"");
            push_escaped_xml_attribute(output, &attribute.value);
            output.push('"');
        }
        Ok(())
    }

    fn finish_element(
        &mut self,
        id: NodeId,
        serialized: &SerializedElement<'_>,
        map: &PrefixMap,
        output: &mut String,
    ) -> Result<(), XmlSerializeError> {
        let SerializedElement {
            name,
            qualified,
            namespace: ns,
            context: child_context,
        } = *serialized;
        let element_children = children(self.dom, id);
        let in_html = ns == Some(html_namespace().as_ref());
        let has_children = !element_children.is_empty();
        if in_html && !has_children && serializes_as_void(name) {
            output.push_str(" />");
            return Ok(());
        }
        if !in_html && !has_children {
            output.push_str("/>");
            return Ok(());
        }
        output.push('>');
        let contents = if in_html && name.local.as_ref() == "template" {
            self.dom.template_contents(id)
        } else {
            None
        };
        match contents {
            Some(contents) => {
                for child in children(self.dom, contents) {
                    self.node(child, child_context, map, output)?;
                }
            }
            None => {
                for child in element_children {
                    self.node(child, child_context, map, output)?;
                }
            }
        }
        output.push_str("</");
        output.push_str(qualified);
        output.push('>');
        Ok(())
    }

    /// Generates a namespace prefix
    /// (<https://w3c.github.io/DOM-Parsing/#dfn-generating-a-prefix>).
    ///
    /// The spec's algorithm does not avoid an existing `ns<n>` binding,
    /// and the WPT suite pins that down (`"ns1" is generated even if the
    /// element already has xmlns:ns1`).
    fn generate_prefix(&mut self, map: &mut PrefixMap, ns: Option<&str>) -> String {
        let prefix = format!("ns{}", self.prefix_index);
        self.prefix_index += 1;
        map.add(ns, &prefix);
        prefix
    }
}

/// Records the namespace information of one element's attribute list
/// (<https://w3c.github.io/DOM-Parsing/#dfn-recording-the-namespace-information>).
///
/// Returns the default declaration's value, if there is one. `xmlns`
/// attributes created through `setAttribute` have no namespace, but
/// serialize as declarations, so they take part too.
fn record_namespace_information(
    attributes: &[Attribute],
    map: &mut PrefixMap,
    local_prefixes: &mut Vec<(String, Option<String>)>,
) -> Option<String> {
    let mut local_default = None;
    for attribute in attributes {
        if is_default_declaration(attribute) {
            local_default = Some(attribute.value.clone());
        } else if attribute.name.prefix.as_deref() == Some("xmlns") {
            let prefix = attribute.name.local.to_string();
            let namespace = normalize(&attribute.value);
            if namespace == Some(XML_NS) {
                continue;
            }
            if map.contains_prefix(namespace, &prefix) {
                continue;
            }
            map.add(namespace, &prefix);
            local_prefixes.push((prefix, namespace.map(str::to_owned)));
        }
    }
    local_default
}

/// A default namespace declaration: an unprefixed `xmlns`, in the XMLNS
/// namespace (parsed) or not (created with `setAttribute`).
fn is_default_declaration(attribute: &Attribute) -> bool {
    attribute.name.prefix.is_none()
        && (attribute.name.ns == xmlns_namespace() || attribute.name.local.as_ref() == "xmlns")
}

/// The element's namespace after a locally declared default declaration;
/// the empty declaration resets it to no namespace.
fn inherit_default(value: &str) -> Option<String> {
    normalize(value).map(str::to_owned)
}

/// `Some` for a namespace that names a real namespace, `None` for the
/// empty string (no namespace).
fn normalize(value: &str) -> Option<&str> {
    (!value.is_empty()).then_some(value)
}

fn namespace_str(namespace: &Namespace) -> Option<&str> {
    normalize(namespace.as_ref())
}

fn push_escaped_xml_text(output: &mut String, text: &str) {
    for character in text.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            _ => output.push(character),
        }
    }
}

/// Appends one namespace declaration, `xmlns="value"` or
/// `xmlns:prefix="value"`
/// (<https://w3c.github.io/DOM-Parsing/#dfn-xml-serializing-an-element-node>).
fn push_xmlns(output: &mut String, prefix: Option<&str>, value: &str) {
    output.push_str(" xmlns");
    if let Some(prefix) = prefix {
        output.push(':');
        output.push_str(prefix);
    }
    output.push_str("=\"");
    push_escaped_xml_attribute(output, value);
    output.push('"');
}

fn push_escaped_xml_attribute(output: &mut String, value: &str) {
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '"' => output.push_str("&quot;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '\t' => output.push_str("&#x9;"),
            '\n' => output.push_str("&#xA;"),
            '\r' => output.push_str("&#xD;"),
            _ => output.push(character),
        }
    }
}

/// Serializes a doctype identifier, choosing the quote the value does not
/// contain (<https://w3c.github.io/DOM-Parsing/#dfn-serialization-of-the-id>).
fn push_xml_identifier(output: &mut String, identifier: &str) {
    let quote = if identifier.contains('"') { '\'' } else { '"' };
    output.push(quote);
    output.push_str(identifier);
    output.push(quote);
}

fn has_document_element(dom: &Dom, document: NodeId) -> bool {
    children(dom, document)
        .into_iter()
        .any(|child| matches!(dom.kind(child), Some(NodeKind::Element { .. })))
}

fn children(dom: &Dom, parent: NodeId) -> Vec<NodeId> {
    dom.children(parent)
        .map(Iterator::collect)
        .unwrap_or_default()
}

/// The XML `Char` production (<https://www.w3.org/TR/xml/#NT-Char>).
fn is_xml_char(character: char) -> bool {
    matches!(character, '\u{9}' | '\u{A}' | '\u{D}')
        || ('\u{20}'..='\u{D7FF}').contains(&character)
        || ('\u{E000}'..='\u{FFFD}').contains(&character)
        || ('\u{10000}'..='\u{10FFFF}').contains(&character)
}

/// The XML `PubidChar` production (<https://www.w3.org/TR/xml/#NT-PubidChar>).
fn is_pubid_char(character: char) -> bool {
    matches!(character, ' ' | '\r' | '\n')
        || character.is_ascii_alphanumeric()
        || matches!(
            character,
            '-' | '\''
                | '('
                | ')'
                | '+'
                | ','
                | '.'
                | '/'
                | ':'
                | '='
                | '?'
                | ';'
                | '!'
                | '*'
                | '#'
                | '@'
                | '$'
                | '_'
                | '%'
        )
}

/// The namespace state one element's serialization works on: a copy of the
/// inherited prefix map plus the declarations the element itself carries.
struct ElementNamespaces {
    map: PrefixMap,
    local_prefixes: Vec<(String, Option<String>)>,
}

/// The resolved start tag of one element, carried from serialization of the
/// opening tag to the end tag and the children.
struct SerializedElement<'a> {
    name: &'a QualName,
    qualified: &'a str,
    namespace: Option<&'a str>,
    context: Option<&'a str>,
}

/// The namespace prefix map
/// (<https://w3c.github.io/DOM-Parsing/#the-namespace-prefix-map>): an
/// ordered map from a namespace (or no namespace) to the prefixes that
/// currently map to it, most recently recorded last.
#[derive(Clone, Default)]
struct PrefixMap {
    entries: Vec<(Option<String>, Vec<String>)>,
}

impl PrefixMap {
    fn add(&mut self, namespace: Option<&str>, prefix: &str) {
        if let Some((_, prefixes)) = self
            .entries
            .iter_mut()
            .find(|(key, _)| key_matches(key.as_deref(), namespace))
        {
            prefixes.push(prefix.to_owned());
        } else {
            self.entries
                .push((namespace.map(str::to_owned), vec![prefix.to_owned()]));
        }
    }

    fn prefixes(&self, namespace: Option<&str>) -> Option<&[String]> {
        self.entries
            .iter()
            .find(|(key, _)| key_matches(key.as_deref(), namespace))
            .map(|(_, prefixes)| prefixes.as_slice())
    }

    fn contains_prefix(&self, namespace: Option<&str>, prefix: &str) -> bool {
        self.prefixes(namespace)
            .is_some_and(|prefixes| prefixes.iter().any(|candidate| candidate == prefix))
    }

    /// Whether the reserved `xml` binding is the only one in scope.
    fn binds_only_xml(&self) -> bool {
        self.entries
            .iter()
            .all(|(namespace, _)| namespace.as_deref() == Some(XML_NS))
    }

    /// Retrieving a preferred prefix string
    /// (<https://w3c.github.io/DOM-Parsing/#dfn-retrieving-a-preferred-prefix-string>).
    fn retrieve(&self, namespace: Option<&str>, preferred: Option<&str>) -> Option<String> {
        let prefixes = self.prefixes(namespace)?;
        if let Some(preferred) = preferred
            && prefixes.iter().any(|candidate| candidate == preferred)
        {
            return Some(preferred.to_owned());
        }
        prefixes.last().cloned()
    }
}

fn key_matches(key: Option<&str>, namespace: Option<&str>) -> bool {
    match (key, namespace) {
        (None, None) => true,
        (Some(key), Some(namespace)) => key == namespace,
        _ => false,
    }
}
