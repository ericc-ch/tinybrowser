//! Public-API behavior of the dom layer.
//!
//! These tests go through `Dom`'s methods only (no private internals), so
//! they double as executable documentation of the boundary contract.

use dom::{Attribute, Dom, DomError, LocalName, Namespace, NodeId, NodeKind, QualName};

/// The HTML namespace URL; `markup5ever`'s `ns!` macro wraps this same string.
const HTML_NS: &str = "http://www.w3.org/1999/xhtml";

/// HTML-namespace qualified name with no prefix.
fn qn(local: &str) -> QualName {
    QualName::new(None, Namespace::from(HTML_NS), LocalName::from(local))
}

/// No-namespace attribute with the given local name and value.
fn attr(name: &str, value: &str) -> Attribute {
    Attribute {
        name: qn(name),
        value: value.to_string(),
    }
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
fn clone_node_copies_subtree_with_fresh_ids() {
    let mut d = Dom::new();
    let p = d.create_element(qn("p"), vec![attr("id", "a")]);
    let text = d.create_text("hi");
    d.append(p, text).unwrap();
    let copy = d.clone_node(p, true).unwrap();
    assert_ne!(copy, p);
    match d.get(copy).map(|view| view.kind()) {
        Some(NodeKind::Element { attributes, .. }) => {
            assert_eq!(attributes[0].value, "a");
        }
        other => panic!("expected element, got {other:?}"),
    }
    let copied_text = d.children(copy).unwrap().copied().next().unwrap();
    assert_ne!(copied_text, text);
    match d.get(copied_text).map(|view| view.kind()) {
        Some(NodeKind::Text { data }) => assert_eq!(data, "hi"),
        other => panic!("expected text, got {other:?}"),
    }
    match d.get(text).map(|view| view.kind()) {
        Some(NodeKind::Text { data }) => assert_eq!(data, "hi"),
        other => panic!("original text changed: {other:?}"),
    }
}

#[test]
fn clone_node_refuses_the_document() {
    let mut d = Dom::new();
    let doc = d.document();
    assert_eq!(d.clone_node(doc, true), Err(DomError::WrongNodeType));
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
    let list = d.create_element(qn("ul"), Vec::new());
    d.append(doc, list).unwrap();
    let mk = |d: &mut Dom| d.create_element(qn("li"), Vec::new());
    let (a, b, c) = (mk(&mut d), mk(&mut d), mk(&mut d));

    d.append(list, a).unwrap();
    d.append(list, c).unwrap();
    d.insert_before(c, b).unwrap();

    assert_eq!(
        d.children(list).unwrap().copied().collect::<Vec<_>>(),
        vec![a, b, c]
    );
}

#[test]
fn insert_before_head_and_document_are_handled() {
    let mut d = Dom::new();
    let doc = d.document();
    let x = d.create_element(qn("x"), Vec::new());
    let y = d.create_element(qn("y"), Vec::new());

    // The document node has no parent, so there is nothing to insert beside
    // it: the spec's NotFoundError path (pre-insert: parent null).
    assert_eq!(d.insert_before(doc, x), Err(DomError::NoParent));

    d.append(doc, x).unwrap();
    // A document holds at most one element child, so a second element is
    // refused even via insert_before: the old silent two-root tree (M4).
    assert_eq!(d.insert_before(x, y), Err(DomError::HierarchyRequest));

    // Comments, however, are welcome beside the document element.
    let note = d.create_comment("prolog");
    d.insert_before(x, note).unwrap();
    assert_eq!(
        d.children(doc).unwrap().copied().collect::<Vec<_>>(),
        vec![note, x]
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
    assert_eq!(d.detach(doc), Err(DomError::HierarchyRequest));
}

#[test]
fn reparent_children_moves_everything_in_order() {
    let mut d = Dom::new();
    let doc = d.document();
    let body = d.create_element(qn("body"), Vec::new());
    d.append(doc, body).unwrap();
    let from = d.create_element(qn("from"), Vec::new());
    let to = d.create_element(qn("to"), Vec::new());
    d.append(body, from).unwrap();
    d.append(body, to).unwrap();

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
    // The button/span scenario from wayfinding: create, destroy, reuse;
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
    assert_eq!(d.set_text(element, "nope"), Err(DomError::WrongNodeType));
    assert_eq!(d.set_comment(element, "nope"), Err(DomError::WrongNodeType));

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
        Err(DomError::WrongNodeType)
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

    // inserting a node beside itself is a spec-sanctioned no-op
    // (https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity),
    // not a reorder and not an error
    d.insert_before(inner, inner).unwrap();
    assert_eq!(
        d.children(outer).unwrap().copied().collect::<Vec<_>>(),
        vec![inner]
    );

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
    assert_eq!(d.append(stray, doc), Err(DomError::HierarchyRequest));

    // nor may its content drain into an arbitrary detached subtree
    assert_eq!(
        d.reparent_children(doc, stray),
        Err(DomError::HierarchyRequest)
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
    assert_eq!(
        d.children(wide).unwrap().copied().collect::<Vec<_>>(),
        expected
    );
}

#[test]
fn insert_into_wide_list_lands_exactly() {
    let mut d = Dom::new();
    let doc = d.document();
    let wide = d.create_element(qn("wide"), Vec::new());
    d.append(doc, wide).unwrap();

    // deep into heap-backed territory: inserts at every edge of a long list
    let kids: Vec<_> = (0..10)
        .map(|_| d.create_element(qn("span"), Vec::new()))
        .collect();
    for &k in &kids {
        d.append(wide, k).unwrap();
    }

    let head = d.create_element(qn("h"), Vec::new());
    d.insert_before(kids[0], head).unwrap();
    let middle = d.create_element(qn("m"), Vec::new());
    d.insert_before(kids[5], middle).unwrap();
    let tail = d.create_element(qn("t"), Vec::new());
    d.append(wide, tail).unwrap();

    let expected: Vec<_> = [
        vec![head],
        kids[..5].to_vec(),
        vec![middle],
        kids[5..].to_vec(),
        vec![tail],
    ]
    .concat();
    assert_eq!(
        d.children(wide).unwrap().copied().collect::<Vec<_>>(),
        expected
    );
}

/// xorshift64*: tiny, deterministic, good enough to scatter op choices.
fn storm_roll(state: &mut u64, n: u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    state.wrapping_mul(0x2545_F491_4F6C_DD1D) % n.max(1)
}

/// One uniformly chosen live handle, or `None` when nothing is left alive.
fn storm_pick(d: &Dom, tracked: &[NodeId], state: &mut u64) -> Option<NodeId> {
    let alive: Vec<NodeId> = tracked
        .iter()
        .copied()
        .filter(|&id| d.contains(id))
        .collect();
    let count = u64::try_from(alive.len()).unwrap_or(u64::MAX);
    let picked = usize::try_from(storm_roll(state, count)).unwrap_or(0);
    alive.into_iter().nth(picked)
}

/// Deterministic mutation storm: thousands of random structural ops with a
/// bidirectional-link audit after every step.
///
/// This is the property-style guard for the parent-pointer/child-list
/// duality, the exact invariant class whose silent breakage a one-node-
/// two-parents corruption would need (see `unlink_from_current_parent`'s
/// defect policy). Seeded xorshift, so any failure reproduces exactly; no
/// property-testing dependency, per the repo's dependency diet.
#[test]
fn mutation_storm_keeps_parent_links_bidirectional() {
    use std::collections::HashSet;

    let mut d = Dom::new();
    let document = d.document();
    let mut tracked = vec![document];
    for _ in 0..8 {
        tracked.push(d.create_element(qn("host"), Vec::new()));
    }
    let mut state = 0x2545_F491_4F6C_DD1D_u64;

    for _ in 0..1500 {
        match storm_roll(&mut state, 8) {
            0 => {
                tracked.push(d.create_element(qn("e"), Vec::new()));
            }
            1 => {
                tracked.push(d.create_text("t"));
            }
            2 | 3 => {
                if let (Some(parent), Some(child)) = (
                    storm_pick(&d, &tracked, &mut state),
                    storm_pick(&d, &tracked, &mut state),
                ) {
                    let _ = d.append(parent, child); // cycle refusals expected
                }
            }
            4 => {
                if let (Some(sibling), Some(node)) = (
                    storm_pick(&d, &tracked, &mut state),
                    storm_pick(&d, &tracked, &mut state),
                ) {
                    let _ = d.insert_before(sibling, node);
                }
            }
            5 => {
                if let Some(id) = storm_pick(&d, &tracked, &mut state)
                    && id != document
                {
                    let _ = d.detach(id);
                }
            }
            6 => {
                if let (Some(from), Some(to)) = (
                    storm_pick(&d, &tracked, &mut state),
                    storm_pick(&d, &tracked, &mut state),
                ) {
                    let _ = d.reparent_children(from, to);
                }
            }
            _ => {
                if let Some(id) = storm_pick(&d, &tracked, &mut state)
                    && id != document
                {
                    let _ = d.destroy(id); // stales the subtree; contains() filters below
                }
            }
        }

        // Audit: every live node's two link directions agree, and no parent
        // chain ever loops.
        for &id in &tracked {
            if !d.contains(id) {
                continue;
            }
            if let Some(parent) = d.parent(id) {
                assert!(
                    d.contains(parent) && d.children(parent).unwrap().any(|&kid| kid == id),
                    "node {id:?} names parent {parent:?} but is absent from its list"
                );
            }
            if let Some(kids) = d.children(id) {
                for kid in kids {
                    assert_eq!(
                        d.parent(*kid),
                        Some(id),
                        "child {kid:?} is listed under {id:?} but disowns it"
                    );
                }
            }
            let mut cursor = Some(id);
            let mut seen = HashSet::new();
            while let Some(step) = cursor {
                assert!(
                    seen.insert(step),
                    "parent chain from {id:?} cycles at {step:?}"
                );
                cursor = d.parent(step);
            }
        }
    }
}

#[test]
fn fragments_are_detached_containers() {
    let mut d = Dom::new();
    let f = d.create_fragment();
    let t = d.create_text("hi");
    d.append(f, t).unwrap();

    assert!(matches!(
        d.get(f).map(|n| n.kind()),
        Some(NodeKind::Fragment)
    ));
    // outside the main tree: no parent, but fully live and usable
    assert_eq!(d.parent(f), None);
    assert_eq!(d.children(f).unwrap().copied().collect::<Vec<_>>(), vec![t]);
}

#[test]
fn template_contents_live_on_dom_outside_the_child_list() {
    let mut d = Dom::new();
    let template = d.create_element(qn("template"), Vec::new());
    let contents = d.create_fragment();
    let inner = d.create_element(qn("div"), Vec::new());
    d.append(contents, inner).unwrap();

    d.set_template_contents(template, contents).unwrap();
    assert_eq!(d.template_contents(template), Some(contents));
    assert_eq!(d.children(template).unwrap().count(), 0);
    assert_eq!(
        d.children(contents).unwrap().copied().collect::<Vec<_>>(),
        vec![inner]
    );

    d.destroy(template).unwrap();
    assert!(d.template_contents(template).is_none());
    assert!(!d.contains(contents));
    assert!(!d.contains(inner));
}

#[test]
fn template_contents_refuse_non_template_and_shared_fragments() {
    let mut d = Dom::new();
    let div = d.create_element(qn("div"), Vec::new());
    let fragment = d.create_fragment();
    assert_eq!(
        d.set_template_contents(div, fragment),
        Err(DomError::WrongNodeType)
    );

    let template = d.create_element(qn("template"), Vec::new());
    d.set_template_contents(template, fragment).unwrap();
    let other = d.create_element(qn("template"), Vec::new());
    assert_eq!(
        d.set_template_contents(other, fragment),
        Err(DomError::WrongNodeType)
    );

    let replacement = d.create_fragment();
    d.set_template_contents(template, replacement).unwrap();
    assert!(!d.contains(fragment));
    assert_eq!(d.template_contents(template), Some(replacement));
}

// ── content model: the pre-insert gate (WHATWG ensure pre-insert validity) ──

/// A document holds at most one element child: the spec throws
/// `HierarchyRequestError` for a second root
/// (<https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity>).
/// Before the gate this silently produced two roots, both of which matched
/// `:root`.
#[test]
fn documents_take_at_most_one_element_child() {
    let mut d = Dom::new();
    let doc = d.document();
    let html = d.create_element(qn("html"), Vec::new());
    d.append(doc, html).unwrap();

    let html2 = d.create_element(qn("html"), Vec::new());
    assert_eq!(d.append(doc, html2), Err(DomError::HierarchyRequest));
    // ...and the refusal is total: still exactly one element child.
    assert_eq!(d.children(doc).unwrap().count(), 1);
}

/// Doctype rules from the same gate: at most one, only in a document,
/// always before the document element.
#[test]
fn doctype_placement_follows_the_content_model() {
    let mut d = Dom::new();
    let doc = d.document();
    let div = d.create_element(qn("div"), Vec::new());
    d.append(doc, div).unwrap();
    let dt = d.create_doctype("html", "", "");

    // nowhere but a document
    assert_eq!(d.append(div, dt), Err(DomError::HierarchyRequest));

    // after the document element is too late
    let late = d.create_doctype("html", "", "");
    assert_eq!(d.append(doc, late), Err(DomError::HierarchyRequest));

    // ahead of the document element is exactly right
    let early = d.create_doctype("html", "", "");
    d.insert_before(div, early).unwrap();
    assert_eq!(
        d.children(doc).unwrap().copied().collect::<Vec<_>>(),
        vec![early, div]
    );

    // a second doctype is refused even when well placed
    let second = d.create_doctype("html", "", "");
    assert_eq!(d.append(doc, second), Err(DomError::HierarchyRequest));

    // ...and an element may not leap ahead of the standing doctype either:
    // the doctype must precede the document element, from both directions.
    let another_div = d.create_element(qn("div"), Vec::new());
    assert_eq!(
        d.insert_before(early, another_div),
        Err(DomError::HierarchyRequest)
    );
    assert_eq!(
        d.children(doc).unwrap().copied().collect::<Vec<_>>(),
        vec![early, div]
    );
}
/// Leaves are not parents: character data and doctypes refuse children with
/// `HierarchyRequestError` (the gate's container-kind rule). This was audit
/// finding M5: the old code accepted the append and document-order matching
/// then walked the fake subtree.
#[test]
fn leaves_refuse_children_and_answer_empty_child_lists() {
    let mut d = Dom::new();
    let doc = d.document();
    let text = d.create_text("t");
    let comment = d.create_comment("c");
    let span = d.create_element(qn("span"), Vec::new());

    assert_eq!(d.append(text, span), Err(DomError::HierarchyRequest));
    assert_eq!(d.append(comment, span), Err(DomError::HierarchyRequest));
    let dt = d.create_doctype("html", "", "");
    assert_eq!(d.append(dt, span), Err(DomError::HierarchyRequest));
    // bulk moves are gated on container-ness too
    let host = d.create_element(qn("div"), Vec::new());
    d.append(doc, host).unwrap();
    assert_eq!(
        d.reparent_children(host, text),
        Err(DomError::HierarchyRequest)
    );
    assert_eq!(
        d.reparent_children(text, host),
        Err(DomError::HierarchyRequest)
    );

    // childNodes semantics: every live node answers a list; leaves answer
    // an empty one, never None (that answer belongs to stale handles).
    assert_eq!(d.children(text).unwrap().count(), 0);
    assert_eq!(d.children(comment).unwrap().count(), 0);
}

/// Documents hold no character data (content-model step for Document).
#[test]
fn documents_refuse_character_data() {
    let mut d = Dom::new();
    let doc = d.document();
    let t = d.create_text("stray");
    assert_eq!(d.append(doc, t), Err(DomError::HierarchyRequest));
}

/// The bulk move answers to the document content model like every other
/// path into a document. A run is validated as a *whole*, because per-child
/// checks cannot see the pair: `[a, b]` into an empty document passes
/// child-by-child and fails as a batch.
///
/// Doctypes need no bulk-run test: gated insertion keeps them directly
/// under the root, and draining the root is refused, so no doctype can
/// ever appear inside a moved run.
#[test]
fn reparent_children_into_a_document_honors_the_content_model() {
    let mut d = Dom::new();
    let doc = d.document();

    // A second element cannot land while one stands.
    let html = d.create_element(qn("html"), Vec::new());
    d.append(doc, html).unwrap();
    let run = d.create_element(qn("div"), Vec::new());
    let stray = d.create_element(qn("span"), Vec::new());
    d.append(run, stray).unwrap();
    assert_eq!(
        d.reparent_children(run, doc),
        Err(DomError::HierarchyRequest)
    );
    assert_eq!(
        d.children(run).unwrap().copied().collect::<Vec<_>>(),
        vec![stray],
        "refusal left the source run intact"
    );

    // Two elements into an *empty* document: each alone would pass, the
    // pair may not.
    d.detach(html).unwrap();
    assert_eq!(d.children(doc).unwrap().count(), 0);
    let pair_host = d.create_element(qn("pair-host"), Vec::new());
    let a = d.create_element(qn("a"), Vec::new());
    let b = d.create_element(qn("b"), Vec::new());
    d.append(pair_host, a).unwrap();
    d.append(pair_host, b).unwrap();
    assert_eq!(
        d.reparent_children(pair_host, doc),
        Err(DomError::HierarchyRequest)
    );
    assert_eq!(d.children(doc).unwrap().count(), 0);

    // Character data in the run is refused too, even beside nothing.
    let text_host = d.create_element(qn("text-host"), Vec::new());
    let words = d.create_text("stray");
    d.append(text_host, words).unwrap();
    assert_eq!(
        d.reparent_children(text_host, doc),
        Err(DomError::HierarchyRequest)
    );

    // A single element remains exactly as legal as through append.
    let solo_host = d.create_element(qn("solo-host"), Vec::new());
    let main = d.create_element(qn("main"), Vec::new());
    d.append(solo_host, main).unwrap();
    d.reparent_children(solo_host, doc).unwrap();
    assert_eq!(
        d.children(doc).unwrap().copied().collect::<Vec<_>>(),
        vec![main]
    );
    assert_eq!(d.parent(main), Some(doc));

    // Comments ride along fine, before or after the document element.
    let note_host = d.create_element(qn("note-host"), Vec::new());
    let note = d.create_comment("epilog");
    d.append(note_host, note).unwrap();
    d.reparent_children(note_host, doc).unwrap();
    assert_eq!(
        d.children(doc).unwrap().copied().collect::<Vec<_>>(),
        vec![main, note]
    );
}

/// Fragments splice: inserting a fragment moves its children into the
/// parent and leaves the fragment empty
/// (<https://dom.spec.whatwg.org/#concept-node-insert>).
#[test]
fn inserting_a_fragment_splices_its_children() {
    let mut d = Dom::new();
    let doc = d.document();
    let frag = d.create_fragment();
    let item = d.create_element(qn("b"), Vec::new());
    d.append(frag, item).unwrap();
    let host = d.create_element(qn("section"), Vec::new());
    d.append(doc, host).unwrap();

    d.append(host, frag).unwrap();
    assert_eq!(d.parent(item), Some(host));
    assert_eq!(
        d.children(host).unwrap().copied().collect::<Vec<_>>(),
        vec![item]
    );
    assert_eq!(d.parent(frag), None);
    assert_eq!(d.children(frag).unwrap().count(), 0);
    assert!(d.contains(frag));
}

/// A fragment with two element children cannot land in a document
/// (<https://dom.spec.whatwg.org/#concept-node-ensure-pre-insert-validity>).
#[test]
fn document_refuses_a_fragment_with_two_element_children() {
    let mut d = Dom::new();
    let frag = d.create_fragment();
    let a = d.create_element(qn("html"), Vec::new());
    let b = d.create_element(qn("body"), Vec::new());
    d.append(frag, a).unwrap();
    d.append(frag, b).unwrap();
    assert_eq!(
        d.append(d.document(), frag),
        Err(DomError::HierarchyRequest)
    );
    assert_eq!(d.children(frag).unwrap().count(), 2);
}

#[test]
fn handles_from_another_document_do_not_resolve() {
    let mut a = Dom::new();
    let mut b = Dom::new();
    let a_el = a.create_element(qn("div"), Vec::new());
    let b_el = b.create_element(qn("div"), Vec::new());
    assert_ne!(a_el, b_el);
    assert!(!a.contains(b_el));
    assert!(!b.contains(a_el));
    assert_eq!(a.append(a.document(), b_el), Err(DomError::StaleNode));
    a.append(a.document(), a_el).unwrap();
    assert_eq!(a.parent(a_el), Some(a.document()));
}

/// Attribute names are unique per DOM (`NamedNodeMap` keyed by qualified
/// name); hand-built duplicates collapse first-wins, matching how repeated
/// start tags merge.
#[test]
fn duplicate_attributes_dedupe_first_wins() {
    let mut d = Dom::new();
    let el = d.create_element(
        qn("i"),
        vec![
            attr("id", "first"),
            attr("class", "keep"),
            attr("id", "second"),
        ],
    );
    match d.get(el).map(|view| view.kind()) {
        Some(NodeKind::Element { attributes, .. }) => {
            assert_eq!(attributes.len(), 2);
            assert_eq!(attributes[0].value, "first");
            assert_eq!(attributes[1].value, "keep");
        }
        other => panic!("expected element, got {other:?}"),
    }
}
