//! XML parsing for `DOMParser`'s XML MIME types.
//!
//! Delegates to `blitz-html`'s XML path. Malformed input yields a document
//! holding a `parsererror` element, matching the HTML XML parsing rules.

use crate::Parsed;

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
    // is not one of those nodes.
    attach_xml_processing_instructions(&mut document, input);
    Parsed {
        id: 0,
        document,
        quirks_mode: markup5ever::interface::QuirksMode::NoQuirks,
        content_type,
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

/// Replaces the empty comments the XML sink left for processing
/// instructions. The XML declaration (`<?xml ...?>`) keeps its placeholder.
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
            continue;
        }
        let fresh = document.create_processing_instruction(target, &data);
        document.base.mutate().replace_node_with(hole, &[fresh]);
    }
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
