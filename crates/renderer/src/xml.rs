//! XML parsing for `DOMParser`'s XML MIME types.
//!
//! Delegates to `blitz-html`'s XML path. Malformed input yields a document
//! holding a `parsererror` element, matching the HTML XML parsing rules.

use std::collections::HashSet;
use std::sync::Mutex;

use crate::Parsed;

static MIME_ESSENCES: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);

/// Parses `input` as an XML document with the given content type.
pub(crate) fn parse_document(
    input: &str,
    content_type: &'static str,
    base_url: String,
    font_ctx: parley::FontContext,
) -> Parsed {
    parse_with_config(
        input,
        content_type,
        crate::ReadyState::Complete,
        blitz_dom::DocumentConfig {
            base_url: Some(base_url),
            font_ctx: Some(font_ctx),
            ua_stylesheets: Some(Vec::new()),
            ..blitz_dom::DocumentConfig::default()
        },
    )
}

/// Parses a navigated XML response with the frame's document config.
///
/// XML documents are always no-quirks
/// (<https://dom.spec.whatwg.org/#concept-document-quirks>).
pub(crate) fn parse_navigated(
    input: &str,
    content_type: &'static str,
    config: blitz_dom::DocumentConfig,
) -> Parsed {
    parse_with_config(input, content_type, crate::ReadyState::Loading, config)
}

fn parse_with_config(
    input: &str,
    content_type: &'static str,
    ready_state: crate::ReadyState,
    config: blitz_dom::DocumentConfig,
) -> Parsed {
    let base: blitz_dom::BaseDocument = blitz_html::HtmlDocument::from_xml(input, config).into();
    let mut document = crate::documents::BlitzDocument::from_base(base);
    // The XML sink's `append_doctype_to_document` ignores the token. The
    // document type declaration is still a child of the document
    // (<https://www.w3.org/TR/xml/#NT-doctypedecl>).
    crate::attach_leading_doctype(&mut document, input, true);
    // `create_pi` stores an empty comment and drops the target and data.
    // A processing instruction is a real node
    // (<https://dom.spec.whatwg.org/#concept-node-pi>). The XML declaration
    // is not a node
    // (<https://www.w3.org/TR/xml/#sec-prolog-dtd>).
    attach_xml_processing_instructions(&mut document, input);
    // xml5ever binds xmlns declarations then drops those attributes from the
    // element, so lookupPrefix/lookupNamespaceURI cannot see them
    // (xml5ever `process_namespaces`). Namespace declaration attributes stay
    // on the element
    // (<https://dom.spec.whatwg.org/#locate-a-namespace-prefix>).
    attach_xml_namespace_declarations(&mut document, input);
    Parsed {
        id: 0,
        document,
        quirks_mode: markup5ever::interface::QuirksMode::NoQuirks,
        content_type,
        xml_document: true,
        ready_state,
        url: None,
    }
}

/// The document content type when `header` is an XML MIME type, otherwise
/// `None` so the response stays on the HTML parser.
///
/// An XML MIME type has essence `text/xml` or `application/xml`, or a subtype
/// ending in `+xml` (<https://mimesniff.spec.whatwg.org/#xml-mime-type>).
pub(crate) fn navigated_content_type(header: &str) -> Option<&'static str> {
    let essence = header.split(';').next()?.trim().to_ascii_lowercase();
    let (kind, subtype) = essence.split_once('/')?;
    if kind.is_empty() || !(subtype == "xml" || subtype.ends_with("+xml")) {
        return None;
    }
    Some(match essence.as_str() {
        "text/xml" => "text/xml",
        "application/xhtml+xml" => "application/xhtml+xml",
        "image/svg+xml" => "image/svg+xml",
        // `application/xml` and every other XML MIME type.
        _ => "application/xml",
    })
}

/// The [document's content type] from a navigated response's `Content-Type`.
///
/// XML MIME types keep the values [`navigated_content_type`] already
/// interned. Other essences are interned so `Document.contentType` can
/// report them without changing `Parsed::content_type` off `&'static str`.
///
/// <https://html.spec.whatwg.org/multipage/nav-history-apis.html#concept-document-content-type>
/// <https://html.spec.whatwg.org/multipage/dom.html#dom-document-contenttype>
pub(crate) fn document_content_type(header: &str) -> &'static str {
    if let Some(xml) = navigated_content_type(header) {
        return xml;
    }
    let essence = header
        .split(';')
        .next()
        .unwrap_or(header)
        .trim()
        .to_ascii_lowercase();
    if essence.is_empty() {
        return "application/octet-stream";
    }
    match essence.as_str() {
        "text/html" => "text/html",
        "text/css" => "text/css",
        "text/plain" => "text/plain",
        "image/jpeg" => "image/jpeg",
        "image/png" => "image/png",
        "image/gif" => "image/gif",
        "image/bmp" => "image/bmp",
        "application/octet-stream" => "application/octet-stream",
        _ => intern_mime_essence(essence),
    }
}

fn intern_mime_essence(essence: String) -> &'static str {
    let mut guard = MIME_ESSENCES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let interned = guard.get_or_insert_with(HashSet::new);
    if let Some(existing) = interned.get(essence.as_str()) {
        return existing;
    }
    let leaked: &'static str = Box::leak(essence.into_boxed_str());
    interned.insert(leaked);
    leaked
}

/// Replaces the empty comments the XML sink left for processing
/// instructions. The XML declaration (`<?xml ...?>`) is not a node, so its
/// placeholder is removed rather than kept as a comment.
fn attach_xml_processing_instructions(document: &mut crate::documents::BlitzDocument, input: &str) {
    let instructions = scan_xml_processing_instructions(input);
    if instructions.is_empty() {
        return;
    }
    let mut holes = Vec::new();
    collect_pi_holes(document, document.base.root_node().id, &mut holes);
    let mut holes = holes.into_iter();
    for (index, (target, data)) in instructions.into_iter().enumerate() {
        let Some(hole) = holes.next() else {
            break;
        };
        if index == 0 && target == "xml" {
            document.base.mutate().remove_node(hole);
            continue;
        }
        let fresh = document.create_processing_instruction(target, &data);
        document.base.mutate().replace_node_with(hole, &[fresh]);
    }
}

/// Puts xmlns attributes xml5ever dropped back onto the matching elements.
fn attach_xml_namespace_declarations(document: &mut crate::documents::BlitzDocument, input: &str) {
    let tags = scan_xml_start_xmlns(input);
    if tags.is_empty() {
        return;
    }
    let mut elements = Vec::new();
    collect_xml_elements(document, document.base.root_node().id, &mut elements);
    if tags.len() != elements.len() {
        return;
    }
    for (id, attributes) in elements.into_iter().zip(tags) {
        for (name, value) in attributes {
            document.base.mutate().set_attribute(id, name, &value);
        }
    }
}

fn collect_xml_elements(
    document: &crate::documents::BlitzDocument,
    id: blitz_traits::node_id::NodeId,
    elements: &mut Vec<blitz_traits::node_id::NodeId>,
) {
    let is_element = document.extra(id).is_none()
        && document
            .base
            .get_node(id)
            .and_then(|node| node.data.downcast_element())
            .is_some();
    if is_element {
        elements.push(id);
    }
    let children: Vec<_> = document
        .base
        .get_node(id)
        .map(|node| node.children.iter().copied().collect())
        .unwrap_or_default();
    for child in children {
        collect_xml_elements(document, child, elements);
    }
}

fn scan_xml_start_xmlns(input: &str) -> Vec<Vec<(markup5ever::QualName, String)>> {
    let mut found = Vec::new();
    let mut pos = 0;
    while pos < input.len() {
        let rest = &input[pos..];
        if let Some(inside) = rest.strip_prefix("<!--") {
            match inside.find("-->") {
                Some(end) => pos += 4 + end + 3,
                None => break,
            }
            continue;
        }
        if let Some(inside) = rest.strip_prefix("<![CDATA[") {
            match inside.find("]]>") {
                Some(end) => pos += 9 + end + 3,
                None => break,
            }
            continue;
        }
        if rest.starts_with("<?") || rest.starts_with("</") || rest.starts_with("<!") {
            pos += skip_markup(rest);
            continue;
        }
        if rest.starts_with('<') {
            let (consumed, xmlns) = parse_xml_start_xmlns(rest);
            found.push(xmlns);
            pos += consumed;
            continue;
        }
        let Some(next) = rest.chars().next() else {
            break;
        };
        pos += next.len_utf8();
    }
    found
}

fn parse_xml_start_xmlns(tag: &str) -> (usize, Vec<(markup5ever::QualName, String)>) {
    let consumed = skip_markup(tag);
    let body = tag.get(1..consumed).unwrap_or("");
    let mut xmlns = Vec::new();
    let mut chars = body.chars().peekable();
    while chars
        .peek()
        .is_some_and(|c| !is_xml_whitespace(*c) && *c != '/' && *c != '>')
    {
        chars.next();
    }
    loop {
        while chars.peek().is_some_and(|c| is_xml_whitespace(*c)) {
            chars.next();
        }
        match chars.peek() {
            None | Some('/' | '>') => break,
            _ => {}
        }
        let mut name = String::new();
        while let Some(c) = chars.peek().copied() {
            if is_xml_whitespace(c) || c == '=' || c == '/' || c == '>' {
                break;
            }
            name.push(c);
            chars.next();
        }
        while chars.peek().is_some_and(|c| is_xml_whitespace(*c)) {
            chars.next();
        }
        if chars.peek() != Some(&'=') {
            continue;
        }
        chars.next();
        while chars.peek().is_some_and(|c| is_xml_whitespace(*c)) {
            chars.next();
        }
        let Some(quote) = chars.next() else {
            break;
        };
        if quote != '"' && quote != '\'' {
            continue;
        }
        let mut value = String::new();
        for c in chars.by_ref() {
            if c == quote {
                break;
            }
            value.push(c);
        }
        if let Some(name) = xmlns_attribute_name(&name) {
            xmlns.push((name, value));
        }
    }
    (consumed, xmlns)
}

fn xmlns_attribute_name(name: &str) -> Option<markup5ever::QualName> {
    const XMLNS: &str = "http://www.w3.org/2000/xmlns/";
    if name == "xmlns" {
        return Some(markup5ever::QualName::new(
            None,
            markup5ever::Namespace::from(XMLNS),
            markup5ever::LocalName::from("xmlns"),
        ));
    }
    let prefix = name.strip_prefix("xmlns:")?;
    if prefix.is_empty() {
        return None;
    }
    Some(markup5ever::QualName::new(
        Some(markup5ever::Prefix::from("xmlns")),
        markup5ever::Namespace::from(XMLNS),
        markup5ever::LocalName::from(prefix),
    ))
}

fn collect_pi_holes(
    document: &crate::documents::BlitzDocument,
    id: blitz_traits::node_id::NodeId,
    holes: &mut Vec<blitz_traits::node_id::NodeId>,
) {
    let is_hole = document.extra(id).is_none()
        && document.base.get_node(id).is_some_and(|node| {
            matches!(
                &node.data,
                blitz_dom::NodeData::Comment { contents } if contents.is_empty()
            )
        });
    if is_hole {
        holes.push(id);
    }
    let children: Vec<_> = document
        .base
        .get_node(id)
        .map(|node| node.children.iter().copied().collect())
        .unwrap_or_default();
    for child in children {
        collect_pi_holes(document, child, holes);
    }
}

fn scan_xml_processing_instructions(input: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut pos = 0;
    while pos < input.len() {
        let rest = &input[pos..];
        if let Some(inside) = rest.strip_prefix("<!--") {
            match inside.find("-->") {
                Some(end) => pos += 4 + end + 3,
                None => break,
            }
            continue;
        }
        if let Some(inside) = rest.strip_prefix("<![CDATA[") {
            match inside.find("]]>") {
                Some(end) => pos += 9 + end + 3,
                None => break,
            }
            continue;
        }
        if let Some(inside) = rest.strip_prefix("<?") {
            match inside.find("?>") {
                Some(end) => {
                    push_processing_instruction(&mut found, &inside[..end]);
                    pos += 2 + end + 2;
                }
                None => break,
            }
            continue;
        }
        if rest.starts_with('<') {
            pos += skip_markup(rest);
            continue;
        }
        let Some(next) = rest.chars().next() else {
            break;
        };
        pos += next.len_utf8();
    }
    found
}

fn push_processing_instruction(found: &mut Vec<(String, String)>, body: &str) {
    let mut target = String::new();
    let mut chars = body.chars();
    for character in chars.by_ref() {
        if is_xml_whitespace(character) {
            break;
        }
        target.push(character);
    }
    if target.is_empty() {
        return;
    }
    let data: String = chars.collect();
    let data = data.trim_start_matches(is_xml_whitespace).to_owned();
    found.push((target, data));
}

fn skip_markup(rest: &str) -> usize {
    let mut quote = None;
    for (index, character) in rest.char_indices().skip(1) {
        if let Some(open) = quote {
            if character == open {
                quote = None;
            }
            continue;
        }
        match character {
            '"' | '\'' => quote = Some(character),
            '>' => return index + character.len_utf8(),
            _ => {}
        }
    }
    rest.len()
}

fn is_xml_whitespace(character: char) -> bool {
    matches!(character, ' ' | '\t' | '\r' | '\n')
}

/// Whether `name` matches the XML `Name` production
/// (<https://www.w3.org/TR/xml/#NT-Name>).
pub(crate) fn is_valid_name(name: &str) -> bool {
    let mut characters = name.chars();
    match characters.next() {
        Some(first) if is_name_start(first) => characters.all(is_name_char),
        _ => false,
    }
}

// https://www.w3.org/TR/xml/#NT-NameStartChar
pub(crate) fn is_name_start(character: char) -> bool {
    matches!(character, ':' | 'A'..='Z' | '_' | 'a'..='z' | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}' | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}' | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}' | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}')
        || ('\u{10000}'..='\u{EFFFF}').contains(&character)
}

// https://www.w3.org/TR/xml/#NT-NameChar
pub(crate) fn is_name_char(character: char) -> bool {
    is_name_start(character)
        || matches!(character, '-' | '.' | '0'..='9' | '\u{B7}' | '\u{0300}'..='\u{036F}' | '\u{203F}'..='\u{2040}')
}
