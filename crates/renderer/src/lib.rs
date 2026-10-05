//! Page engine for one document: Blitz parsing and trees plus `QuickJS`
//! behind a value-only browser interface.
//!
//! The engine owns the frame actor (`document::Document`) and never links `net`.
//! The browser process owns the tab, navigation, the network, and cookies.
//! Style, layout, and paint live in the `render` module: one-shot screenshot
//! and geometry (Blink `core/css`,
//! `core/layout`, `core/paint`). A carrier drives this crate: the browser's
//! child transport on a native build, the WebAssembly component on a wasm
//! build. Both feed the same [`Engine`], and both implement [`BrowserServices`]
//! to supply effects.

use html5ever::tree_builder::QuirksMode;

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
mod remote;
mod render;
mod storage;
mod xml;

pub use document::Stop;
pub use embedded::EmbeddedRenderer;
pub use engine::Engine;
pub use protocol::{
    BrowserServices, BrowsingContextHost, DialCancellation, DialCompletion, DialFailure, DialKind,
    DialOutcome, DialRequest, FrameId, HistorySnapshot, MAX_RESPONSE_BODY_BYTES, MessagingHost,
    Mount, NetworkHost, RendererEvent, ResourceLimit, STORAGE_QUOTA_BYTES, ScreenshotClip,
    ScreenshotRequest, ScriptFailure, ScriptSource, StorageChange, StorageError, StorageHost,
    StorageKind, TabError,
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
            base_url: Some("http://invalid/".to_owned()),
            ..blitz_dom::DocumentConfig::default()
        };
        Self {
            id: 0,
            document: BlitzDocument::new(config),
            quirks_mode: QuirksMode::NoQuirks,
            content_type,
            ready_state: ReadyState::Complete,
            url: None,
        }
    }
}


/// Parses a full HTML document into a fresh tree with the scripting flag
/// enabled (the browser default).
///
/// Broken markup is recovered exactly the way the HTML spec, and therefore
/// every browser, mandates; that recovery is html5ever's job (through
/// `blitz-html`), not ours. Whole-document parse: script interleaving moved
/// to tree-order execution in `document`, since the tokenizer no longer
/// yields script boundaries.
#[must_use]
pub(crate) fn parse_html(input: &str, config: blitz_dom::DocumentConfig) -> Parsed {
    let quirks_mode = sniff_quirks_mode(input);
    let base: blitz_dom::BaseDocument = blitz_html::HtmlDocument::from_html(input, config).into();
    let mut document = BlitzDocument::from_base(base);
    // Blitz drops the doctype while parsing; recapture it from the source so
    // `document.doctype` and doctype-sensitive tests observe it.
    if let Some((name, public_id, system_id)) = capture_doctype(input) {
        insert_doctype(&mut document, &name, &public_id, &system_id);
    }
    Parsed {
        id: 0,
        document,
        quirks_mode,
        content_type: "text/html",
        ready_state: ReadyState::Loading,
        url: None,
    }
}

/// Inserts a synthetic doctype as the document's first child, ahead of the
/// document element.
pub(crate) fn insert_doctype(document: &mut BlitzDocument, name: &str, public_id: &str, system_id: &str) {
    let backing = document.create_doctype(name, public_id, system_id);
    let root = document.base.root_node().id;
    let first_element = document
        .base
        .get_node(root)
        .and_then(|root| root.children.first().copied());
    match first_element {
        Some(anchor) => {
            document.base.mutate().insert_nodes_before(anchor, &[backing]);
        }
        None => {
            document.base.mutate().append_children(root, &[backing]);
        }
    }
}

/// The `<!DOCTYPE ...>` declaration's name and identifiers, when the input
/// opens with one. Quoted literals decode the five predefined XML entities;
/// anything malformed reports no doctype rather than a partial one.
pub(crate) fn capture_doctype(input: &str) -> Option<(String, String, String)> {
    let rest = input.trim_start_matches(['\u{feff}', ' ', '\t', '\n', '\x0c', '\r']);
    let after_open = rest
        .get(..9)
        .filter(|prefix| prefix.eq_ignore_ascii_case("<!doctype"))?;
    let _ = after_open;
    let mut cursor = rest[9..].trim_start_matches([' ', '\t', '\n', '\x0c', '\r']);
    let end = cursor
        .find(|c: char| c == '>' || c.is_ascii_whitespace())
        .unwrap_or(cursor.len());
    let name = cursor[..end].to_owned();
    if name.is_empty() {
        return None;
    }
    cursor = cursor[end..].trim_start_matches([' ', '\t', '\n', '\x0c', '\r']);
    let (public_id, system_id) = if cursor.len() >= 6 && cursor[..6].eq_ignore_ascii_case("public") {
        let rest = cursor[6..].trim_start_matches([' ', '\t', '\n', '\x0c', '\r']);
        let (public_id, rest) = take_quoted(rest)?;
        let rest = rest.trim_start_matches([' ', '\t', '\n', '\x0c', '\r']);
        let (system_id, _) = take_quoted(rest).unwrap_or_default();
        (public_id, system_id)
    } else if cursor.len() >= 6 && cursor[..6].eq_ignore_ascii_case("system") {
        let rest = cursor[6..].trim_start_matches([' ', '\t', '\n', '\x0c', '\r']);
        let (system_id, _) = take_quoted(rest).unwrap_or_default();
        (String::new(), system_id)
    } else {
        (String::new(), String::new())
    };
    Some((name, public_id, system_id))
}

/// Consumes one quoted literal, decoding character references.
fn take_quoted(input: &str) -> Option<(String, &str)> {
    let quote = input.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let rest = &input[1..];
    let end = rest.find(quote)?;
    let raw = &rest[..end];
    let mut value = String::with_capacity(raw.len());
    decode_references(raw, &mut value)?;
    Some((value, &rest[end + 1..]))
}

/// Decodes the five predefined XML entities and numeric character references.
fn decode_references(raw: &str, out: &mut String) -> Option<()> {
    let mut rest = raw;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp + 1..];
        let semi = rest.find(';')?;
        match &rest[..semi] {
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "amp" => out.push('&'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            entity if entity.starts_with('#') => {
                let code = if let Some(hex) = entity.strip_prefix("#x").or_else(|| entity.strip_prefix("#X")) {
                    u32::from_str_radix(hex, 16).ok()?
                } else {
                    entity[1..].parse().ok()?
                };
                out.push(char::from_u32(code)?);
            }
            _ => return None,
        }
        rest = &rest[semi + 1..];
    }
    out.push_str(rest);
    Some(())
}

/// Parses a full HTML document with scripting disabled, the mode `DOMParser`
/// uses
/// (<https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-domparser-parsefromstring>).
///
/// The Blitz parser always runs with scripting disabled (`noscript` parses as
/// markup); the scripting-enabled `noscript`-as-text divergence is a known
/// gap tracked with the cutover.
#[must_use]
pub(crate) fn parse_html_without_scripting(input: &str, config: blitz_dom::DocumentConfig) -> Parsed {
    parse_html(input, config)
}

/// Compatibility mode from the doctype, simplified: missing means quirks, an
/// exact `html` doctype means standards, anything else means limited quirks.
/// The full public-identifier table
/// (<https://html.spec.whatwg.org/multipage/parsing.html#the-initial-insertion-mode>)
/// is a known gap; Blitz computes its own mode for style internally, so this
/// only feeds `document.compatMode`.
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
