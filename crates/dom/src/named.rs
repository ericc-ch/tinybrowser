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

use crate::{Dom, NodeId, NodeKind, html_namespace};

/// Extra names tolerated beyond twice the live count before a rebuild.
const REBUILD_SLACK: usize = 1024;

#[derive(Debug, Default)]
pub(crate) struct NamedIndex {
    names: HashSet<String>,
    seeded: bool,
    watermark: usize,
}

impl Dom {
    /// Whether `name` is a supported Window named-property name, seeding the
    /// index from the document on first use.
    pub fn named_name_exists(&mut self, name: &str) -> bool {
        if !self.named.seeded {
            self.named.seeded = true;
            let document = self.document();
            self.walk_named_names(document);
            self.named.watermark = self.named.names.len();
        }
        self.named.names.contains(name)
    }

    pub(crate) fn index_inserted_names(&mut self, node: NodeId) {
        if self.named.seeded && self.is_connected(node) {
            self.insert_named_names(node);
        }
    }

    pub(crate) fn index_changed_name(&mut self, node: NodeId, name: &str) {
        if self.named.seeded
            && self.is_connected(node)
            && (name.eq_ignore_ascii_case("id") || name.eq_ignore_ascii_case("name"))
        {
            self.insert_named_names(node);
        }
    }

    /// Adds `root`'s names, rebuilding the index from the document first when
    /// churn has grown it past the live count.
    fn insert_named_names(&mut self, root: NodeId) {
        let rebuild_at = self
            .named
            .watermark
            .saturating_mul(2)
            .saturating_add(REBUILD_SLACK);
        if self.named.names.len() >= rebuild_at {
            self.named.names.clear();
            let document = self.document();
            self.walk_named_names(document);
            self.named.watermark = self.named.names.len();
        }
        self.walk_named_names(root);
    }

    /// Adds every non-empty id and exposed `name` in `root`'s inclusive
    /// subtree. Roots enter through the document scan or connected mutations;
    /// names removed later remain harmless until a rebuild.
    fn walk_named_names(&mut self, root: NodeId) {
        let mut stack = vec![root];
        while let Some(current) = stack.pop() {
            if let Some(id) = self.no_namespace_attribute(current, "id")
                && !id.is_empty()
            {
                self.named.names.insert(id);
            }
            let exposes_name = matches!(
                self.kind(current),
                Some(NodeKind::Element { name, .. })
                    if name.ns == html_namespace()
                        && matches!(name.local.as_ref(), "embed" | "form" | "img" | "object")
            );
            if exposes_name
                && let Some(name) = self.no_namespace_attribute(current, "name")
                && !name.is_empty()
            {
                self.named.names.insert(name);
            }
            if let Some(children) = self.children(current) {
                stack.extend(children);
            }
        }
    }
}
