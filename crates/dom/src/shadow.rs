//! Template contents, shadow roots, and slot assignment.

use std::collections::HashMap;

use crate::mutation;
use crate::{Document, DomError, NodeId, NodeKind, html_namespace};

#[derive(Debug, Default)]
pub(crate) struct ShadowState {
    /// Template elements own a separate contents fragment
    /// (<https://html.spec.whatwg.org/multipage/scripting.html#the-template-element>).
    template_contents: HashMap<NodeId, NodeId>,
    /// A shadow root remains outside the host's child list
    /// (<https://dom.spec.whatwg.org/#concept-shadow-root>).
    shadow_roots: HashMap<NodeId, (NodeId, bool)>,
    shadow_hosts: HashMap<NodeId, NodeId>,
}

impl ShadowState {
    pub(crate) fn forget(&mut self, id: NodeId, pending: &mut Vec<NodeId>) {
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

    pub(crate) fn associated_host(&self, id: NodeId) -> Option<NodeId> {
        self.shadow_hosts.get(&id).copied().or_else(|| {
            self.template_contents
                .iter()
                .find_map(|(&host, &contents)| (contents == id).then_some(host))
        })
    }
}

fn is_html_template_element(document: &Document, id: NodeId) -> bool {
    match document.kind(id) {
        Some(NodeKind::Element { name, .. }) => {
            name.ns == html_namespace() && name.local.as_ref().eq_ignore_ascii_case("template")
        }
        _ => false,
    }
}

pub(crate) fn is_html_slot(document: &Document, id: NodeId) -> bool {
    matches!(
        document.kind(id),
        Some(NodeKind::Element { name, .. })
            if name.ns == html_namespace() && name.local.as_ref() == "slot"
    )
}

fn slottable_name(document: &Document, id: NodeId) -> Option<String> {
    match document.kind(id) {
        Some(NodeKind::Element { .. }) => Some(document.attribute(id, "slot").unwrap_or_default()),
        Some(NodeKind::Text { .. }) => Some(String::new()),
        _ => None,
    }
}

fn containing_shadow_root(document: &Document, id: NodeId) -> Option<NodeId> {
    let mut current = Some(id);
    while let Some(node) = current {
        if shadow_host(document, node).is_some() {
            return Some(node);
        }
        current = document.parent(node);
    }
    None
}

fn first_slot(document: &Document, root: NodeId, name: &str) -> Option<NodeId> {
    let mut stack: Vec<NodeId> = document
        .children(root)
        .map(Iterator::collect)
        .unwrap_or_default();
    stack.reverse();
    while let Some(node) = stack.pop() {
        if is_html_slot(document, node)
            && document.attribute(node, "name").unwrap_or_default() == name
        {
            return Some(node);
        }
        if shadow_root(document, node).is_some() {
            continue;
        }
        if let Some(children) = document.children(node) {
            stack.extend(children.rev());
        }
    }
    None
}

pub(crate) fn assigned_nodes(document: &Document, slot: NodeId) -> Vec<NodeId> {
    let Some(root) = containing_shadow_root(document, slot) else {
        return Vec::new();
    };
    let Some(host) = shadow_host(document, root) else {
        return Vec::new();
    };
    let name = document.attribute(slot, "name").unwrap_or_default();
    if first_slot(document, root, &name) != Some(slot) {
        return Vec::new();
    }
    document
        .children(host)
        .into_iter()
        .flatten()
        .filter(|&child| slottable_name(document, child).as_deref() == Some(name.as_str()))
        .collect()
}

pub(crate) fn assigned_slot(document: &Document, id: NodeId) -> Option<NodeId> {
    let host = document.parent(id)?;
    let root = shadow_root(document, host)?;
    let name = slottable_name(document, id)?;
    let slot = first_slot(document, root, &name)?;
    Some(slot).filter(|&slot| assigned_nodes(document, slot).contains(&id))
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
    document: &mut Document,
    template: NodeId,
    contents: NodeId,
) -> Result<(), DomError> {
    document.require_live(template)?;
    document.require_live(contents)?;
    if !is_html_template_element(document, template) {
        return Err(DomError::WrongNodeType);
    }
    if !document.is_fragment(contents) {
        return Err(DomError::WrongNodeType);
    }
    if document
        .shadow
        .template_contents
        .iter()
        .any(|(&owner, &mapped)| mapped == contents && owner != template)
    {
        return Err(DomError::WrongNodeType);
    }
    // https://dom.spec.whatwg.org/#concept-tree-host-including-inclusive-ancestor
    if document.would_cycle(contents, template) {
        return Err(DomError::CycleForbidden);
    }
    if let Some(old) = document.shadow.template_contents.get(&template).copied()
        && old != contents
    {
        if document.would_cycle(old, contents) {
            return Err(DomError::HierarchyRequest);
        }
        document.destroy(old)?;
        document.shadow.template_contents.remove(&template);
    }
    document.shadow.template_contents.insert(template, contents);
    Ok(())
}

/// The contents fragment of `template`, if this document associated one.
#[must_use]
pub fn template_contents(document: &Document, template: NodeId) -> Option<NodeId> {
    let contents = document.shadow.template_contents.get(&template).copied()?;
    document.contains(contents).then_some(contents)
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
pub fn attach_shadow(document: &mut Document, host: NodeId, open: bool) -> Result<NodeId, DomError> {
    document.require_live(host)?;
    if !matches!(document.kind(host), Some(NodeKind::Element { .. })) {
        return Err(DomError::WrongNodeType);
    }
    if document.shadow.shadow_roots.contains_key(&host) {
        return Err(DomError::HierarchyRequest);
    }
    let root = document.create_fragment();
    document.shadow.shadow_roots.insert(host, (root, open));
    document.shadow.shadow_hosts.insert(root, host);
    mutation::bump(document);
    Ok(root)
}

/// The shadow root associated with `host`, including a closed root.
#[must_use]
pub fn shadow_root(document: &Document, host: NodeId) -> Option<NodeId> {
    let (root, _) = document.shadow.shadow_roots.get(&host).copied()?;
    document.contains(root).then_some(root)
}

/// The open shadow root associated with `host`.
#[must_use]
pub fn open_shadow_root(document: &Document, host: NodeId) -> Option<NodeId> {
    let (root, open) = document.shadow.shadow_roots.get(&host).copied()?;
    (open && document.contains(root)).then_some(root)
}

/// The host of a shadow-root fragment.
#[must_use]
pub fn shadow_host(document: &Document, root: NodeId) -> Option<NodeId> {
    let host = document.shadow.shadow_hosts.get(&root).copied()?;
    document.contains(host).then_some(host)
}

/// Whether `root` is an open shadow root.
#[must_use]
pub fn shadow_root_is_open(document: &Document, root: NodeId) -> Option<bool> {
    let host = shadow_host(document, root)?;
    document
        .shadow
        .shadow_roots
        .get(&host)
        .and_then(|&(candidate, open)| (candidate == root).then_some(open))
}
