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
    /// The encoding name `document.characterSet` reports
    /// (<https://encoding.spec.whatwg.org/#dom-document-characterset>).
    pub character_set: &'static str,
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
            character_set: "UTF-8",
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
    let document = BlitzDocument::from_base(base);
    // The sink appends the doctype as a real document child now; the PI
    // attribute maps initialize lazily through the bindings.
    Parsed {
        id: 0,
        document,
        quirks_mode,
        content_type: "text/html",
        xml_document: false,
        ready_state: ReadyState::Loading,
        url: None,
        character_set: "UTF-8",
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

#[cfg(test)]
mod tests {
    use super::{QuirksMode, sniff_quirks_mode};

    // The quirks sniff slices near attacker-controlled bytes; none of these
    // inputs may panic, whatever mode they return.
    #[test]
    fn quirks_sniff_never_panics_on_partial_or_multibyte_input() {
        let inputs = [
            "",
            "<",
            "<!DOC",
            "<!DOCTYPE",
            "<!DOCTYPE ",
            "<!DOCTYPE a",
            "<!DOCTYPE a SYSTE",
            "<!DOCTYPE a SYSTE\u{e9}>",
            "<!DOCTYPE a PUBLI\u{e9}\">",
            "<ab>\u{65e5}\u{672c}\u{8a9e}</ab>",
            "<!DOCTYPE \u{65e5}\u{672c}\u{8a9e}>",
            "\u{feff}<!DOCTYPE html>",
            "<!-- unterminated",
            "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Transitional//EN\"",
            "<?\u{65e5}?>",
            "<!\u{e9}DOCTYPE html>",
        ];
        for input in inputs {
            let _ = sniff_quirks_mode(input);
            // Quirks is the only mode the exact-`html` production escapes.
            if input == "\u{feff}<!DOCTYPE html>" {
                assert_eq!(sniff_quirks_mode(input), QuirksMode::NoQuirks);
            }
        }
    }
}
