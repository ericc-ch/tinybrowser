//! `PageActor` and navigation lifecycle: sessions, waits, wiring.
//!
//! The single fan-in point over `dom`, `net`, and `js`. Injects the HTTP
//! adapter into the JS runtime. Everything above it goes through here; when
//! CDP arrives it will depend on this crate alone.
//!
//! This crate also hosts the html5ever [`TreeSink`] adapter (per
//! docs/adr/0002-dom-layer-architecture.md): the parser narrates tree
//! construction, and [`Sink`] translates each instruction into `Dom`
//! mutations. Storage stays parser-free in `dom`; parsing stays storage-free
//! here.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

use dom::{Attribute as DomAttribute, Dom, NodeId as Handle, NodeKind, QualName};
use html5ever::tree_builder::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use markup5ever::interface::tree_builder::ElemName;
use tendril::{StrTendril, TendrilSink};

/// The result of parsing one document.
#[derive(Debug)]
pub struct Parsed {
    /// The parsed tree, rooted at [`Dom::document`].
    pub dom: Dom,
    /// Compatibility mode selected by the doctype (or its absence).
    pub quirks_mode: QuirksMode,
    /// How many spec parse errors the tokenizer/tree builder reported.
    pub parse_errors: u32,
    /// `<template>` element → its contents fragment. Contents live *outside*
    /// the child list per spec ([ADR 0003](../../docs/adr/0003-treesink-adapter-in-browser.md)),
    /// so this map is the only way to reach them — exactly like the
    /// `template.content` DOM property.
    pub template_contents: HashMap<Handle, Handle>,
}

/// Parses a full HTML document into a fresh [`Dom`] with the scripting flag
/// enabled (the browser default).
///
/// Broken markup is recovered exactly the way the HTML spec — and therefore
/// every browser — mandates; that recovery is html5ever's job, not ours.
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

// ── the sink ────────────────────────────────────────────────────────────────

struct Sink {
    // `TreeSink` 0.39 hands out `&self`, while every `Dom` mutation needs
    // `&mut self` — hence interior mutability at this one boundary. Sound
    // because the driver is single-threaded and never reenters the sink in
    // the middle of another call: borrows are short, sequential, and cannot
    // overlap. If one ever did overlap, that is an adapter bug and the
    // `RefCell` panics loudly rather than corrupting the tree.
    dom: RefCell<Dom>,
    quirks_mode: Cell<QuirksMode>,
    parse_errors: Cell<u32>,
    /// `<template>` element → its contents fragment. Contents live *outside*
    /// the child list per spec, so a side map keeps them out of `children()`.
    template_contents: RefCell<HashMap<Handle, Handle>>,
    /// Elements the tree builder flagged as
    /// [HTML integration points](https://html.spec.whatwg.org/multipage/parsing.html#html-integration-point)
    /// — `MathML` `annotation-xml` whose `encoding` makes HTML content parse
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
            template_contents: RefCell::new(HashMap::new()),
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
            template_contents: self.template_contents.into_inner(),
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
            self.template_contents
                .borrow_mut()
                .insert(element, contents);
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
        *self
            .template_contents
            .borrow()
            .get(target)
            .unwrap_or_else(|| panic!("template contents requested for a non-template"))
    }

    /// Answers the builder's integration-point question from the flag it
    /// handed us at [`Sink::create_element`] time — per the
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
}
