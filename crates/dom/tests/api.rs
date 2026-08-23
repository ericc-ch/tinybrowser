//! Public-API behavior of the dom layer.
//!
//! These tests go through `Dom`'s methods only — no private internals — so
//! they double as executable documentation of the boundary contract.

use dom::{Attribute, Dom, DomError, LocalName, Namespace, NodeKind, QualName};

/// The HTML namespace URL; `markup5ever`'s `ns!` macro wraps this same string.
const HTML_NS: &str = "http://www.w3.org/1999/xhtml";

/// HTML-namespace qualified name with no prefix.
fn qn(local: &str) -> QualName {
    QualName::new(None, Namespace::from(HTML_NS), LocalName::from(local))
}

#[test]
fn fresh_document_has_root_and_nothing_else() {
    let d = Dom::new();
    let doc = d.document();
    assert!(d.contains(doc));
    assert_eq!(d.children(doc).unwrap().count(), 0);
    assert_eq!(d.parent(doc), None);
}

#[test]
fn append_builds_chain_in_order() {
    let mut d = Dom::new();
    let doc = d.document();
    let p = d.create_element(qn("p"), Vec::new());
    let text = d.create_text("hello");

    d.append(doc, p).unwrap();
    d.append(p, text).unwrap();

    let top: Vec<_> = d.children(doc).unwrap().copied().collect();
    assert_eq!(top, vec![p]);
    assert_eq!(d.parent(p), Some(doc));
    assert_eq!(d.parent(text), Some(p));
    assert_eq!(
        d.children(p).unwrap().copied().collect::<Vec<_>>(),
        vec![text]
    );
}

#[test]
fn append_moves_already_attached_nodes() {
    let mut d = Dom::new();
    let doc = d.document();
    let a = d.create_element(qn("a"), Vec::new());
    let b = d.create_element(qn("b"), Vec::new());

    d.append(a, b).unwrap();
    d.append(doc, b).unwrap(); // moves b from under a to under doc

    assert_eq!(d.children(a).unwrap().count(), 0);
    assert_eq!(d.parent(b), Some(doc));
}

#[test]
fn appending_into_own_subtree_is_refused() {
    let mut d = Dom::new();
    let doc = d.document();
    let outer = d.create_element(qn("outer"), Vec::new());
    let inner = d.create_element(qn("inner"), Vec::new());
    d.append(doc, outer).unwrap();
    d.append(outer, inner).unwrap();

    // self-append and ancestor-append both refused
    assert_eq!(d.append(outer, outer), Err(DomError::CycleForbidden));
    assert_eq!(d.append(inner, outer), Err(DomError::CycleForbidden));
}

#[test]
fn insert_before_positions_exactly() {
    let mut d = Dom::new();
    let doc = d.document();
    let mk = |d: &mut Dom| d.create_element(qn("li"), Vec::new());
    let (a, b, c) = (mk(&mut d), mk(&mut d), mk(&mut d));

    d.append(doc, a).unwrap();
    d.append(doc, c).unwrap();
    d.insert_before(c, b).unwrap();

    assert_eq!(
        d.children(doc).unwrap().copied().collect::<Vec<_>>(),
        vec![a, b, c]
    );
}

#[test]
fn insert_before_head_and_document_are_handled() {
    let mut d = Dom::new();
    let doc = d.document();
    let x = d.create_element(qn("x"), Vec::new());
    let y = d.create_element(qn("y"), Vec::new());

    // inserting before the document root is not a thing
    assert_eq!(d.insert_before(doc, x), Err(DomError::IllegalTarget));

    d.append(doc, x).unwrap();
    d.insert_before(x, y).unwrap();
    assert_eq!(
        d.children(doc).unwrap().copied().collect::<Vec<_>>(),
        vec![y, x]
    );
}

#[test]
fn detach_unlinks_but_keeps_subtree_alive() {
    let mut d = Dom::new();
    let doc = d.document();
    let parent = d.create_element(qn("ul"), Vec::new());
    let item = d.create_text("item");
    d.append(doc, parent).unwrap();
    d.append(parent, item).unwrap();

    d.detach(parent).unwrap();

    assert_eq!(d.parent(parent), None);
    assert!(d.contains(parent));
    assert!(d.contains(item)); // subtree survives intact
    assert_eq!(d.parent(item), Some(parent));
    assert_eq!(d.children(doc).unwrap().count(), 0);

    // idempotent
    d.detach(parent).unwrap();

    // but the document root itself cannot be detached
    assert_eq!(d.detach(doc), Err(DomError::IllegalTarget));
}

#[test]
fn reparent_children_moves_everything_in_order() {
    let mut d = Dom::new();
    let doc = d.document();
    let from = d.create_element(qn("from"), Vec::new());
    let to = d.create_element(qn("to"), Vec::new());
    d.append(doc, from).unwrap();
    d.append(doc, to).unwrap();

    let kids: Vec<_> = (0..6)
        .map(|_| d.create_element(qn("span"), Vec::new()))
        .collect();
    for &k in &kids {
        d.append(from, k).unwrap();
    }

    d.reparent_children(from, to).unwrap();

    assert_eq!(d.children(from).unwrap().count(), 0);
    assert_eq!(d.children(to).unwrap().copied().collect::<Vec<_>>(), kids);
    assert!(kids.iter().all(|&k| d.parent(k) == Some(to)));
}

#[test]
fn reparent_children_into_own_subtree_is_refused() {
    let mut d = Dom::new();
    let doc = d.document();
    let outer = d.create_element(qn("outer"), Vec::new());
    let inner = d.create_element(qn("inner"), Vec::new());
    d.append(doc, outer).unwrap();
    d.append(outer, inner).unwrap();

    assert_eq!(
        d.reparent_children(outer, inner),
        Err(DomError::CycleForbidden)
    );
}

#[test]
fn destroy_recycles_slots_and_stales_every_handle_inside() {
    let mut d = Dom::new();
    let doc = d.document();
    let parent = d.create_element(qn("section"), Vec::new());
    let child = d.create_text("child");
    d.append(doc, parent).unwrap();
    d.append(parent, child).unwrap();

    d.destroy(parent).unwrap();

    assert!(!d.contains(parent));
    assert!(!d.contains(child)); // whole subtree went stale together
    assert!(d.get(parent).is_none());
    assert_eq!(d.parent(child), None);
    assert_eq!(d.children(doc).unwrap().count(), 0);
    assert_eq!(d.destroy(parent), Err(DomError::StaleNode));
}

#[test]
fn recycled_slots_never_impersonate_dead_nodes() {
    // The button/span scenario from wayfinding: create, destroy, reuse —
    // the old handle must miss even though the slot number matches.
    let mut d = Dom::new();
    let doc = d.document();
    let button = d.create_element(qn("button"), Vec::new());
    d.append(doc, button).unwrap();

    d.destroy(button).unwrap();
    let span = d.create_element(qn("span"), Vec::new());

    // fresh creation takes back the freed slot number under a new
    // generation; only the behavior below is public, not the slot identity
    assert!(!d.contains(button), "old handle must not name the new node");
    assert!(d.contains(span));
    assert_eq!(d.parent(span), None); // created unattached
    d.append(doc, span).unwrap();
    assert!(matches!(
        d.get(span).map(|n| n.kind()),
        Some(NodeKind::Element { .. })
    ));
}

#[test]
fn stale_handles_error_on_mutation() {
    let mut d = Dom::new();
    let ghost = d.create_element(qn("ghost"), Vec::new());
    d.destroy(ghost).unwrap();

    let live = d.create_element(qn("live"), Vec::new());
    assert_eq!(d.append(ghost, live), Err(DomError::StaleNode));
    assert_eq!(d.append(live, ghost), Err(DomError::StaleNode));
    assert_eq!(d.set_text(ghost, "boo"), Err(DomError::StaleNode));
}

#[test]
fn set_data_targets_only_its_kind() {
    let mut d = Dom::new();
    let text = d.create_text("one");
    d.set_text(text, "two").unwrap();

    let comment = d.create_comment("note");
    d.set_comment(comment, "annotated").unwrap();

    let element = d.create_element(qn("b"), Vec::new());
    assert_eq!(d.set_text(element, "nope"), Err(DomError::IllegalTarget));
    assert_eq!(d.set_comment(element, "nope"), Err(DomError::IllegalTarget));

    match d.get(comment).map(|n| n.kind()) {
        Some(NodeKind::Comment { data }) => assert_eq!(data, "annotated"),
        other => panic!("expected comment kind, got {other:?}"),
    }
}

#[test]
fn add_attrs_if_missing_merges_without_duplicating() {
    let mut d = Dom::new();
    let el = d.create_element(
        qn("input"),
        vec![Attribute {
            name: qn("type"),
            value: "text".into(),
        }],
    );
    d.append(d.document(), el).unwrap();

    d.add_attrs_if_missing(
        el,
        vec![
            Attribute {
                name: qn("type"),
                value: "checkbox".into(),
            },
            Attribute {
                name: qn("name"),
                value: "q".into(),
            },
        ],
    )
    .unwrap();

    match d.get(el).map(|n| n.kind()) {
        Some(NodeKind::Element { attributes, .. }) => {
            assert_eq!(attributes.len(), 2, "existing name wins, new one lands");
            assert_eq!(attributes[0].value, "text");
            assert_eq!(attributes[1].name, qn("name"));
        }
        other => panic!("expected element kind, got {other:?}"),
    }

    // only elements take attributes
    let text = d.create_text("t");
    assert_eq!(
        d.add_attrs_if_missing(text, Vec::new()),
        Err(DomError::IllegalTarget)
    );

    // stale handles are refused like every other mutation
    let ghost = d.create_element(qn("gone"), Vec::new());
    d.destroy(ghost).unwrap();
    assert_eq!(
        d.add_attrs_if_missing(ghost, Vec::new()),
        Err(DomError::StaleNode)
    );
}

#[test]
fn doctype_carries_its_ids() {
    let mut d = Dom::new();
    let dt = d.create_doctype(
        "html",
        "-//W3C//DTD HTML 4.01//EN",
        "http://www.w3.org/tr/html4/strict.dtd",
    );
    match d.get(dt).map(|n| n.kind()) {
        Some(NodeKind::Doctype {
            name,
            public_id,
            system_id,
        }) => {
            assert_eq!(name, "html");
            assert_eq!(public_id, "-//W3C//DTD HTML 4.01//EN");
            assert_eq!(system_id, "http://www.w3.org/tr/html4/strict.dtd");
        }
        other => panic!("expected doctype kind, got {other:?}"),
    }
}

#[test]
fn elements_carry_names_and_attributes() {
    let mut d = Dom::new();
    let class = qn("class");
    let attrs = vec![Attribute {
        name: class.clone(),
        value: "menu".into(),
    }];
    let el = d.create_element(qn("nav"), attrs);

    match d.get(el).map(|n| n.kind()) {
        Some(NodeKind::Element { name, attributes }) => {
            assert_eq!(&*name.local, "nav");
            assert_eq!(name.ns, Namespace::from(HTML_NS));
            assert!(name.prefix.is_none());
            assert_eq!(attributes.len(), 1);
            assert_eq!(attributes[0].name, class);
            assert_eq!(attributes[0].value, "menu");
        }
        other => panic!("expected element kind, got {other:?}"),
    }
}

#[test]
fn insert_before_edge_cases_are_refused_correctly() {
    let mut d = Dom::new();
    let doc = d.document();
    let outer = d.create_element(qn("outer"), Vec::new());
    let inner = d.create_element(qn("inner"), Vec::new());
    d.append(doc, outer).unwrap();
    d.append(outer, inner).unwrap();

    // inserting a node before itself is meaningless, not a reorder
    assert_eq!(d.insert_before(inner, inner), Err(DomError::IllegalTarget));

    // moving `outer` to before its own descendant would tear the subtree
    assert_eq!(d.insert_before(inner, outer), Err(DomError::CycleForbidden));

    // state unchanged by all refusals
    assert_eq!(
        d.children(doc).unwrap().copied().collect::<Vec<_>>(),
        vec![outer]
    );
    assert_eq!(
        d.children(outer).unwrap().copied().collect::<Vec<_>>(),
        vec![inner]
    );
}

#[test]
fn reparent_children_to_itself_is_a_clean_noop() {
    let mut d = Dom::new();
    let doc = d.document();
    let parent = d.create_element(qn("p"), Vec::new());
    let a = d.create_element(qn("a"), Vec::new());
    let b = d.create_element(qn("b"), Vec::new());
    d.append(doc, parent).unwrap();
    d.append(parent, a).unwrap();
    d.append(parent, b).unwrap();

    d.reparent_children(parent, parent).unwrap();

    assert_eq!(
        d.children(parent).unwrap().copied().collect::<Vec<_>>(),
        vec![a, b]
    );
    assert_eq!(d.parent(a), Some(parent));
}

#[test]
fn every_mutation_rejects_stale_handles() {
    let mut d = Dom::new();
    let doc = d.document();
    let live = d.create_element(qn("live"), Vec::new());
    let ghost = d.create_text("ghost");
    let sibling = d.create_element(qn("s"), Vec::new());
    d.append(doc, live).unwrap();
    d.append(live, sibling).unwrap();
    d.destroy(ghost).unwrap();

    assert_eq!(d.insert_before(sibling, ghost), Err(DomError::StaleNode));
    assert_eq!(d.insert_before(ghost, live), Err(DomError::StaleNode));
    assert_eq!(d.detach(ghost), Err(DomError::StaleNode));
    assert_eq!(d.destroy(ghost), Err(DomError::StaleNode));
    assert_eq!(d.reparent_children(ghost, doc), Err(DomError::StaleNode));
    assert_eq!(d.reparent_children(doc, ghost), Err(DomError::StaleNode));
    assert_eq!(d.set_comment(ghost, "boo"), Err(DomError::StaleNode));
}

#[test]
fn the_document_root_cannot_gain_a_parent_or_be_drained() {
    let mut d = Dom::new();
    let doc = d.document();
    let stray = d.create_element(qn("stray"), Vec::new());
    let body = d.create_element(qn("body"), Vec::new());
    d.append(doc, body).unwrap();

    // the root must never gain a parent (orphaned document)
    assert_eq!(d.append(stray, doc), Err(DomError::IllegalTarget));

    // nor may its content drain into an arbitrary detached subtree
    assert_eq!(
        d.reparent_children(doc, stray),
        Err(DomError::IllegalTarget)
    );

    // refusals left state untouched
    assert_eq!(d.parent(doc), None);
    assert_eq!(
        d.children(doc).unwrap().copied().collect::<Vec<_>>(),
        vec![body]
    );
}

#[test]
fn stale_children_read_reports_none_not_childless() {
    let mut d = Dom::new();
    let ghost = d.create_element(qn("ghost"), Vec::new());
    d.destroy(ghost).unwrap();
    assert!(d.children(ghost).is_none());
}

#[test]
fn dom_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<Dom>();
    // `!Sync` is enforced structurally by `_share_forbidden: PhantomData<Cell<()>>`
    // (Cell<()> is !Sync by definition). Stable Rust cannot assert negative
    // bounds in tests; deleting that field is the only way this guarantee can
    // rot, and the field's own doc comment says exactly that.
}

#[test]
fn wide_children_lists_behave_like_any_list() {
    let mut d = Dom::new();
    let doc = d.document();
    let wide = d.create_element(qn("wide"), Vec::new());
    d.append(doc, wide).unwrap();

    // far past inline capacity
    let kids: Vec<_> = (0..50).map(|i| d.create_text(format!("{i}"))).collect();
    for &k in &kids {
        d.append(wide, k).unwrap();
    }
    assert_eq!(d.children(wide).unwrap().copied().collect::<Vec<_>>(), kids);

    // removals from the middle keep order stable
    let middle = kids[25];
    d.destroy(middle).unwrap();
    let expected: Vec<_> = kids.iter().copied().filter(|&k| k != middle).collect();
    assert_eq!(d.children(wide).unwrap().copied().collect::<Vec<_>>(),
        expected
    );
}

#[test]
fn fragments_are_detached_containers() {
    let mut d = Dom::new();
    let f = d.create_fragment();
    let t = d.create_text("hi");
    d.append(f, t).unwrap();

    assert!(matches!(d.get(f).map(|n| n.kind()), Some(NodeKind::Fragment)));
    // outside the main tree: no parent, but fully live and usable
    assert_eq!(d.parent(f), None);
    assert_eq!(d.children(f).unwrap().copied().collect::<Vec<_>>(), vec![t]);
}
