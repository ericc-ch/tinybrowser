//! Document metadata and per-node presentation state.

use std::collections::HashMap;

use super::{Dom, NodeId, QuirksMode};

#[derive(Debug, Default)]
pub(super) struct Metadata {
    quirks_mode: QuirksMode,
    /// Document's default language for `:lang()`.
    document_language: Option<String>,
    /// Parsed inline script start lines for exception reporting
    /// (<https://html.spec.whatwg.org/multipage/webappapis.html#script's-line-number>).
    script_lines: HashMap<NodeId, u32>,
    /// Focused element per document (<https://drafts.csswg.org/selectors-4/#the-focus-pseudo>).
    active_element: HashMap<u32, NodeId>,
    /// Per-element scroll offsets (<https://drafts.csswg.org/cssom-view/#dom-element-scrollleft>).
    scroll_offsets: HashMap<NodeId, (f64, f64)>,
}

impl Metadata {
    pub(super) fn forget(&mut self, id: NodeId) {
        self.script_lines.remove(&id);
        self.scroll_offsets.remove(&id);
    }
}

impl Dom {
    /// Compatibility mode this document answers selector queries under.
    #[must_use]
    pub fn quirks_mode(&self) -> QuirksMode {
        self.metadata.quirks_mode
    }

    /// Sets the compatibility mode. The html5ever adapter writes this from
    /// the tree builder; tests may set it to exercise the id/class quirk.
    pub fn set_quirks_mode(&mut self, mode: QuirksMode) {
        self.metadata.quirks_mode = mode;
    }

    /// Document-level language from HTTP `Content-Language`, used by
    /// `:lang()` when no element `lang` / `xml:lang` applies.
    #[must_use]
    pub fn document_language(&self) -> Option<&str> {
        self.metadata.document_language.as_deref()
    }

    /// Sets the document language default. `browser` writes this after
    /// navigation; tests may set it directly.
    pub fn set_document_language(&mut self, language: Option<String>) {
        self.metadata.document_language = language;
    }

    /// Records the source-text start line of the inline `script` element `id`,
    /// as the parser counted it (1-based)
    /// (<https://html.spec.whatwg.org/multipage/webappapis.html#script's-line-number>).
    pub fn set_script_line(&mut self, id: NodeId, line: u32) {
        self.metadata.script_lines.insert(id, line);
    }

    /// The start line recorded for the inline `script` element `id`.
    #[must_use]
    pub fn script_line(&self, id: NodeId) -> Option<u32> {
        self.metadata.script_lines.get(&id).copied()
    }

    /// The scrolled offset `(left, top)` of `id`; `(0, 0)` when never set.
    #[must_use]
    pub fn scroll_offset(&self, id: NodeId) -> (f64, f64) {
        self.metadata
            .scroll_offsets
            .get(&id)
            .copied()
            .unwrap_or((0.0, 0.0))
    }

    /// Stores `id`'s scroll offset.
    pub fn set_scroll_offset(&mut self, id: NodeId, left: f64, top: f64) {
        self.metadata.scroll_offsets.insert(id, (left, top));
    }

    /// Records the focused element for a document, backing `:focus`.
    pub fn set_active_element(&mut self, document: u32, node: Option<NodeId>) {
        match node {
            Some(node) => {
                self.metadata.active_element.insert(document, node);
            }
            None => {
                self.metadata.active_element.remove(&document);
            }
        }
    }

    /// The focused element for a document, if any.
    #[must_use]
    pub fn active_element(&self, document: u32) -> Option<NodeId> {
        self.metadata.active_element.get(&document).copied()
    }
}
