//! Window named access: the index of supported property names
//! (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#named-access-on-the-window-object>).
//!
//! A global-property miss consults this index to decide whether the name is a
//! named property. The index must never miss a name, so within a rebuild it
//! grows only: a removed name costs one extra value lookup that resolves to
//! nothing, while a name absent from the index would silently break named
//! access. The document is scanned once, on first use, and insertion and
//! attribute hooks add names as mutations arrive.
//!
//! Growth is bounded by rebuilding from the current document when churn
//! (names that no longer exist) grows the set past the live count. A rebuild
//! can only drop stale names; every live name is rescanned, so the index still
//! never misses one.

use std::collections::HashSet;

use crate::arena::Tree;
use crate::{Document, NodeId, NodeKind, html_namespace, lifecycle};

/// Extra names tolerated beyond twice the live count before a rebuild.
const REBUILD_SLACK: usize = 1024;

#[derive(Debug, Default)]
pub(crate) struct NamedIndex {
    names: HashSet<String>,
    seeded: bool,
    watermark: usize,
}

/// Whether `name` is a supported Window named-property name. The first query
/// seeds the index from the document tree.
pub fn exists(document: &mut Document, name: &str) -> bool {
    document.named.exists(&document.tree, name)
}

/// Include names in an inserted, connected subtree.
pub(crate) fn inserted(document: &mut Document, node: NodeId) {
    if document.named.seeded && lifecycle::is_connected(document, node) {
        document.named.inserted(&document.tree, node);
    }
}

/// Include a connected element's changed `id` or exposed `name`.
pub(crate) fn attribute_changed(document: &mut Document, node: NodeId, name: &str) {
    if document.named.seeded
        && (name.eq_ignore_ascii_case("id") || name.eq_ignore_ascii_case("name"))
        && lifecycle::is_connected(document, node)
    {
        document.named.inserted(&document.tree, node);
    }
}

impl NamedIndex {
    fn exists(&mut self, tree: &Tree, name: &str) -> bool {
        if !self.seeded {
            self.seeded = true;
            self.walk_named_names(tree, tree.document());
            self.watermark = self.names.len();
        }
        self.names.contains(name)
    }

    fn inserted(&mut self, tree: &Tree, node: NodeId) {
        self.insert_named_names(tree, node);
    }

    /// Adds `root`'s names, rebuilding the index from the document first when
    /// churn has grown it past the live count.
    fn insert_named_names(&mut self, tree: &Tree, root: NodeId) {
        let rebuild_at = self
            .watermark
            .saturating_mul(2)
            .saturating_add(REBUILD_SLACK);
        if self.names.len() >= rebuild_at {
            self.names.clear();
            self.walk_named_names(tree, tree.document());
            self.watermark = self.names.len();
        }
        self.walk_named_names(tree, root);
    }

    /// Adds every non-empty id and exposed `name` in `root`'s inclusive
    /// subtree. Roots enter through the document scan or connected mutations;
    /// names removed later remain harmless until a rebuild.
    fn walk_named_names(&mut self, tree: &Tree, root: NodeId) {
        let mut stack = vec![root];
        while let Some(current) = stack.pop() {
            if let Some(id) = tree.no_namespace_attribute(current, "id")
                && !id.is_empty()
            {
                self.names.insert(id.to_owned());
            }
            let exposes_name = matches!(
                tree.kind(current),
                Some(NodeKind::Element { name, .. })
                    if name.ns == html_namespace()
                        && matches!(name.local.as_ref(), "embed" | "form" | "img" | "object")
            );
            if exposes_name
                && let Some(name) = tree.no_namespace_attribute(current, "name")
                && !name.is_empty()
            {
                self.names.insert(name.to_owned());
            }
            if let Some(children) = tree.children(current) {
                stack.extend(children);
            }
        }
    }
}
