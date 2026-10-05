//! XML parsing for `DOMParser`'s XML MIME types.
//!
//! Delegates to `blitz-html`'s XML path. Malformed input yields a document
//! holding a `parsererror` element, matching the HTML XML parsing rules.

use crate::Parsed;

/// Parses `input` as an XML document with the given content type.
pub(crate) fn parse_document(input: &str, content_type: &'static str) -> Parsed {
    let base: blitz_dom::BaseDocument =
        blitz_html::HtmlDocument::from_xml(input, blitz_dom::DocumentConfig::default()).into();
    // Known gap (upstream): Blitz drops the doctype while parsing, so
    // `document.doctype` reads null. Tracked in docs/progress.md.
    let document = crate::documents::BlitzDocument::from_base(base);
    Parsed {
        id: 0,
        document,
        quirks_mode: html5ever::tree_builder::QuirksMode::NoQuirks,
        content_type,
        ready_state: crate::ReadyState::Complete,
        url: None,
    }
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
fn is_name_start(character: char) -> bool {
    matches!(character, ':' | 'A'..='Z' | '_' | 'a'..='z' | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}' | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}' | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}' | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}')
        || ('\u{10000}'..='\u{EFFFF}').contains(&character)
}

// https://www.w3.org/TR/xml/#NT-NameChar
fn is_name_char(character: char) -> bool {
    is_name_start(character)
        || matches!(character, '-' | '.' | '0'..='9' | '\u{B7}' | '\u{0300}'..='\u{036F}' | '\u{203F}'..='\u{2040}')
}
