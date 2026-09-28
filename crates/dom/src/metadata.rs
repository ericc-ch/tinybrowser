//! Document settings and per-node presentation state.
//!
//! Grouped by consumer: document-level settings (compatibility mode,
//! language) answer selector and language queries, script lines serve
//! exception reporting, focus backs `:focus`, and scroll offsets back
//! CSSOM view.

use std::collections::HashMap;

use crate::{Document, NodeId};

/// The document-compatibility mode a query runs under: what html5ever's
/// tree builder reports and parsed pages carry.
///
/// It changes exactly one matching behavior: in full quirks mode, class
/// and id selector values compare ASCII-case-insensitively (the WHATWG
/// id/class quirk). Standards and limited-quirks modes stay exact.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum QuirksMode {
    /// Standards mode: full CSS case rules.
    #[default]
    NoQuirks,
    /// Limited quirks: same selector rules as standards mode.
    LimitedQuirks,
    /// Full quirks: legacy case-insensitive class/id matching.
    Quirks,
}

/// Compatibility mode and document language.
#[derive(Debug, Default)]
pub(crate) struct Settings {
    quirks_mode: QuirksMode,
    document_language: Option<String>,
}

/// Parsed inline script start lines for exception reporting
/// (<https://html.spec.whatwg.org/multipage/webappapis.html#script's-line-number>).
#[derive(Debug, Default)]
pub(crate) struct ScriptLines(HashMap<NodeId, u32>);

/// Per-element scroll offsets
/// (<https://drafts.csswg.org/cssom-view/#dom-element-scrollleft>).
#[derive(Debug, Default)]
pub(crate) struct ScrollOffsets(HashMap<NodeId, (f64, f64)>);

/// Focused element per document
/// (<https://drafts.csswg.org/selectors-4/#the-focus-pseudo>).
#[derive(Debug, Default)]
pub(crate) struct ActiveElements(HashMap<u32, NodeId>);

/// Drops a destroyed node's script line and scroll offset. Focus entries and
/// document-level settings are keyed by document, not by node, so they stay.
pub(crate) fn forget(document: &mut Document, id: NodeId) {
    document.script_lines.0.remove(&id);
    document.scroll_offsets.0.remove(&id);
}

/// Compatibility mode this document answers selector queries under.
#[must_use]
pub fn quirks_mode(document: &Document) -> QuirksMode {
    document.settings.quirks_mode
}

/// Sets the compatibility mode. The html5ever adapter writes this from the
/// tree builder; tests may set it to exercise the id/class quirk.
pub fn set_quirks_mode(document: &mut Document, mode: QuirksMode) {
    document.settings.quirks_mode = mode;
}

/// Document-level language from HTTP `Content-Language`, used by `:lang()`
/// when no element `lang` / `xml:lang` applies.
#[must_use]
pub fn document_language(document: &Document) -> Option<&str> {
    document.settings.document_language.as_deref()
}

/// Sets the document language default. The renderer writes this after
/// navigation; tests may set it directly.
pub fn set_document_language(document: &mut Document, language: Option<String>) {
    document.settings.document_language = language;
}

/// Records the source-text start line of the inline `script` element `id`,
/// as the parser counted it (1-based)
/// (<https://html.spec.whatwg.org/multipage/webappapis.html#script's-line-number>).
pub fn set_script_line(document: &mut Document, id: NodeId, line: u32) {
    document.script_lines.0.insert(id, line);
}

/// The start line recorded for the inline `script` element `id`.
#[must_use]
pub fn script_line(document: &Document, id: NodeId) -> Option<u32> {
    document.script_lines.0.get(&id).copied()
}

/// The scrolled offset `(left, top)` of `id`; `(0, 0)` when never set.
#[must_use]
pub fn scroll_offset(document: &Document, id: NodeId) -> (f64, f64) {
    document
        .scroll_offsets
        .0
        .get(&id)
        .copied()
        .unwrap_or((0.0, 0.0))
}

/// Stores `id`'s scroll offset.
pub fn set_scroll_offset(document: &mut Document, id: NodeId, left: f64, top: f64) {
    document.scroll_offsets.0.insert(id, (left, top));
}

/// Records the focused element for a document, backing `:focus`.
pub fn set_active_element(document: &mut Document, document_id: u32, node: Option<NodeId>) {
    match node {
        Some(node) => {
            document.active_elements.0.insert(document_id, node);
        }
        None => {
            document.active_elements.0.remove(&document_id);
        }
    }
}

/// The focused element for a document, if any.
#[must_use]
pub fn active_element(document: &Document, document_id: u32) -> Option<NodeId> {
    document.active_elements.0.get(&document_id).copied()
}
