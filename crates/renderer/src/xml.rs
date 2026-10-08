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
    let document = crate::documents::BlitzDocument::from_base(base);
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
