//! Engine crate: `parse_html` (html5ever `TreeSink`) and the page
//! (HTML jobs, `Agent`, `QuickJS` host). Depends on `dom` and `net`
//! ([ADR 0007](../../wiki/adrs/0007-engine-charter.md)).
//!
//! Future CDP depends on this crate alone. Fetch is `net::Agent` held here,
//! not a `HttpTransport` trait. `Agent::send` runs through `spawn_blocking`
//! on the page thread.

mod js;
mod page;

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;

use dom::{
    Attribute as DomAttribute, LocalName, Namespace, NodeId as Handle, NodeKind, QualName,
    html_namespace,
};
use html5ever::tree_builder::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use markup5ever::interface::tree_builder::ElemName;
use tendril::{StrTendril, TendrilSink};

pub use dom::{Dom, DomError, NodeId};
pub use net::Agent;
pub use page::{Page, PageError, PageEvent, ScriptFailure};

/// The result of parsing one document.
#[derive(Debug)]
pub struct Parsed {
    /// The parsed tree, rooted at [`Dom::document`].
    pub dom: Dom,
    /// Compatibility mode selected by the doctype (or its absence).
    pub quirks_mode: QuirksMode,
    /// How many spec parse errors the tokenizer/tree builder reported.
    pub parse_errors: u32,
}

/// Parses a full HTML document into a fresh [`Dom`] with the scripting flag
/// enabled (the browser default).
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

// ── the sink ────────────────────────────────────────────────────────────────

struct Sink {
    // `TreeSink` 0.39 hands out `&self`, while every `Dom` mutation needs
    // `&mut self`; hence interior mutability at this one boundary. Sound
    // because the driver is single-threaded and never reenters the sink in
    // the middle of another call: borrows are short, sequential, and cannot
    // overlap. If one ever did overlap, that is an adapter bug and the
    // `RefCell` panics loudly rather than corrupting the tree.
    dom: RefCell<Dom>,
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
            dom: RefCell::new(Dom::new()),
            quirks_mode: Cell::new(QuirksMode::NoQuirks),
            parse_errors: Cell::new(0),
            integration_points: RefCell::new(HashSet::new()),
        }
    }

    /// Places character data under `parent`, coalescing with the neighbor
    /// when that is text: the spec's adjacent-character rule has exactly one
    /// home here. The append path (`before == None`) checks the last child;
    /// the insert path checks `before`'s previous sibling.
    fn insert_text(&self, parent: Handle, before: Option<Handle>, text: &str) {
        let merged = self.neighbor_before(parent, before).and_then(|handle| {
            match self.dom.borrow().get(handle).map(|node| node.kind()) {
                Some(NodeKind::Text { data }) => {
                    let mut merged = data.clone();
                    merged.push_str(text);
                    Some((handle, merged))
                }
                _ => None,
            }
        });
        let Some((handle, merged)) = merged else {
            let fresh = self.dom.borrow_mut().create_text(text);
            let placed = match before {
                None => self.dom.borrow_mut().append(parent, fresh),
                Some(sibling) => self.dom.borrow_mut().insert_before(sibling, fresh),
            };
            placed.expect("builder places text beside a live parented anchor");
            return;
        };
        self.dom
            .borrow_mut()
            .set_text(handle, merged)
            .expect("live text node stays live within one sink call");
    }

    /// The node positioned to absorb new character data: `before`'s previous
    /// sibling, or the current last child on the append path.
    fn neighbor_before(&self, parent: Handle, before: Option<Handle>) -> Option<Handle> {
        let dom = self.dom.borrow();
        let position = match before {
            None => {
                return dom
                    .children(parent)
                    .and_then(|mut kids| kids.next_back().copied());
            }
            Some(sibling) => dom
                .children(parent)
                .and_then(|mut kids| kids.position(|&kid| kid == sibling))
                .expect("parented sibling sits inside its parent's child list"),
        };
        dom.children(parent)
            .and_then(|mut kids| kids.nth(position.checked_sub(1)?))
            .copied()
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
                self.dom
                    .borrow_mut()
                    .append(*parent, node)
                    .expect("builder appends a parentless live node");
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
        self.dom
            .borrow_mut()
            .append(doc, doctype)
            .expect("document accepts its own doctype");
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
                self.dom
                    .borrow_mut()
                    .insert_before(*sibling, node)
                    .expect("builder inserts beside a live parented sibling");
            }
            NodeOrText::AppendText(ref text) => {
                // Merge into the previous sibling when that is text; the
                // builder promises `sibling` itself is not a text node.
                let parent = self
                    .dom
                    .borrow()
                    .parent(*sibling)
                    .expect("sibling has a parent per builder promise");
                self.insert_text(parent, Some(*sibling), text);
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
        self.dom
            .borrow_mut()
            .add_attrs_if_missing(*target, converted)
            .expect("builder adds attributes to a live element");
    }

    fn remove_from_parent(&self, target: &Self::Handle) {
        self.dom
            .borrow_mut()
            .detach(*target)
            .expect("builder detaches non-root elements only");
    }

    fn reparent_children(&self, node: &Self::Handle, new_parent: &Self::Handle) {
        self.dom
            .borrow_mut()
            .reparent_children(*node, *new_parent)
            .expect("builder reparents between live, cycle-free nodes");
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

fn is_html_named(dom: &Dom, id: Handle, local: &str) -> bool {
    match dom.get(id).map(|node| node.kind()) {
        Some(NodeKind::Element { name, .. }) => {
            name.ns == html_namespace() && name.local.as_ref().eq_ignore_ascii_case(local)
        }
        _ => false,
    }
}

fn html_bool_attr(dom: &Dom, id: Handle, local: &str) -> bool {
    match dom.get(id).map(|node| node.kind()) {
        Some(NodeKind::Element { attributes, .. }) => attributes.iter().any(|attribute| {
            attribute.name.ns.is_empty()
                && attribute.name.local.as_ref().eq_ignore_ascii_case(local)
        }),
        _ => false,
    }
}

fn html_attr_value(dom: &Dom, id: Handle, local: &str) -> Option<String> {
    match dom.get(id).map(|node| node.kind()) {
        Some(NodeKind::Element { attributes, .. }) => attributes.iter().find_map(|attribute| {
            (attribute.name.ns.is_empty()
                && attribute.name.local.as_ref().eq_ignore_ascii_case(local))
            .then(|| attribute.value.clone())
        }),
        _ => None,
    }
}

fn nearest_html_select(dom: &Dom, mut id: Handle) -> Option<Handle> {
    loop {
        id = dom.parent(id)?;
        if is_html_named(dom, id, "select") {
            return Some(id);
        }
    }
}

/// [Select display size](https://html.spec.whatwg.org/multipage/form-elements.html#concept-select-size).
fn select_display_size(dom: &Dom, select: Handle) -> u32 {
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
fn html_list_of_options(dom: &Dom, select: Handle) -> Vec<Handle> {
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

fn option_is_disabled(dom: &Dom, option: Handle) -> bool {
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
fn option_is_selected(dom: &Dom, option: Handle, select: Handle) -> bool {
    let options = html_list_of_options(dom, select);
    let mut selected: Vec<bool> = options
        .iter()
        .map(|&id| html_bool_attr(dom, id, "selected"))
        .collect();
    let multiple = html_bool_attr(dom, select, "multiple");
    if !multiple
        && select_display_size(dom, select) == 1
        && !selected.iter().any(|&flag| flag)
        && let Some(index) = options.iter().position(|&id| !option_is_disabled(dom, id))
    {
        selected[index] = true;
    } else if !multiple
        && selected.iter().filter(|flag| **flag).count() >= 2
        && let Some(last) = selected.iter().rposition(|flag| *flag)
    {
        for (index, flag) in selected.iter_mut().enumerate() {
            *flag = index == last;
        }
    }
    options
        .iter()
        .position(|&id| id == option)
        .and_then(|index| selected.get(index).copied())
        .unwrap_or(false)
}

/// [Enabled selectedcontent](https://html.spec.whatwg.org/multipage/form-elements.html#get-a-select-s-enabled-selectedcontent).
fn enabled_selectedcontent(dom: &Dom, select: Handle) -> Option<Handle> {
    if html_bool_attr(dom, select, "multiple") {
        return None;
    }
    first_html_named_descendant(dom, select, "selectedcontent")
}

fn first_html_named_descendant(dom: &Dom, root: Handle, local: &str) -> Option<Handle> {
    let kids = dom.children(root)?;
    for kid in kids.copied() {
        if is_html_named(dom, kid, local) {
            return Some(kid);
        }
        if let Some(found) = first_html_named_descendant(dom, kid, local) {
            return Some(found);
        }
    }
    None
}

/// [Clone an option into a selectedcontent](https://html.spec.whatwg.org/multipage/form-elements.html#clone-an-option-into-a-selectedcontent).
fn clone_option_into_selectedcontent(dom: &mut Dom, option: Handle, selectedcontent: Handle) {
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
