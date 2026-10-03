//! Page engine for one document: the HTML parser, `dom::Document`, and `QuickJS`
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

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;

use dom::{
    Attribute as DomAttribute, LocalName, Namespace, NodeId as Handle, NodeKind, QualName,
    html_namespace,
};
use html5ever::tree_builder::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use markup5ever::interface::{TokenizerResult, tree_builder::ElemName};
use tendril::{StrTendril, TendrilSink};

mod document;
mod documents;
mod embedded;
mod engine;
mod js;
mod messaging;
mod protocol;
mod remote;
mod render;
mod serialize;
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
#[derive(Debug)]
pub(crate) struct Parsed {
    /// The parsed tree, rooted at [`dom::Document::document`].
    pub document: dom::Document,
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
            document: dom::Document::new(),
            quirks_mode: QuirksMode::NoQuirks,
            content_type,
            ready_state: ReadyState::Complete,
            url: None,
        }
    }
}

pub(crate) enum ParseProgress {
    Script(Handle),
    Done,
}

pub(crate) struct ActiveParser {
    parser: html5ever::Parser<Sink>,
}

/// The parser options every document parse shares; `scripting_enabled` is the
/// document's scripting flag
/// (<https://html.spec.whatwg.org/multipage/parsing.html#scripting-flag>).
fn parse_opts(scripting_enabled: bool) -> html5ever::ParseOpts {
    html5ever::ParseOpts {
        tree_builder: html5ever::tree_builder::TreeBuilderOpts {
            scripting_enabled,
            ..html5ever::tree_builder::TreeBuilderOpts::default()
        },
        ..html5ever::ParseOpts::default()
    }
}

fn convert_attrs(attrs: Vec<markup5ever::Attribute>) -> Vec<DomAttribute> {
    attrs
        .into_iter()
        .map(|attr| DomAttribute {
            name: attr.name,
            value: String::from(attr.value),
        })
        .collect()
}

impl ActiveParser {
    pub(crate) fn new(input: &str) -> Self {
        let parser = html5ever::parse_document(Sink::new(), parse_opts(true));
        parser.input_buffer.push_back(StrTendril::from(input));
        Self { parser }
    }

    pub(crate) fn advance(&self) -> ParseProgress {
        loop {
            match self.parser.tokenizer.feed(&self.parser.input_buffer) {
                TokenizerResult::Done => return ParseProgress::Done,
                TokenizerResult::Script(handle) => return ParseProgress::Script(handle),
                TokenizerResult::EncodingIndicator(_) => {}
            }
        }
    }

    pub(crate) fn take_state(&self) -> Parsed {
        self.parser.tokenizer.sink.sink.take_state()
    }

    pub(crate) fn restore(&self, parsed: Parsed) {
        self.parser.tokenizer.sink.sink.restore(parsed);
    }

    pub(crate) fn insert_html(&self, html: String) {
        self.parser.input_buffer.push_front(StrTendril::from(html));
    }

    pub(crate) fn append_html(&self, html: String) {
        self.parser.input_buffer.push_back(StrTendril::from(html));
    }

    pub(crate) fn finish(self) -> Parsed {
        self.parser.finish()
    }
}

/// Parses a full HTML document into a fresh [`dom::Document`] with the scripting
/// flag enabled (the browser default).
///
/// Broken markup is recovered exactly the way the HTML spec, and therefore
/// every browser, mandates; that recovery is html5ever's job, not ours.
#[cfg(test)]
#[must_use]
pub(crate) fn parse_html(input: &str) -> Parsed {
    let sink = Sink::new();
    html5ever::parse_document(sink, parse_opts(true)).one(input)
}

/// Parses a full HTML document with scripting disabled, the mode `DOMParser`
/// uses
/// (<https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#dom-domparser-parsefromstring>).
#[must_use]
pub(crate) fn parse_html_without_scripting(input: &str) -> Parsed {
    let sink = Sink::new();
    html5ever::parse_document(sink, parse_opts(false)).one(input)
}

/// Parses an HTML fragment with `context` as the
/// [context element](https://html.spec.whatwg.org/multipage/parsing.html#html-fragment-parsing-algorithm)
/// local name, using html5lib's `svg ` / `math ` prefixes for foreign
/// namespaces. The returned tree is a document whose `html` element holds
/// the fragment's nodes (html5ever's fragment root).
#[must_use]
pub(crate) fn parse_html_fragment(input: &str, context: &str, scripting_enabled: bool) -> Parsed {
    let sink = Sink::new();
    html5ever::parse_fragment(
        sink,
        parse_opts(scripting_enabled),
        fragment_context_name(context),
        Vec::new(),
        scripting_enabled,
    )
    .one(input)
}

fn fragment_context_name(spec: &str) -> QualName {
    const SVG: &str = "http://www.w3.org/2000/svg";
    const MATHML: &str = "http://www.w3.org/1998/Math/MathML";
    if let Some(local) = spec.strip_prefix("svg ") {
        QualName::new(None, Namespace::from(SVG), LocalName::from(local))
    } else if let Some(local) = spec.strip_prefix("math ") {
        QualName::new(None, Namespace::from(MATHML), LocalName::from(local))
    } else {
        QualName::new(None, html_namespace(), LocalName::from(spec))
    }
}

// ── the sink ────────────────────────────────────────────────────────────────

struct Sink {
    // `TreeSink` 0.39 hands out `&self`, while every `dom::Document` mutation needs
    // `&mut self`; hence interior mutability at this one boundary. Sound
    // because the driver is single-threaded and never reenters the sink in
    // the middle of another call: borrows are short, sequential, and cannot
    // overlap. If one ever did overlap, that is an adapter bug and the
    // `RefCell` panics loudly rather than corrupting the tree.
    document: RefCell<dom::Document>,
    quirks_mode: Cell<QuirksMode>,
    /// Elements the tree builder flagged as
    /// [HTML integration points](https://html.spec.whatwg.org/multipage/parsing.html#html-integration-point):
    /// `MathML` `annotation-xml` whose `encoding` makes HTML content parse
    /// inside it. The builder asks back through
    /// [`TreeSink::is_mathml_annotation_xml_integration_point`] while deciding
    /// whether foreign-content tokens break out; the default answer (`false`)
    /// mis-nests every child of such elements.
    integration_points: RefCell<HashSet<Handle>>,
    /// The line number html5ever most recently reported for the token being
    /// processed, so an inline `script` can record its source's start line.
    current_line: Cell<u64>,
}

impl Sink {
    fn new() -> Self {
        Self {
            document: RefCell::new(dom::Document::new()),
            quirks_mode: Cell::new(QuirksMode::NoQuirks),
            integration_points: RefCell::new(HashSet::new()),
            current_line: Cell::new(0),
        }
    }

    fn take_state(&self) -> Parsed {
        Parsed {
            document: std::mem::replace(&mut *self.document.borrow_mut(), dom::Document::new()),
            quirks_mode: self.quirks_mode.get(),
            content_type: "text/html",
            ready_state: ReadyState::Loading,
            url: None,
        }
    }

    fn restore(&self, parsed: Parsed) {
        *self.document.borrow_mut() = parsed.document;
        self.quirks_mode.set(parsed.quirks_mode);
    }

    /// Places character data under `parent`, coalescing with the neighbor
    /// when that is text: the spec's adjacent-character rule has exactly one
    /// home here. The append path (`before == None`) checks the last child;
    /// the insert path checks `before`'s previous sibling.
    fn insert_text(&self, parent: Handle, before: Option<Handle>, text: &str) {
        let mut dom = self.document.borrow_mut();
        let neighbor = match before {
            None => dom.children(parent).and_then(|mut kids| kids.next_back()),
            // On the parser path `sibling` is always a child of `parent`, so
            // its previous sibling is the neighbor to coalesce with.
            Some(sibling) => dom.previous_sibling(sibling),
        };
        if let Some(handle) = neighbor
            && matches!(dom.kind(handle), Some(NodeKind::Text { .. }))
        {
            let _ = dom::mutation::append_text(&mut dom, handle, text);
            return;
        }
        let fresh = dom.create_text(text);
        let placed = match before {
            None => dom::mutation::append(&mut dom, parent, fresh),
            Some(sibling) => dom::mutation::insert_before(&mut dom, sibling, fresh),
        };
        // A parser-blocking script may have moved or removed the insertion
        // point since html5ever last yielded. The HTML tree builder ignores
        // that insertion rather than taking down the renderer.
        let _ = placed;
    }
}
/// Owned element-name view satisfying the sink's GAT. `Ref::deref` loans are
/// statement-scoped, so instead of smuggling a guard out we clone the two
/// interned atoms (each one machine word) per query.
#[derive(Debug)]
struct OwnedElemName {
    ns: markup5ever::Namespace,
    local: markup5ever::LocalName,
}

impl ElemName for OwnedElemName {
    fn ns(&self) -> &markup5ever::Namespace {
        &self.ns
    }

    fn local_name(&self) -> &markup5ever::LocalName {
        &self.local
    }
}

impl TreeSink for Sink {
    type Handle = Handle;
    type Output = Parsed;
    type ElemName<'a>
        = OwnedElemName
    where
        Self: 'a;

    fn finish(self) -> Self::Output {
        Parsed {
            document: self.document.into_inner(),
            quirks_mode: self.quirks_mode.get(),
            content_type: "text/html",
            ready_state: ReadyState::Loading,
            url: None,
        }
    }

    /// Part of the `TreeSink` contract; parsing recovery is the point, and no
    /// consumer reads a parse-error count.
    fn parse_error(&self, _msg: Cow<'static, str>) {}

    /// html5ever reports the line of each token before the tree builder
    /// processes it; remember it so a `script` start tag can record the line
    /// its source text starts on.
    fn set_current_line(&self, line_number: u64) {
        self.current_line.set(line_number);
    }

    fn get_document(&self) -> Self::Handle {
        self.document.borrow().document()
    }

    fn elem_name<'a>(&'a self, target: &'a Self::Handle) -> Self::ElemName<'a> {
        match self.document.borrow().kind(*target) {
            Some(NodeKind::Element { name, .. }) => OwnedElemName {
                ns: name.ns.clone(),
                local: name.local.clone(),
            },
            _ => panic!("elem_name called on a non-element or dead handle"),
        }
    }

    fn create_element(
        &self,
        name: QualName,
        attrs: Vec<markup5ever::Attribute>,
        flags: ElementFlags,
    ) -> Self::Handle {
        let is_script = name.ns == dom::html_namespace() && name.local.as_ref() == "script";
        let line = self.current_line.get();
        let element = self
            .document
            .borrow_mut()
            .create_element(name, convert_attrs(attrs));
        if is_script {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "a document line beyond u32 is not reachable"
            )]
            dom::metadata::set_script_line(&mut self.document.borrow_mut(), element, line as u32);
        }
        if flags.template {
            let contents = self.document.borrow_mut().create_fragment();
            dom::shadow::set_template_contents(&mut self.document.borrow_mut(), element, contents)
                .expect("fresh template element accepts a fresh contents fragment");
        }
        if flags.mathml_annotation_xml_integration_point {
            self.integration_points.borrow_mut().insert(element);
        }
        element
    }

    fn create_comment(&self, text: StrTendril) -> Self::Handle {
        self.document.borrow_mut().create_comment(&*text)
    }

    /// Per the HTML spec, processing instructions become comments whose data
    /// is `target + ' ' + data`.
    fn create_pi(&self, target: StrTendril, data: StrTendril) -> Self::Handle {
        let combined = format!("{target} {data}");
        self.document.borrow_mut().create_comment(combined)
    }

    fn append(&self, parent: &Self::Handle, child: NodeOrText<Self::Handle>) {
        match child {
            NodeOrText::AppendNode(node) => {
                let _ = dom::mutation::append(&mut self.document.borrow_mut(), *parent, node);
            }
            NodeOrText::AppendText(ref text) => self.insert_text(*parent, None, text),
        }
    }

    fn append_based_on_parent_node(
        &self,
        element: &Self::Handle,
        prev_element: &Self::Handle,
        child: NodeOrText<Self::Handle>,
    ) {
        if self.document.borrow().parent(*element).is_some() {
            self.append_before_sibling(element, child);
        } else {
            self.append(prev_element, child);
        }
    }

    fn append_doctype_to_document(
        &self,
        name: StrTendril,
        public_id: StrTendril,
        system_id: StrTendril,
    ) {
        let doc = self.get_document();
        let doctype = self
            .document
            .borrow_mut()
            .create_doctype(name, public_id, system_id);
        let _ = dom::mutation::append(&mut self.document.borrow_mut(), doc, doctype);
    }

    fn get_template_contents(&self, target: &Self::Handle) -> Self::Handle {
        dom::shadow::template_contents(&self.document.borrow(), *target)
            .unwrap_or_else(|| panic!("template contents requested for a non-template"))
    }

    /// Answers the builder's integration-point question from the flag it
    /// handed us at [`Sink::create_element`] time, per the
    /// [HTML integration point](https://html.spec.whatwg.org/multipage/parsing.html#html-integration-point)
    /// definition, an `annotation-xml` with `encoding="text/html"` (ASCII
    /// case-insensitive) or `"application/xhtml+xml"`.
    fn is_mathml_annotation_xml_integration_point(&self, target: &Self::Handle) -> bool {
        self.integration_points.borrow().contains(target)
    }

    fn same_node(&self, x: &Self::Handle, y: &Self::Handle) -> bool {
        x == y
    }

    fn set_quirks_mode(&self, mode: QuirksMode) {
        self.quirks_mode.set(mode);
        let stored = match mode {
            QuirksMode::NoQuirks => dom::QuirksMode::NoQuirks,
            QuirksMode::LimitedQuirks => dom::QuirksMode::LimitedQuirks,
            QuirksMode::Quirks => dom::QuirksMode::Quirks,
        };
        dom::metadata::set_quirks_mode(&mut self.document.borrow_mut(), stored);
    }

    fn append_before_sibling(&self, sibling: &Self::Handle, new_node: NodeOrText<Self::Handle>) {
        match new_node {
            NodeOrText::AppendNode(node) => {
                let _ =
                    dom::mutation::insert_before(&mut self.document.borrow_mut(), *sibling, node);
            }
            NodeOrText::AppendText(ref text) => {
                // Merge into the previous sibling when that is text; the
                // builder promises `sibling` itself is not a text node.
                let parent = { self.document.borrow().parent(*sibling) };
                if let Some(parent) = parent {
                    self.insert_text(parent, Some(*sibling), text);
                }
            }
        }
    }

    fn add_attrs_if_missing(&self, target: &Self::Handle, attrs: Vec<markup5ever::Attribute>) {
        let _ = dom::mutation::add_attrs_if_missing(
            &mut self.document.borrow_mut(),
            *target,
            convert_attrs(attrs),
        );
    }

    fn remove_from_parent(&self, target: &Self::Handle) {
        let _ = dom::mutation::detach(&mut self.document.borrow_mut(), *target);
    }

    fn reparent_children(&self, node: &Self::Handle, new_parent: &Self::Handle) {
        let _ =
            dom::mutation::reparent_children(&mut self.document.borrow_mut(), *node, *new_parent);
    }

    /// [Maybe clone an option into selectedcontent](https://html.spec.whatwg.org/multipage/form-elements.html#maybe-clone-an-option-into-selectedcontent).
    fn maybe_clone_an_option_into_selectedcontent(&self, option: &Self::Handle) {
        dom::form::maybe_clone_option_into_selectedcontent(
            &mut self.document.borrow_mut(),
            *option,
        );
    }
}
