//! Renderer crate: the page engine for one document — HTML parser,
//! `Dom`, `QuickJS` — behind the value-only browser seam.
//!
//! [ADR 0011](../../../docs/adrs/0011-renderer-processes-per-site.md): the
//! renderer owns `Document` and never links `net`; the browser process owns `Tab`, the
//! tab, navigation, network, and cookies. The same [`run`] loop backs the
//! in-process backend and the `--renderer` child.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::time::Duration;

use dom::{
    Attribute as DomAttribute, LocalName, Namespace, NodeId as Handle, NodeKind, QualName,
    html_namespace,
};
use html5ever::tree_builder::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use markup5ever::interface::{TokenizerResult, tree_builder::ElemName};
use tendril::{StrTendril, TendrilSink};

mod document;
mod documents;
mod engine;
mod js;
mod process;
mod protocol;
mod remote;

pub use document::{Document, ScriptValue, Stop};
pub use engine::Engine;
pub use process::serve_stdio;
pub use protocol::{
    BrowserServices, Command, DialCompletion, DialKind, DialOutcome, DialRequest, FrameId,
    FromRenderer, MAX_IPC_MESSAGE_BYTES, Mount, RENDERER_INBOX_CAPACITY, RENDERER_OUTBOX_CAPACITY,
    Reply, ResourceLimit, ScriptFailure, ServiceCall, ServiceReply, TabError, TabEvent, ToRenderer,
    encode_ipc_message, read_ipc_message,
};
pub use remote::RemoteValue;

/// The result of parsing one document.
#[derive(Debug)]
pub struct Parsed {
    /// The parsed tree, rooted at [`Dom::document`].
    pub dom: dom::Dom,
    /// Compatibility mode selected by the doctype (or its absence).
    pub quirks_mode: QuirksMode,
    /// How many spec parse errors the tokenizer/tree builder reported.
    pub parse_errors: u32,
    /// MIME type this document reports from `document.contentType`.
    pub content_type: &'static str,
}

pub(crate) enum ParseProgress {
    Script(Handle),
    Done,
}

pub(crate) struct ActiveParser {
    parser: html5ever::Parser<Sink>,
}

impl ActiveParser {
    pub(crate) fn new(input: &str) -> Self {
        let opts = html5ever::ParseOpts {
            tree_builder: html5ever::tree_builder::TreeBuilderOpts {
                scripting_enabled: true,
                ..html5ever::tree_builder::TreeBuilderOpts::default()
            },
            ..html5ever::ParseOpts::default()
        };
        let parser = html5ever::parse_document(Sink::new(), opts);
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

/// Parses a full HTML document into a fresh [`dom::Dom`] with the scripting
/// flag enabled (the browser default).
///
/// Broken markup is recovered exactly the way the HTML spec, and therefore
/// every browser, mandates; that recovery is html5ever's job, not ours.
#[must_use]
pub fn parse_html(input: &str) -> Parsed {
    parse_html_with_scripting(input, true)
}

/// Parses a full HTML document with the tree builder's
/// [scripting flag](https://html.spec.whatwg.org/multipage/parsing.html#scripting-flag)
/// set explicitly. The flag changes how `<noscript>` contents are parsed and
/// feeds form-control behavior; conformance suites run both settings.
#[must_use]
pub fn parse_html_with_scripting(input: &str, scripting_enabled: bool) -> Parsed {
    let opts = html5ever::ParseOpts {
        tree_builder: html5ever::tree_builder::TreeBuilderOpts {
            scripting_enabled,
            ..html5ever::tree_builder::TreeBuilderOpts::default()
        },
        ..html5ever::ParseOpts::default()
    };
    let sink = Sink::new();
    html5ever::parse_document(sink, opts).one(input)
}

/// Parses an HTML fragment with `context` as the
/// [context element](https://html.spec.whatwg.org/multipage/parsing.html#html-fragment-parsing-algorithm)
/// local name, using html5lib's `svg ` / `math ` prefixes for foreign
/// namespaces. The returned tree is a document whose `html` element holds
/// the fragment's nodes (html5ever's fragment root).
#[must_use]
pub fn parse_html_fragment(input: &str, context: &str, scripting_enabled: bool) -> Parsed {
    let opts = html5ever::ParseOpts {
        tree_builder: html5ever::tree_builder::TreeBuilderOpts {
            scripting_enabled,
            ..html5ever::tree_builder::TreeBuilderOpts::default()
        },
        ..html5ever::ParseOpts::default()
    };
    let sink = Sink::new();
    html5ever::parse_fragment(
        sink,
        opts,
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

/// Runs one renderer loop until `Shutdown` or its inbox closes.
///
/// The in-process backend calls this on a thread; the `--renderer` child calls
/// it with the pipe's channels. Events and replies go to `outbox`.
pub fn run(
    inbox: &Receiver<ToRenderer>,
    outbox: &SyncSender<FromRenderer>,
    services: Arc<dyn BrowserServices>,
) {
    run_with_stop(inbox, outbox, services, &Arc::new(Stop::new()));
}

/// [`run`] with an externally owned stop flag, so an in-process host can
/// interrupt a runaway script.
pub fn run_with_stop(
    inbox: &Receiver<ToRenderer>,
    outbox: &SyncSender<FromRenderer>,
    services: Arc<dyn BrowserServices>,
    stop: &Arc<Stop>,
) {
    let mut engine = Engine::with_stop(services, Arc::clone(stop));
    loop {
        let received = if engine.has_background_work() {
            inbox.recv_timeout(Duration::from_millis(10))
        } else {
            inbox.recv().map_err(|_| RecvTimeoutError::Disconnected)
        };
        match received {
            Ok(ToRenderer::Request { id, command }) => {
                let (reply, shutdown) = handle_command(&mut engine, command, stop);
                if !send_to_browser(outbox, FromRenderer::Reply { id, reply }) {
                    stop.request();
                    break;
                }
                if shutdown {
                    break;
                }
            }
            // Service replies are routed by the transport, never delivered here.
            Ok(ToRenderer::ServiceReply { .. }) => {}
            Err(RecvTimeoutError::Timeout) => engine.drive_for(Duration::from_millis(10)),
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if !publish(&mut engine, outbox) {
            stop.request();
            break;
        }
    }
    engine.shutdown();
}

fn handle_command(
    engine: &mut Engine,
    command: Command,
    stop: &Arc<document::Stop>,
) -> (Reply, bool) {
    match command {
        Command::Mount { frame, mount } => (Reply::Unit(engine.mount_frame(frame, &mount)), false),
        Command::Eval { frame, source } => (Reply::Text(engine.eval_in(frame, &source)), false),
        Command::ExecuteScript {
            frame,
            source,
            timeout_ms,
        } => {
            let timeout = timeout_ms.map(Duration::from_millis);
            let value = engine.execute_remote_in(frame, &source, timeout);
            (Reply::Value(value), false)
        }
        Command::SetDocumentUrl { frame, url } => {
            (Reply::Unit(engine.set_document_url_in(frame, &url)), false)
        }
        Command::IsIdle => (Reply::Bool(!engine.has_background_work()), false),
        Command::Shutdown => {
            stop.request();
            (Reply::Unit(Ok(())), true)
        }
    }
}

fn publish(engine: &mut Engine, outbox: &SyncSender<FromRenderer>) -> bool {
    let Ok(events) = engine.take_events() else {
        return false;
    };
    for (frame, event) in events {
        if !send_to_browser(outbox, FromRenderer::Event { frame, event }) {
            return false;
        }
    }
    true
}

fn send_to_browser(outbox: &SyncSender<FromRenderer>, message: FromRenderer) -> bool {
    match outbox.try_send(message) {
        Ok(()) => true,
        Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => false,
    }
}

// ── the sink ────────────────────────────────────────────────────────────────

struct Sink {
    // `TreeSink` 0.39 hands out `&self`, while every `dom::Dom` mutation needs
    // `&mut self`; hence interior mutability at this one boundary. Sound
    // because the driver is single-threaded and never reenters the sink in
    // the middle of another call: borrows are short, sequential, and cannot
    // overlap. If one ever did overlap, that is an adapter bug and the
    // `RefCell` panics loudly rather than corrupting the tree.
    dom: RefCell<dom::Dom>,
    quirks_mode: Cell<QuirksMode>,
    parse_errors: Cell<u32>,
    /// Elements the tree builder flagged as
    /// [HTML integration points](https://html.spec.whatwg.org/multipage/parsing.html#html-integration-point):
    /// `MathML` `annotation-xml` whose `encoding` makes HTML content parse
    /// inside it. The builder asks back through
    /// [`TreeSink::is_mathml_annotation_xml_integration_point`] while deciding
    /// whether foreign-content tokens break out; the default answer (`false`)
    /// mis-nests every child of such elements.
    integration_points: RefCell<HashSet<Handle>>,
}

impl Sink {
    fn new() -> Self {
        Self {
            dom: RefCell::new(dom::Dom::new()),
            quirks_mode: Cell::new(QuirksMode::NoQuirks),
            parse_errors: Cell::new(0),
            integration_points: RefCell::new(HashSet::new()),
        }
    }

    fn take_state(&self) -> Parsed {
        Parsed {
            dom: std::mem::replace(&mut *self.dom.borrow_mut(), dom::Dom::new()),
            quirks_mode: self.quirks_mode.get(),
            parse_errors: self.parse_errors.get(),
            content_type: "text/html",
        }
    }

    fn restore(&self, parsed: Parsed) {
        *self.dom.borrow_mut() = parsed.dom;
        self.quirks_mode.set(parsed.quirks_mode);
        self.parse_errors.set(parsed.parse_errors);
    }

    /// Places character data under `parent`, coalescing with the neighbor
    /// when that is text: the spec's adjacent-character rule has exactly one
    /// home here. The append path (`before == None`) checks the last child;
    /// the insert path checks `before`'s previous sibling.
    fn insert_text(&self, parent: Handle, before: Option<Handle>, text: &str) {
        let mut dom = self.dom.borrow_mut();
        let neighbor = match before {
            None => dom
                .children(parent)
                .and_then(|mut kids| kids.next_back().copied()),
            Some(sibling) => dom
                .children(parent)
                .and_then(|mut kids| kids.position(|&kid| kid == sibling))
                .and_then(|position| {
                    dom.children(parent)
                        .and_then(|mut kids| kids.nth(position.checked_sub(1)?))
                        .copied()
                }),
        };
        if let Some(handle) = neighbor
            && let Some(NodeKind::Text { data }) = dom.get(handle).map(|node| node.kind())
        {
            let mut merged = data.clone();
            merged.push_str(text);
            let _ = dom.set_text(handle, merged);
            return;
        }
        let fresh = dom.create_text(text);
        let placed = match before {
            None => dom.append(parent, fresh),
            Some(sibling) => dom.insert_before(sibling, fresh),
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
            dom: self.dom.into_inner(),
            quirks_mode: self.quirks_mode.get(),
            parse_errors: self.parse_errors.get(),
            content_type: "text/html",
        }
    }

    fn parse_error(&self, _msg: Cow<'static, str>) {
        self.parse_errors.set(self.parse_errors.get() + 1);
    }

    fn get_document(&self) -> Self::Handle {
        self.dom.borrow().document()
    }

    fn elem_name<'a>(&'a self, target: &'a Self::Handle) -> Self::ElemName<'a> {
        match self.dom.borrow().get(*target).map(|node| node.kind()) {
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
        let converted: Vec<DomAttribute> = attrs
            .into_iter()
            .map(|attr| DomAttribute {
                name: attr.name,
                value: attr.value.to_string(),
            })
            .collect();
        let element = self.dom.borrow_mut().create_element(name, converted);
        if flags.template {
            let contents = self.dom.borrow_mut().create_fragment();
            self.dom
                .borrow_mut()
                .set_template_contents(element, contents)
                .expect("fresh template element accepts a fresh contents fragment");
        }
        if flags.mathml_annotation_xml_integration_point {
            self.integration_points.borrow_mut().insert(element);
        }
        element
    }

    fn create_comment(&self, text: StrTendril) -> Self::Handle {
        self.dom.borrow_mut().create_comment(text.to_string())
    }

    /// Per the HTML spec, processing instructions become comments whose data
    /// is `target + ' ' + data`.
    fn create_pi(&self, target: StrTendril, data: StrTendril) -> Self::Handle {
        let combined = format!("{target} {data}");
        self.dom.borrow_mut().create_comment(combined)
    }

    fn append(&self, parent: &Self::Handle, child: NodeOrText<Self::Handle>) {
        match child {
            NodeOrText::AppendNode(node) => {
                let _ = self.dom.borrow_mut().append(*parent, node);
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
        if self.dom.borrow().parent(*element).is_some() {
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
        let doctype = self.dom.borrow_mut().create_doctype(
            name.to_string(),
            public_id.to_string(),
            system_id.to_string(),
        );
        let _ = self.dom.borrow_mut().append(doc, doctype);
    }

    fn get_template_contents(&self, target: &Self::Handle) -> Self::Handle {
        self.dom
            .borrow()
            .template_contents(*target)
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
        self.dom.borrow_mut().set_quirks_mode(stored);
    }

    fn append_before_sibling(&self, sibling: &Self::Handle, new_node: NodeOrText<Self::Handle>) {
        match new_node {
            NodeOrText::AppendNode(node) => {
                let _ = self.dom.borrow_mut().insert_before(*sibling, node);
            }
            NodeOrText::AppendText(ref text) => {
                // Merge into the previous sibling when that is text; the
                // builder promises `sibling` itself is not a text node.
                let parent = { self.dom.borrow().parent(*sibling) };
                if let Some(parent) = parent {
                    self.insert_text(parent, Some(*sibling), text);
                }
            }
        }
    }

    fn add_attrs_if_missing(&self, target: &Self::Handle, attrs: Vec<markup5ever::Attribute>) {
        let converted: Vec<DomAttribute> = attrs
            .into_iter()
            .map(|attr| DomAttribute {
                name: attr.name,
                value: attr.value.to_string(),
            })
            .collect();
        let _ = self
            .dom
            .borrow_mut()
            .add_attrs_if_missing(*target, converted);
    }

    fn remove_from_parent(&self, target: &Self::Handle) {
        let _ = self.dom.borrow_mut().detach(*target);
    }

    fn reparent_children(&self, node: &Self::Handle, new_parent: &Self::Handle) {
        let _ = self.dom.borrow_mut().reparent_children(*node, *new_parent);
    }

    /// [Maybe clone an option into selectedcontent](https://html.spec.whatwg.org/multipage/form-elements.html#maybe-clone-an-option-into-selectedcontent).
    fn maybe_clone_an_option_into_selectedcontent(&self, option: &Self::Handle) {
        let mut dom = self.dom.borrow_mut();
        let Some(select) = nearest_html_select(&dom, *option) else {
            return;
        };
        if !option_is_selected(&dom, *option, select) {
            return;
        }
        let Some(selectedcontent) = enabled_selectedcontent(&dom, select) else {
            return;
        };
        clone_option_into_selectedcontent(&mut dom, *option, selectedcontent);
    }
}

fn is_html_named(dom: &dom::Dom, id: Handle, local: &str) -> bool {
    match dom.get(id).map(|node| node.kind()) {
        Some(NodeKind::Element { name, .. }) => {
            name.ns == html_namespace() && name.local.as_ref().eq_ignore_ascii_case(local)
        }
        _ => false,
    }
}

fn html_bool_attr(dom: &dom::Dom, id: Handle, local: &str) -> bool {
    match dom.get(id).map(|node| node.kind()) {
        Some(NodeKind::Element { attributes, .. }) => attributes.iter().any(|attribute| {
            attribute.name.ns.is_empty()
                && attribute.name.local.as_ref().eq_ignore_ascii_case(local)
        }),
        _ => false,
    }
}

fn html_attr_value(dom: &dom::Dom, id: Handle, local: &str) -> Option<String> {
    match dom.get(id).map(|node| node.kind()) {
        Some(NodeKind::Element { attributes, .. }) => attributes.iter().find_map(|attribute| {
            (attribute.name.ns.is_empty()
                && attribute.name.local.as_ref().eq_ignore_ascii_case(local))
            .then(|| attribute.value.clone())
        }),
        _ => None,
    }
}

fn nearest_html_select(dom: &dom::Dom, mut id: Handle) -> Option<Handle> {
    loop {
        id = dom.parent(id)?;
        if is_html_named(dom, id, "select") {
            return Some(id);
        }
    }
}

/// [Select display size](https://html.spec.whatwg.org/multipage/form-elements.html#concept-select-size).
fn select_display_size(dom: &dom::Dom, select: Handle) -> u32 {
    if let Some(raw) = html_attr_value(dom, select, "size")
        && let Ok(size) = raw.trim().parse::<u32>()
        && size > 0
    {
        return size;
    }
    if html_bool_attr(dom, select, "multiple") {
        4
    } else {
        1
    }
}

/// [List of options](https://html.spec.whatwg.org/multipage/form-elements.html#concept-select-option-list).
fn html_list_of_options(dom: &dom::Dom, select: Handle) -> Vec<Handle> {
    let mut out = Vec::new();
    let Some(kids) = dom.children(select) else {
        return out;
    };
    for kid in kids.copied() {
        if is_html_named(dom, kid, "option") {
            out.push(kid);
        } else if is_html_named(dom, kid, "optgroup")
            && let Some(grouped) = dom.children(kid)
        {
            for inner in grouped.copied() {
                if is_html_named(dom, inner, "option") {
                    out.push(inner);
                }
            }
        }
    }
    out
}

fn option_is_disabled(dom: &dom::Dom, option: Handle) -> bool {
    if html_bool_attr(dom, option, "disabled") {
        return true;
    }
    let Some(parent) = dom.parent(option) else {
        return false;
    };
    is_html_named(dom, parent, "optgroup") && html_bool_attr(dom, parent, "disabled")
}

/// Parse-time [selectedness](https://html.spec.whatwg.org/multipage/form-elements.html#concept-option-selectedness)
/// plus the [selectedness setting algorithm](https://html.spec.whatwg.org/multipage/form-elements.html#selectedness-setting-algorithm).
fn option_is_selected(dom: &dom::Dom, option: Handle, select: Handle) -> bool {
    let options = html_list_of_options(dom, select);
    if html_bool_attr(dom, select, "multiple") {
        return options.contains(&option) && html_bool_attr(dom, option, "selected");
    }
    let selected = options
        .iter()
        .rfind(|&&id| html_bool_attr(dom, id, "selected"))
        .or_else(|| {
            (select_display_size(dom, select) == 1)
                .then(|| options.iter().find(|&&id| !option_is_disabled(dom, id)))
                .flatten()
        });
    selected == Some(&option)
}

/// [Enabled selectedcontent](https://html.spec.whatwg.org/multipage/form-elements.html#get-a-select-s-enabled-selectedcontent).
fn enabled_selectedcontent(dom: &dom::Dom, select: Handle) -> Option<Handle> {
    if html_bool_attr(dom, select, "multiple") {
        return None;
    }
    let mut pending: Vec<_> = dom.children(select)?.rev().copied().collect();
    while let Some(id) = pending.pop() {
        if is_html_named(dom, id, "selectedcontent") {
            // https://html.spec.whatwg.org/multipage/form-elements.html#the-selectedcontent-element
            let mut ancestor = dom.parent(id);
            while let Some(parent) = ancestor {
                if is_html_named(dom, parent, "option")
                    || is_html_named(dom, parent, "selectedcontent")
                {
                    return None;
                }
                ancestor = dom.parent(parent);
            }
            return Some(id);
        }
        if let Some(kids) = dom.children(id) {
            pending.extend(kids.rev().copied());
        }
    }
    None
}

/// [Clone an option into a selectedcontent](https://html.spec.whatwg.org/multipage/form-elements.html#clone-an-option-into-a-selectedcontent).
fn clone_option_into_selectedcontent(dom: &mut dom::Dom, option: Handle, selectedcontent: Handle) {
    let stale: Vec<Handle> = dom
        .children(selectedcontent)
        .map(|children| children.copied().collect())
        .unwrap_or_default();
    for child in stale {
        dom.destroy(child)
            .expect("selectedcontent children are live descendants");
    }
    let kids: Vec<Handle> = dom
        .children(option)
        .map(|children| children.copied().collect())
        .unwrap_or_default();
    for kid in kids {
        let cloned = dom
            .clone_node(kid, true)
            .expect("option children clone into new nodes");
        dom.append(selectedcontent, cloned)
            .expect("selectedcontent accepts cloned option children");
    }
}
