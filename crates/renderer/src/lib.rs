//! Page engine for one document: Blitz parsing and trees plus `QuickJS`
//! behind a value-only browser interface.
//!
//! The engine owns the frame actor (`document::Document`) and never links `net`.
//! The browser process owns the tab, navigation, the network, and cookies.
//! Style, layout, and paint live in the `render` module via Blitz: one-shot
//! screenshot and geometry through `blitz-dom`/`blitz-paint` and the
//! `anyrender_tiny_skia` backend. A carrier drives this crate: the browser's
//! child transport on a native build, the WebAssembly component on a wasm
//! build. Both feed the same [`Engine`], and both implement [`BrowserServices`]
//! to supply effects.

use markup5ever::interface::QuirksMode;

use crate::documents::BlitzDocument;

mod document;
mod documents;
pub mod dom_string;
mod embedded;
mod engine;
mod js;
mod messaging;
pub(crate) mod names;
mod protocol;
mod pseudo_attributes;
mod remote;
mod render;
mod storage;
mod xml;

pub use document::Stop;
pub use embedded::EmbeddedRenderer;
pub use engine::{DEFAULT_VIEWPORT, Engine};
pub use protocol::{
    BrowserServices, BrowsingContextHost, DialCancellation, DialCompletion, DialFailure, DialKind,
    DialOutcome, DialRequest, FrameId, HistorySnapshot, MAX_RESPONSE_BODY_BYTES, MessagingHost,
    Mount, NetworkHost, RendererEvent, ResourceLimit, ResponseHead, STORAGE_QUOTA_BYTES,
    ScreenshotClip, ScreenshotRequest, ScriptFailure, ScriptSource, StorageChange, StorageError,
    StorageHost, StorageKind, TabError,
};
pub use remote::RemoteValue;
pub use storage::PendingStorageEvent;

/// The current document readiness
/// (<https://html.spec.whatwg.org/multipage/dom.html#current-document-readiness>).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReadyState {
    /// The parser is still running.
    Loading,
    /// Parsing finished; `DOMContentLoaded` has fired.
    Interactive,
    /// The document is completely loaded; the `load` event has fired.
    Complete,
}

/// The result of parsing one document.
pub(crate) struct Parsed {
    /// The store id, assigned on insert; `0` means unassigned.
    pub id: u32,
    /// The parsed tree plus its mutation journal.
    pub document: BlitzDocument,
    /// Compatibility mode selected by the doctype (or its absence).
    pub quirks_mode: QuirksMode,
    /// MIME type this document reports from `document.contentType`.
    pub content_type: &'static str,
    /// Whether this object implements `XMLDocument`
    /// (<https://dom.spec.whatwg.org/#xmldocument>).
    ///
    /// `new Document()` creates an XML document that is still a `Document`.
    /// Parsed XML, `new XMLDocument()`, and `createDocument()` use
    /// `XMLDocument`.
    pub xml_document: bool,
    /// The document's readiness; parsed documents start at [`ReadyState::Loading`]
    /// while documents created by script start complete
    /// (<https://html.spec.whatwg.org/multipage/dom.html#current-document-readiness>).
    pub ready_state: ReadyState,
    /// The document's URL when it differs from the world's active document,
    /// such as a `DOMParser` result
    /// (<https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-domparser-parsefromstring>).
    pub url: Option<String>,
}

impl Parsed {
    /// An empty script-created document: never rendered, so no UA sheets and
    /// no providers; shares the process font context for speed (a fresh
    /// scan costs ~25ms per document).
    pub(crate) fn script(content_type: &'static str, font_ctx: parley::FontContext) -> Self {
        let config = blitz_dom::DocumentConfig {
            font_ctx: Some(font_ctx),
            ua_stylesheets: Some(Vec::new()),
            base_url: Some(crate::render::INVALID_BASE_URL.to_owned()),
            ..blitz_dom::DocumentConfig::default()
        };
        Self {
            id: 0,
            document: BlitzDocument::new(config),
            quirks_mode: QuirksMode::NoQuirks,
            content_type,
            xml_document: content_type != "text/html",
            ready_state: ReadyState::Complete,
            url: None,
        }
    }
}

/// Parses a full HTML document into a fresh tree.
///
/// Broken markup recovery is html5ever's job (through `blitz-html`).
/// Script interleaving runs in tree order in `document`.
/// Blitz parses with scripting disabled, so `noscript` parses as markup;
/// `document.write` paths handle scripting-enabled insertion separately.
#[must_use]
pub(crate) fn parse_html(input: &str, config: blitz_dom::DocumentConfig) -> Parsed {
    let quirks_mode = sniff_quirks_mode(input);
    let base: blitz_dom::BaseDocument = blitz_html::HtmlDocument::from_html(input, config).into();
    let mut document = BlitzDocument::from_base(base);
    // Blitz's HTML sink ignores the doctype token (`drop_doctype` and an
    // empty `append_doctype_to_document`). The initial insertion mode still
    // appends that one doctype
    // (<https://html.spec.whatwg.org/multipage/parsing.html#the-initial-insertion-mode>).
    attach_leading_doctype(&mut document, input, false);
    Parsed {
        id: 0,
        document,
        quirks_mode,
        content_type: "text/html",
        xml_document: false,
        ready_state: ReadyState::Loading,
        url: None,
    }
}

/// Compatibility mode from the doctype: missing means quirks, an
/// exact `html` doctype means standards, anything else means limited quirks.
fn sniff_quirks_mode(input: &str) -> QuirksMode {
    let rest = input.trim_start_matches(['\u{feff}', ' ', '\t', '\n', '\x0c', '\r']);
    let Some(prefix) = rest.get(..9) else {
        return QuirksMode::Quirks;
    };
    if !prefix.eq_ignore_ascii_case("<!doctype") {
        return QuirksMode::Quirks;
    }
    let end = rest.find('>').map_or(rest.len(), |index| index + 1);
    let normalized = rest[..end]
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if normalized == "<!doctype html>" {
        QuirksMode::NoQuirks
    } else {
        QuirksMode::LimitedQuirks
    }
}

/// A doctype the parser dropped, plus how many document children precede it.
struct LeadingDoctype {
    name: String,
    public_id: String,
    system_id: String,
    nodes_before: usize,
}

/// Inserts the preamble doctype Blitz's sink discarded.
pub(crate) fn attach_leading_doctype(document: &mut BlitzDocument, input: &str, xml: bool) {
    let Some(found) = scan_leading_doctype(input, xml) else {
        return;
    };
    document.insert_doctype_child(
        found.name,
        found.public_id,
        found.system_id,
        found.nodes_before,
    );
}

/// The doctype from the initial insertion mode, when the source has one
/// before the first element.
///
/// HTML matches `<!DOCTYPE` case-insensitively and lowercases the name.
/// XML matches the production case-sensitively and also counts processing
/// instructions, which the XML sink inserts as empty comments
/// (<https://www.w3.org/TR/xml/#NT-doctypedecl>).
fn scan_leading_doctype(input: &str, xml: bool) -> Option<LeadingDoctype> {
    let mut scan = PreambleScan { input, pos: 0 };
    scan.skip_bom();
    let mut nodes_before = 0;
    loop {
        scan.skip_whitespace();
        if scan.consume_comment() {
            nodes_before += 1;
            continue;
        }
        if xml && scan.consume_processing_instruction() {
            nodes_before += 1;
            continue;
        }
        return scan.read_doctype(xml, nodes_before);
    }
}

struct PreambleScan<'a> {
    input: &'a str,
    pos: usize,
}

impl PreambleScan<'_> {
    fn rest(&self) -> &str {
        &self.input[self.pos..]
    }

    fn skip_bom(&mut self) {
        if self.rest().starts_with('\u{feff}') {
            self.pos += '\u{feff}'.len_utf8();
        }
    }

    fn skip_whitespace(&mut self) {
        while let Some(c) = self.rest().chars().next() {
            if !is_preamble_whitespace(c) {
                break;
            }
            self.pos += c.len_utf8();
        }
    }

    fn consume_comment(&mut self) -> bool {
        if !self.rest().starts_with("<!--") {
            return false;
        }
        let Some(end) = self.rest()[4..].find("-->") else {
            return false;
        };
        self.pos += 4 + end + 3;
        true
    }

    fn consume_processing_instruction(&mut self) -> bool {
        if !self.rest().starts_with("<?") {
            return false;
        }
        let Some(end) = self.rest()[2..].find("?>") else {
            return false;
        };
        self.pos += 2 + end + 2;
        true
    }

    fn starts_with_keyword(&self, keyword: &str, ignore_ascii_case: bool) -> bool {
        let rest = self.rest();
        if rest.len() < keyword.len() {
            return false;
        }
        if ignore_ascii_case {
            rest.as_bytes()[..keyword.len()].eq_ignore_ascii_case(keyword.as_bytes())
        } else {
            &rest[..keyword.len()] == keyword
        }
    }

    fn read_doctype(&mut self, xml: bool, nodes_before: usize) -> Option<LeadingDoctype> {
        if !self.starts_with_keyword("<!DOCTYPE", !xml) {
            return None;
        }
        let after = self.rest()[9..].chars().next()?;
        if !is_preamble_whitespace(after) {
            return None;
        }
        self.pos += 9;
        self.skip_whitespace();
        let name = self.read_name(!xml);
        self.skip_whitespace();
        let (public_id, system_id) = self.read_ids(xml);
        Some(LeadingDoctype {
            name,
            public_id,
            system_id,
            nodes_before,
        })
    }

    fn read_name(&mut self, lowercase: bool) -> String {
        let mut name = String::new();
        while let Some(c) = self.rest().chars().next() {
            if c == '>' || is_preamble_whitespace(c) {
                break;
            }
            self.pos += c.len_utf8();
            if lowercase && c.is_ascii_uppercase() {
                name.push(c.to_ascii_lowercase());
            } else {
                name.push(c);
            }
        }
        name
    }

    fn read_ids(&mut self, xml: bool) -> (String, String) {
        if self.consume_keyword("PUBLIC", !xml) {
            self.skip_whitespace();
            let public_id = self.read_quoted().unwrap_or_default();
            self.skip_whitespace();
            let system_id = self.read_quoted().unwrap_or_default();
            (public_id, system_id)
        } else if self.consume_keyword("SYSTEM", !xml) {
            self.skip_whitespace();
            (String::new(), self.read_quoted().unwrap_or_default())
        } else {
            (String::new(), String::new())
        }
    }

    fn consume_keyword(&mut self, keyword: &str, ignore_ascii_case: bool) -> bool {
        if !self.starts_with_keyword(keyword, ignore_ascii_case) {
            return false;
        }
        let boundary = self.rest()[keyword.len()..].chars().next();
        if boundary.is_some_and(|c| !is_preamble_whitespace(c) && c != '>') {
            return false;
        }
        self.pos += keyword.len();
        true
    }

    fn read_quoted(&mut self) -> Option<String> {
        let quote = self.rest().chars().next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        self.pos += quote.len_utf8();
        let mut value = String::new();
        while let Some(c) = self.rest().chars().next() {
            self.pos += c.len_utf8();
            if c == quote {
                return Some(value);
            }
            value.push(c);
        }
        Some(value)
    }
}

fn is_preamble_whitespace(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\u{000c}' | '\r' | ' ')
}
