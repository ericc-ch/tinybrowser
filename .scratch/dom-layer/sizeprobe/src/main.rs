//! Throwaway size probe: parses a real-world page into `dom` through a
//! minimal TreeSink, then runs real selector queries against the result.
//!
//! Usage: `dom-sizeprobe <path-to-html>` — prints counts so every code path
//! is genuinely exercised and nothing can be optimized away.
//!
//! This exists only to measure (ticket 08); it is not the production
//! adapter, and correctness corners are cut where they cannot affect size.

use std::cell::{Ref, RefCell};
use std::collections::{HashMap, HashSet};

use html5ever::parse_document;
use html5ever::tendril::{StrTendril, TendrilSink};
use markup5ever::interface::{
    ElementFlags, NodeOrText, QuirksMode, TreeSink,
};
use markup5ever::{
    Attribute as HtmlAttribute, LocalName, Namespace, QualName,
};

use dom::{Attribute, Dom, NodeId, NodeKind};

fn attr_of(a: HtmlAttribute) -> Attribute {
    Attribute {
        name: a.name,
        value: a.value.to_string(),
    }
}

/// A `TreeSink` that builds a `Dom`. Handles are just `NodeId`s; the trait's
/// shared-reference shape means the arena sits behind a `RefCell`.
///
/// `elem_name` cannot borrow through the arena `RefCell` (the guard would
/// die on return), so element names are mirrored into their own locked map
/// whose guard *can* live for `'a`. Probe-local scaffolding; the real
/// adapter will solve this differently (or dom grows a read API for it).
struct DomSink {
    dom: RefCell<Dom>,
    names: RefCell<HashMap<NodeId, QualName>>,
    /// template element -> detached node holding its contents
    templates: RefCell<HashMap<NodeId, NodeId>>,
    integration_points: RefCell<HashSet<NodeId>>,
}

impl DomSink {
    fn new() -> Self {
        Self {
            dom: RefCell::new(Dom::new()),
            names: RefCell::new(HashMap::new()),
            templates: RefCell::new(HashMap::new()),
            integration_points: RefCell::new(HashSet::new()),
        }
    }

    /// Shared body of append / append-before-sibling, with text merging.
    fn insert(&self, parent: NodeId, child: NodeOrText<NodeId>, before: Option<NodeId>) {
        let mut dom = self.dom.borrow_mut();
        match child {
            NodeOrText::AppendNode(id) => match before {
                Some(sibling) => {
                    dom.insert_before(sibling, id).unwrap();
                }
                None => {
                    dom.append(parent, id).unwrap();
                }
            },
            NodeOrText::AppendText(text) => {
                // merge into a trailing text sibling if there is one
                let last = dom.children(parent).and_then(|kids| kids.last().copied());
                let extends_text = before.is_none()
                    && last.is_some_and(|last| {
                        matches!(
                            dom.get(last).map(|n| n.kind()),
                            Some(NodeKind::Text { .. })
                        )
                    });
                if extends_text {
                    let last = last.unwrap();
                    let combined = match dom.get(last).map(|n| n.kind()) {
                        Some(NodeKind::Text { data }) => format!("{data}{text}"),
                        _ => unreachable!("checked above"),
                    };
                    dom.set_text(last, combined).unwrap();
                } else {
                    let id = dom.create_text(text.to_string());
                    match before {
                        Some(sibling) => dom.insert_before(sibling, id).unwrap(),
                        None => dom.append(parent, id).unwrap(),
                    }
                }
            }
        }
    }
}

impl TreeSink for DomSink {
    type Handle = NodeId;
    type Output = Dom;

    type ElemName<'a> = Ref<'a, QualName> where Self: 'a;

    fn finish(self) -> Dom {
        self.dom.into_inner()
    }

    fn parse_error(&self, _msg: std::borrow::Cow<'static, str>) {}

    fn get_document(&self) -> NodeId {
        self.dom.borrow().document()
    }

    fn elem_name<'a>(&'a self, target: &'a NodeId) -> Ref<'a, QualName> {
        Ref::map(self.names.borrow(), |names| {
            names.get(target).expect("elem_name of created element")
        })
    }

    fn create_element(
        &self,
        name: QualName,
        attrs: Vec<HtmlAttribute>,
        flags: ElementFlags,
    ) -> NodeId {
        let mut dom = self.dom.borrow_mut();
        let id =
            dom.create_element(name.clone(), attrs.into_iter().map(attr_of).collect());
        drop(dom);
        self.names.borrow_mut().insert(id, name);
        if flags.template {
            let contents = { 
                self.dom.borrow_mut().create_element(
                    QualName::new(
                        None,
                        Namespace::from(""),
                        LocalName::from("#template-contents"),
                    ),
                    Vec::new(),
                )
            };
            self.templates.borrow_mut().insert(id, contents);
        }
        if flags.mathml_annotation_xml_integration_point {
            self.integration_points.borrow_mut().insert(id);
        }
        id
    }

    fn create_comment(&self, text: StrTendril) -> NodeId {
        self.dom.borrow_mut().create_comment(text.to_string())
    }

    fn create_pi(&self, target: StrTendril, data: StrTendril) -> NodeId {
        // no PI kind in dom; a comment keeps the probe honest enough
        self.dom.borrow_mut().create_comment(format!("{target}: {data}"))
    }

    fn append(&self, parent: &NodeId, child: NodeOrText<NodeId>) {
        self.insert(*parent, child, None);
    }

    fn append_based_on_parent_node(
        &self,
        element: &NodeId,
        prev_element: &NodeId,
        child: NodeOrText<NodeId>,
    ) {
        let has_parent = self.dom.borrow().parent(*element).is_some();
        if has_parent {
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
        let mut dom = self.dom.borrow_mut();
        let doc = dom.document();
        let dt =
            dom.create_doctype(name.to_string(), public_id.to_string(), system_id.to_string());
        dom.append(doc, dt).unwrap();
    }

    fn get_template_contents(&self, target: &NodeId) -> NodeId {
        *self.templates.borrow().get(target).expect("template contents exist")
    }

    fn same_node(&self, x: &NodeId, y: &NodeId) -> bool {
        x == y
    }

    fn set_quirks_mode(&self, _mode: QuirksMode) {}

    fn append_before_sibling(&self, sibling: &NodeId, new_node: NodeOrText<NodeId>) {
        let parent = self
            .dom
            .borrow()
            .parent(*sibling)
            .expect("append_before_sibling target has a parent");
        self.insert(parent, new_node, Some(*sibling));
    }

    fn add_attrs_if_missing(&self, target: &NodeId, attrs: Vec<HtmlAttribute>) {
        self.dom
            .borrow_mut()
            .add_attrs_if_missing(*target, attrs.into_iter().map(attr_of).collect())
            .unwrap();
    }

    fn remove_from_parent(&self, target: &NodeId) {
        self.dom.borrow_mut().detach(*target).unwrap();
    }

    fn reparent_children(&self, node: &NodeId, new_parent: &NodeId) {
        self.dom
            .borrow_mut()
            .reparent_children(*node, *new_parent)
            .unwrap();
    }

    fn is_mathml_annotation_xml_integration_point(&self, handle: &NodeId) -> bool {
        self.integration_points.borrow().contains(handle)
    }
}

fn count_elements(dom: &Dom, id: NodeId) -> usize {
    let mut total = 0;
    if matches!(dom.get(id).map(|n| n.kind()), Some(NodeKind::Element { .. })) {
        total += 1;
    }
    if let Some(kids) = dom.children(id) {
        for kid in kids {
            total += count_elements(dom, *kid);
        }
    }
    total
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: dom-sizeprobe <html>");
    let bytes = std::fs::read(&path).expect("readable page");

    let sink = parse_document(DomSink::new(), Default::default())
        .from_utf8()
        .read_from(&mut std::io::Cursor::new(bytes))
        .expect("page parses");

    let doc = sink.document();

    // walk everything once so tree construction is really paid for
    println!("elements: {}", count_elements(&sink, doc));

    // real queries across the common selector shapes
    for (label, selector) in [
        ("links", "a[href]"),
        ("paragraphs-in-divs", "div > p"),
        ("head-matter", "script, style, link[rel=stylesheet]"),
        ("content-root", "#content"),
        ("images", "img[src]"),
        ("classes", "[class]"),
        ("nth", "table tr:nth-child(2n) td"),
    ] {
        match sink.select_all(doc, selector) {
            Ok(list) => println!("{label}: {} for `{selector}`", list.len()),
            Err(why) => println!("{label}: ERR {why}"),
        }
    }
}
