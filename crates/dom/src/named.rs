//! Window named access: the index of supported property names
//! (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#named-access-on-the-window-object>).
//!
//! A global-property miss consults this index to decide whether the name is a
//! named property. The index must never miss a name, so within a rebuild it
//! grows only: a removed name costs one extra value lookup that resolves to
//! nothing, while a name absent from the index would silently break named
//! access. The document is scanned once, on first use, and [`Dom::record`]
//! adds names as mutations arrive.
//!
//! Growth is bounded by rebuilding from the current document when churn
//! (names that no longer exist) grows the set past the live count. A rebuild
//! can only drop stale names; every live name is rescanned, so the index still
//! never misses one.

use crate::{Dom, Mutation, NodeId, NodeKind, html_namespace};

/// Extra names tolerated beyond twice the live count before a rebuild.
const REBUILD_SLACK: usize = 1024;

impl Dom {
    /// Whether `name` is a supported Window named-property name, seeding the
    /// index from the document on first use.
    pub fn named_name_exists(&mut self, name: &str) -> bool {
        if !self.named_names_seeded {
            self.named_names_seeded = true;
            let document = self.document();
            self.walk_named_names(document);
            self.named_names_watermark = self.named_names.len();
        }
        self.named_names.contains(name)
    }

    /// Adds the names a mutation can introduce to the index. Only a child-list
    /// insertion and an `id` or `name` attribute change can.
    pub(crate) fn index_named_names(&mut self, mutation: &Mutation) {
        match mutation {
            Mutation::ChildList { added, .. } => {
                for &node in added {
                    self.insert_named_names(node);
                }
            }
            Mutation::Attributes { target, name, .. }
                if name.eq_ignore_ascii_case("id") || name.eq_ignore_ascii_case("name") =>
            {
                self.insert_named_names(*target);
            }
            Mutation::Attributes { .. } | Mutation::CharacterData { .. } => {}
        }
    }

    /// Adds `root`'s names, rebuilding the index from the document first when
    /// churn has grown it past the live count.
    fn insert_named_names(&mut self, root: NodeId) {
        let rebuild_at = self
            .named_names_watermark
            .saturating_mul(2)
            .saturating_add(REBUILD_SLACK);
        if self.named_names.len() >= rebuild_at {
            self.named_names.clear();
            let document = self.document();
            self.walk_named_names(document);
            self.named_names_watermark = self.named_names.len();
        }
        self.walk_named_names(root);
    }

    /// Adds every non-empty id and exposed `name` in `root`'s inclusive
    /// document subtree. A detached subtree is over-counted until it connects,
    /// which is safe: the value step still only matches connected elements.
    fn walk_named_names(&mut self, root: NodeId) {
        let mut stack = vec![root];
        while let Some(current) = stack.pop() {
            if let Some(id) = self.no_namespace_attribute(current, "id")
                && !id.is_empty()
            {
                self.named_names.insert(id);
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
                self.named_names.insert(name);
            }
            if let Some(children) = self.children(current) {
                stack.extend(children);
            }
        }
    }
}
