//! Element attribute mutation workflows.
//!
//! Each setter validates the element, updates the attribute list, and queues
//! the observer record. Name-driven reactions follow the change: named-access
//! indexing for every attribute change, plus the form control steps for the
//! unnamespaced set/remove paths. `add_attrs_if_missing` merges parser
//! attributes without queuing a record.

use crate::form;
use crate::mutation::{self, Mutation};
use crate::named;
use crate::{
    Attribute, Document, DomError, LocalName, Namespace, NodeId, NodeKind, Prefix, QualName,
    html_namespace, html_qualified_name_eq, qualified_name_eq,
};

/// Mutable element name and attribute list for the attribute mutators.
fn element_mut(
    document: &mut Document,
    id: NodeId,
) -> Result<(&QualName, &mut Vec<Attribute>), DomError> {
    match document.tree.kind_mut(id).ok_or(DomError::StaleNode)? {
        NodeKind::Element { name, attributes } => Ok((name, attributes)),
        _ => Err(DomError::WrongNodeType),
    }
}

fn attr_query_eq(element_ns: &Namespace, name: &QualName, query: &str) -> bool {
    if *element_ns == html_namespace() {
        html_qualified_name_eq(name, query)
    } else {
        qualified_name_eq(name, query)
    }
}

/// The first attribute on `id` whose **qualified name** matches `local`
/// under the element's case regime; HTML elements ASCII-lowercase the
/// queried name first
/// ([DOM get-an-attribute-by-name](https://dom.spec.whatwg.org/#concept-element-attributes-get-by-name)).
pub(crate) fn find_attribute<'a>(
    document: &'a Document,
    id: NodeId,
    local: &str,
) -> Option<&'a Attribute> {
    let (name, attributes) = document.element(id)?;
    attributes
        .iter()
        .find(|attribute| attr_query_eq(&name.ns, &attribute.name, local))
}

/// Adds each attribute that `id` does not already carry, matched by
/// qualified name.
///
/// The adapter's `add_attrs_if_missing` landing pad: html5ever merges
/// attributes from repeated start-tag tokens through this call.
///
/// # Errors
///
/// - [`DomError::StaleNode`] if `id` is stale.
/// - [`DomError::WrongNodeType`] if `id` is not an element.
pub fn add_attrs_if_missing(
    document: &mut Document,
    id: NodeId,
    attrs: Vec<Attribute>,
) -> Result<(), DomError> {
    let (_, attributes) = element_mut(document, id)?;
    let added_name = attrs
        .iter()
        .find(|attribute| {
            attribute.name.ns.as_ref().is_empty()
                && matches!(attribute.name.local.as_ref(), "id" | "name")
                && !attributes
                    .iter()
                    .any(|existing| existing.name == attribute.name)
        })
        .map(|attribute| attribute.name.local.to_string());
    merge_attrs(attributes, attrs);
    if let Some(name) = added_name {
        named::attribute_changed(document, id, &name);
    }
    Ok(())
}

/// Sets an attribute identified by namespace and local name, replacing
/// the first attribute with that namespace and local name (the existing
/// prefix is kept, matching "set an attribute value").
///
/// [DOM setAttributeNS](https://dom.spec.whatwg.org/#dom-element-setattributens)
/// and `setAttributeNode` land here.
///
/// # Errors
///
/// - [`DomError::StaleNode`] if `id` is stale.
/// - [`DomError::WrongNodeType`] if `id` is not an element.
pub fn set_attribute_by_ns(
    document: &mut Document,
    id: NodeId,
    namespace: &str,
    prefix: Option<&str>,
    local: &str,
    value: impl Into<String>,
) -> Result<(), DomError> {
    let value = value.into();
    let (_, attributes) = element_mut(document, id)?;
    let index = attributes.iter().position(|attribute| {
        attribute.name.ns.as_ref() == namespace && attribute.name.local.as_ref() == local
    });
    let old_value = if let Some(index) = index {
        Some(std::mem::replace(&mut attributes[index].value, value))
    } else {
        attributes.push(Attribute {
            name: QualName::new(
                prefix.map(Prefix::from),
                Namespace::from(namespace),
                LocalName::from(local),
            ),
            value,
        });
        None
    };
    mutation::record(
        document,
        Mutation::Attributes {
            target: id,
            name: local.to_owned(),
            namespace: namespace.to_owned(),
            old_value,
        },
    );
    named::attribute_changed(document, id, local);
    // DOM's "set an attribute value" runs the attribute change steps
    // regardless of entry API (`setAttributeNS(null, …)`, `setAttributeNode`,
    // `Attr.value`); the form steps key off no-namespace attributes, so only
    // the empty-namespace path runs them
    // (<https://dom.spec.whatwg.org/#concept-element-attributes-set>).
    if namespace.is_empty() {
        form::attribute_set(document, id, local)?;
    }
    Ok(())
}

/// Sets the unnamespaced attribute `local` on element `id`, replacing a
/// same-name attribute if one exists.
///
/// [DOM setAttribute](https://dom.spec.whatwg.org/#dom-element-setattribute)
///
/// # Errors
///
/// - [`DomError::StaleNode`] if `id` is stale.
/// - [`DomError::WrongNodeType`] if `id` is not an element.
pub fn set_attribute(
    document: &mut Document,
    id: NodeId,
    local: &str,
    value: impl Into<String>,
) -> Result<(), DomError> {
    let value = value.into();
    let (name, attributes) = element_mut(document, id)?;
    let html = name.ns == html_namespace();
    let existing = attributes
        .iter()
        .position(|attribute| attr_query_eq(&name.ns, &attribute.name, local));
    // A matched attribute can carry a namespace even though the query is
    // unnamespaced (`setAttribute("xlink:href", …)` on an SVG element);
    // the record reports the changed attribute's real name and namespace
    // (<https://dom.spec.whatwg.org/#dom-mutationrecord-attributename>).
    let (recorded_name, recorded_namespace, old_value) = if let Some(index) = existing {
        let attribute = &mut attributes[index];
        let old_value = std::mem::replace(&mut attribute.value, value);
        (
            attribute.name.local.to_string(),
            attribute.name.ns.to_string(),
            Some(old_value),
        )
    } else {
        let local = if html {
            local.to_ascii_lowercase()
        } else {
            local.to_owned()
        };
        attributes.push(Attribute {
            name: QualName::new(None, Namespace::from(""), LocalName::from(local.as_str())),
            value,
        });
        (local, String::new(), None)
    };
    mutation::record(
        document,
        Mutation::Attributes {
            target: id,
            name: recorded_name,
            namespace: recorded_namespace,
            old_value,
        },
    );
    named::attribute_changed(document, id, local);
    form::attribute_set(document, id, local)
}

/// [Element.removeAttribute](https://dom.spec.whatwg.org/#dom-element-removeattribute).
///
/// # Errors
///
/// - [`DomError::StaleNode`] if `id` is stale.
/// - [`DomError::WrongNodeType`] if `id` is not an element.
pub fn remove_attribute(document: &mut Document, id: NodeId, local: &str) -> Result<(), DomError> {
    let (name, attributes) = element_mut(document, id)?;
    let Some(index) = attributes
        .iter()
        .position(|attribute| attr_query_eq(&name.ns, &attribute.name, local))
    else {
        return Ok(());
    };
    let removed = attributes.remove(index);
    // `MutationRecord.attributeName` is the attribute's local
    // name and `attributeNamespace` its namespace, not the
    // queried qualified name
    // (<https://dom.spec.whatwg.org/#dom-mutationrecord-attributename>).
    mutation::record(
        document,
        Mutation::Attributes {
            target: id,
            name: removed.name.local.to_string(),
            namespace: removed.name.ns.to_string(),
            old_value: Some(removed.value),
        },
    );
    named::attribute_changed(document, id, local);
    form::attribute_removed(document, id, local)
}

/// [Element.removeAttributeNS](https://dom.spec.whatwg.org/#dom-element-removeattributens).
///
/// # Errors
///
/// - [`DomError::StaleNode`] if `id` is stale.
/// - [`DomError::WrongNodeType`] if `id` is not an element.
pub fn remove_attribute_ns(
    document: &mut Document,
    id: NodeId,
    ns: &str,
    local: &str,
) -> Result<(), DomError> {
    let (_, attributes) = element_mut(document, id)?;
    let Some(index) = attributes.iter().position(|attribute| {
        attribute.name.ns.as_ref() == ns && attribute.name.local.as_ref() == local
    }) else {
        return Ok(());
    };
    let removed = attributes.remove(index);
    mutation::record(
        document,
        Mutation::Attributes {
            target: id,
            name: local.to_owned(),
            namespace: ns.to_owned(),
            old_value: Some(removed.value),
        },
    );
    named::attribute_changed(document, id, local);
    // Same entry-API independence as the setter: `removeAttributeNS(null, …)`
    // and `removeAttributeNode` run the form steps for no-namespace names.
    if ns.is_empty() {
        form::attribute_removed(document, id, local)?;
    }
    Ok(())
}

/// Adds each attribute whose qualified name is not already present; the
/// first occurrence wins, matching the DOM's name-keyed attribute list
/// (<https://dom.spec.whatwg.org/#concept-attribute>).
pub(crate) fn merge_attrs(
    attributes: &mut Vec<Attribute>,
    attrs: impl IntoIterator<Item = Attribute>,
) {
    for attr in attrs {
        if !attributes.iter().any(|existing| existing.name == attr.name) {
            attributes.push(attr);
        }
    }
}
