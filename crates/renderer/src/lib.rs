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
    /// An empty document of `content_type` that is already fully loaded,
    /// like the ones script constructors create.
    pub(crate) fn empty(content_type: &'static str) -> Self {
        Self {
            id: 0,
            document: BlitzDocument::new(blitz_dom::DocumentConfig::default()),
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
    Parsed {
        id: 0,
        document: BlitzDocument::from_base(base),
        quirks_mode,
        content_type: "text/html",
        ready_state: ReadyState::Loading,
        url: None,
    }
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
