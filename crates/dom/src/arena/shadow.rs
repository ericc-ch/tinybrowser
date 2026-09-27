//! Template contents, shadow roots, and slot assignment.

use std::collections::HashMap;

use super::{Document, DomError, NodeId, NodeKind, html_namespace};

#[derive(Debug, Default)]
pub(super) struct ShadowState {
    /// Template elements own a separate contents fragment
    /// (<https://html.spec.whatwg.org/multipage/scripting.html#the-template-element>).
    template_contents: HashMap<NodeId, NodeId>,
    /// A shadow root remains outside the host's child list
    /// (<https://dom.spec.whatwg.org/#concept-shadow-root>).
    shadow_roots: HashMap<NodeId, (NodeId, bool)>,
    shadow_hosts: HashMap<NodeId, NodeId>,
}

impl ShadowState {
    pub(super) fn forget(&mut self, id: NodeId, pending: &mut Vec<NodeId>) {
        if let Some(contents) = self.template_contents.remove(&id) {
            pending.push(contents);
        }
        if let Some((root, _)) = self.shadow_roots.remove(&id) {
            self.shadow_hosts.remove(&root);
            pending.push(root);
        }
        if let Some(host) = self.shadow_hosts.remove(&id) {
            self.shadow_roots.remove(&host);
        }
        self.template_contents.retain(|_, contents| *contents != id);
    }

    pub(super) fn associated_host(&self, id: NodeId) -> Option<NodeId> {
        self.shadow_hosts.get(&id).copied().or_else(|| {
            self.template_contents
                .iter()
                .find_map(|(&host, &contents)| (contents == id).then_some(host))
        })
    }
}

impl Document {
    pub(super) fn is_html_slot(&self, id: NodeId) -> bool {
        matches!(
            self.kind(id),
            Some(NodeKind::Element { name, .. })
                if name.ns == html_namespace() && name.local.as_ref() == "slot"
        )
    }

    fn slottable_name(&self, id: NodeId) -> Option<String> {
        match self.kind(id) {
            Some(NodeKind::Element { .. }) => Some(self.attribute(id, "slot").unwrap_or_default()),
            Some(NodeKind::Text { .. }) => Some(String::new()),
            _ => None,
        }
    }

    fn containing_shadow_root(&self, id: NodeId) -> Option<NodeId> {
        let mut current = Some(id);
        while let Some(node) = current {
            if self.shadow_host(node).is_some() {
                return Some(node);
            }
            current = self.parent(node);
        }
        None
    }

    fn first_slot(&self, root: NodeId, name: &str) -> Option<NodeId> {
        let mut stack: Vec<NodeId> = self
            .children(root)
            .map(Iterator::collect)
            .unwrap_or_default();
        stack.reverse();
        while let Some(node) = stack.pop() {
            if self.is_html_slot(node) && self.attribute(node, "name").unwrap_or_default() == name {
                return Some(node);
            }
            if self.shadow_root(node).is_some() {
                continue;
            }
            if let Some(children) = self.children(node) {
                stack.extend(children.rev());
            }
        }
        None
    }

    pub(super) fn assigned_nodes(&self, slot: NodeId) -> Vec<NodeId> {
        let Some(root) = self.containing_shadow_root(slot) else {
            return Vec::new();
        };
        let Some(host) = self.shadow_host(root) else {
            return Vec::new();
        };
        let name = self.attribute(slot, "name").unwrap_or_default();
        if self.first_slot(root, &name) != Some(slot) {
            return Vec::new();
        }
        self.children(host)
            .into_iter()
            .flatten()
            .filter(|&child| self.slottable_name(child).as_deref() == Some(name.as_str()))
            .collect()
    }

    pub(super) fn assigned_slot(&self, id: NodeId) -> Option<NodeId> {
        let host = self.parent(id)?;
        let root = self.shadow_root(host)?;
        let name = self.slottable_name(id)?;
        let slot = self.first_slot(root, &name)?;
        Some(slot).filter(|&slot| self.assigned_nodes(slot).contains(&id))
    }

    /// Associates `contents` as the [template contents](https://html.spec.whatwg.org/multipage/scripting.html#template-contents)
    /// of `template`. The fragment stays out of `template`'s child list.
    ///
    /// Replacing an existing association destroys the previous fragment.
    ///
    /// # Errors
    ///
    /// - [`DomError::CycleForbidden`] if the association creates a host-including cycle.
    /// - [`DomError::HierarchyRequest`] if replacement would destroy the new contents.
    /// - [`DomError::StaleNode`] if either handle is stale.
    /// - [`DomError::WrongNodeType`] if `template` is not an HTML `template`
    ///   element, `contents` is not a fragment, or `contents` already belongs
    ///   to another template.
    pub fn set_template_contents(
        &mut self,
        template: NodeId,
        contents: NodeId,
    ) -> Result<(), DomError> {
        self.require_live(template)?;
        self.require_live(contents)?;
        if !self.is_html_template_element(template) {
            return Err(DomError::WrongNodeType);
        }
        if !self.is_fragment(contents) {
            return Err(DomError::WrongNodeType);
        }
        if self
            .shadow
            .template_contents
            .iter()
            .any(|(&owner, &mapped)| mapped == contents && owner != template)
        {
            return Err(DomError::WrongNodeType);
        }
        // https://dom.spec.whatwg.org/#concept-tree-host-including-inclusive-ancestor
        if self.would_cycle(contents, template) {
            return Err(DomError::CycleForbidden);
        }
        if let Some(old) = self.shadow.template_contents.get(&template).copied()
            && old != contents
        {
            if self.would_cycle(old, contents) {
                return Err(DomError::HierarchyRequest);
            }
            self.destroy(old)?;
            self.shadow.template_contents.remove(&template);
        }
        self.shadow.template_contents.insert(template, contents);
        Ok(())
    }

    /// The contents fragment of `template`, if this document associated one.
    #[must_use]
    pub fn template_contents(&self, template: NodeId) -> Option<NodeId> {
        let contents = self.shadow.template_contents.get(&template).copied()?;
        self.contains(contents).then_some(contents)
    }

    /// Attaches a new shadow root to `host`.
    ///
    /// Implements the tree association from the DOM "attach a shadow root"
    /// algorithm; policy checks such as the HTML element safelist remain at
    /// the binding boundary.
    /// <https://dom.spec.whatwg.org/#concept-attach-a-shadow-root>
    ///
    /// # Errors
    ///
    /// Returns [`DomError::HierarchyRequest`] if the host already has a
    /// shadow root, and [`DomError::WrongNodeType`] for a non-element host.
    pub fn attach_shadow(&mut self, host: NodeId, open: bool) -> Result<NodeId, DomError> {
        self.require_live(host)?;
        if !matches!(self.kind(host), Some(NodeKind::Element { .. })) {
            return Err(DomError::WrongNodeType);
        }
        if self.shadow.shadow_roots.contains_key(&host) {
            return Err(DomError::HierarchyRequest);
        }
        let root = self.create_fragment();
        self.shadow.shadow_roots.insert(host, (root, open));
        self.shadow.shadow_hosts.insert(root, host);
        self.journal.bump();
        Ok(root)
    }

    /// The shadow root associated with `host`, including a closed root.
    #[must_use]
    pub fn shadow_root(&self, host: NodeId) -> Option<NodeId> {
        let (root, _) = self.shadow.shadow_roots.get(&host).copied()?;
        self.contains(root).then_some(root)
    }

    /// The open shadow root associated with `host`.
    #[must_use]
    pub fn open_shadow_root(&self, host: NodeId) -> Option<NodeId> {
        let (root, open) = self.shadow.shadow_roots.get(&host).copied()?;
        (open && self.contains(root)).then_some(root)
    }

    /// The host of a shadow-root fragment.
    #[must_use]
    pub fn shadow_host(&self, root: NodeId) -> Option<NodeId> {
        let host = self.shadow.shadow_hosts.get(&root).copied()?;
        self.contains(host).then_some(host)
    }

    /// Whether `root` is an open shadow root.
    #[must_use]
    pub fn shadow_root_is_open(&self, root: NodeId) -> Option<bool> {
        let host = self.shadow_host(root)?;
        self.shadow
            .shadow_roots
            .get(&host)
            .and_then(|&(candidate, open)| (candidate == root).then_some(open))
    }
}
