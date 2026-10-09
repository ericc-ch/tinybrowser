//! XML parsing for `DOMParser`'s XML MIME types.
//!
//! Delegates to `blitz-html`'s XML path, which keeps real doctype,
//! processing-instruction, CDATA, and `xmlns` nodes and expands internal
//! general entities up front. Well-formedness violations drain into a
//! `parsererror` document below. Entity expansion covers internal general
//! entities only: undefined or external references fail closed to
//! `parsererror`; attribute-value references expand; duplicate declarations
//! keep the first; `NDATA` declarations never expand.

use std::collections::HashSet;
use std::sync::Mutex;

use crate::Parsed;

static MIME_ESSENCES: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);

/// The namespace of the `parsererror` document element, shared by the
/// document builder here and the XHR `responseXML` failure check (which must
/// compare namespace, not just the local name: a well-formed `<parsererror/>`
/// response is a valid document, not a failure).
pub(crate) const PARSERERROR_NS: &str = "http://www.mozilla.org/newlayout/xml/parsererror.xml";

/// Parses `input` as an XML document with the given content type.
///
/// Used only by `DOMParser`, whose results are plain `Document`s, never
/// `XMLDocument`s
/// (<https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-domparser-parsefromstring>).
/// Navigated XML goes through `parse_navigated` and keeps the XML kind.
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
        false,
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
    parse_with_config(input, content_type, crate::ReadyState::Loading, true, config)
}

/// Parses `input` with the shared XML configuration: the sink keeps real
/// doctype, processing-instruction, CDATA, and `xmlns` nodes, with internal
/// entities expanded up front. A non-well-formed input yields a `parsererror`
/// document instead of the partial tree
/// (<https://html.spec.whatwg.org/multipage/xhtml.html#parsing-xhtml-documents>).
fn parse_with_config(
    input: &str,
    content_type: &'static str,
    ready_state: crate::ReadyState,
    xml_document: bool,
    config: blitz_dom::DocumentConfig,
) -> Parsed {
    // The sink keeps real doctype, processing-instruction, CDATA, and xmlns
    // nodes, with internal entities expanded up front.
    let base: blitz_dom::BaseDocument = blitz_html::HtmlDocument::from_xml(input, config).into();
    let mut document = crate::documents::BlitzDocument::from_base(base);
    // Parsed processing instructions arrive with data but no attribute map;
    // the bindings read the map, so initialize it from the data here.
    document.init_parsed_pi_attributes();
    let errors = document.base.take_parse_errors();
    if !errors.is_empty() {
        return parser_error_document(
            document,
            content_type,
            ready_state,
            xml_document,
            &errors,
        );
    }
    Parsed {
        id: 0,
        document,
        quirks_mode: markup5ever::interface::QuirksMode::NoQuirks,
        content_type,
        xml_document,
        ready_state,
        url: None,
        // `&str` input is already decoded: DOMParser keeps UTF-8 (correct).
        // Navigated responses overwrite this from the sniffed decoder in
        // `document/mod.rs`, so a future "fix" unifying the two breaks
        // `document.characterSet`.
        character_set: "UTF-8",
    }
}

/// Rebuilds the parsed document as a `parsererror` document for a
/// non-well-formed XML input: the same content type, base URL, and providers,
/// whose document element is a single `parsererror` element in
/// [`PARSERERROR_NS`] describing the failure
/// (<https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-domparser-parsefromstring>).
///
/// Reuses the parsed document instead of building a fresh one: a fresh build
/// would lose the base URL and providers the caller configured, so a failed
/// navigated response would resolve URLs differently than the same response
/// parsed cleanly.
fn parser_error_document(
    mut document: crate::documents::BlitzDocument,
    content_type: &'static str,
    ready_state: crate::ReadyState,
    xml_document: bool,
    errors: &[String],
) -> Parsed {
    let root = document.base.root_node().id;
    document.base.mutate().remove_and_drop_all_children(root);
    let name = markup5ever::QualName::new(
        None,
        markup5ever::Namespace::from(PARSERERROR_NS),
        markup5ever::LocalName::from("parsererror"),
    );
    let element = document.base.mutate().create_element(name, Vec::new());
    document.base.mutate().append_children(root, &[element]);
    let message = errors.join("\n");
    if !message.is_empty() {
        let text = document.base.mutate().create_text_node(&message);
        document.base.mutate().append_children(element, &[text]);
    }
    Parsed {
        id: 0,
        document,
        quirks_mode: markup5ever::interface::QuirksMode::NoQuirks,
        content_type,
        xml_document,
        ready_state,
        url: None,
        character_set: "UTF-8",
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

/// Interns an arbitrary MIME essence for `Parsed::content_type`, which is
/// `&'static str`.
fn intern_mime_essence(essence: String) -> &'static str {
    // Bounded interning: essences arrive from response `Content-Type`
    // headers, so an unbounded table is a memory leak on attacker input.
    // Past the cap, unknown essences report as the generic binary type;
    // every essence the specs name is a fixed static above and unaffected.
    const MAX_INTERNED_ESSENCES: usize = 1024;
    let mut guard = MIME_ESSENCES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let interned = guard.get_or_insert_with(HashSet::new);
    if let Some(existing) = interned.get(essence.as_str()) {
        return existing;
    }
    if interned.len() >= MAX_INTERNED_ESSENCES {
        return "application/octet-stream";
    }
    let leaked: &'static str = Box::leak(essence.into_boxed_str());
    interned.insert(leaked);
    leaked
}

/// Whether `name` matches the XML `Name` production: pure validation for
/// processing-instruction targets, not tree reconstruction
/// (<https://www.w3.org/TR/xml/#NT-Name>).
pub(crate) fn is_valid_name(name: &str) -> bool {
    let mut characters = name.chars();
    match characters.next() {
        Some(first) if is_name_start(first) => characters.all(is_name_char),
        _ => false,
    }
}

/// Whether `character` may start an XML `Name`
/// (<https://www.w3.org/TR/xml/#NT-NameStartChar>).
pub(crate) fn is_name_start(character: char) -> bool {
    matches!(character, ':' | 'A'..='Z' | '_' | 'a'..='z' | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}' | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}' | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}' | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}')
        || ('\u{10000}'..='\u{EFFFF}').contains(&character)
}

/// Whether `character` may continue an XML `Name`
/// (<https://www.w3.org/TR/xml/#NT-NameChar>).
pub(crate) fn is_name_char(character: char) -> bool {
    is_name_start(character)
        || matches!(character, '-' | '.' | '0'..='9' | '\u{B7}' | '\u{0300}'..='\u{036F}' | '\u{203F}'..='\u{2040}')
}

