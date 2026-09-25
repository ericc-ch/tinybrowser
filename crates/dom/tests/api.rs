use dom::{Attribute, Dom, DomError, Lifecycle, LocalName, Namespace, NodeId, NodeKind, QualName};

const HTML_NS: &str = "http://www.w3.org/1999/xhtml";

fn qn(local: &str) -> QualName {
    QualName::new(None, Namespace::from(HTML_NS), LocalName::from(local))
}

#[derive(Default)]
struct Model {
    alive: Vec<bool>,
    parents: Vec<Option<usize>>,
    children: Vec<Vec<usize>>,
}

impl Model {
    fn create(&mut self) -> usize {
        let id = self.alive.len();
        self.alive.push(true);
        self.parents.push(None);
        self.children.push(Vec::new());
        id
    }

    fn is_ancestor(&self, ancestor: usize, mut node: usize) -> bool {
        loop {
            if ancestor == node {
                return true;
            }
            let Some(parent) = self.parents[node] else {
                return false;
            };
            node = parent;
        }
    }

    fn detach(&mut self, node: usize) {
        if let Some(parent) = self.parents[node].take() {
            self.children[parent].retain(|&child| child != node);
        }
    }

    fn append(&mut self, parent: usize, child: usize) -> Result<(), DomError> {
        if self.is_ancestor(child, parent) {
            return Err(DomError::CycleForbidden);
        }
        self.detach(child);
        self.parents[child] = Some(parent);
        self.children[parent].push(child);
        Ok(())
    }

    fn insert_before(&mut self, sibling: usize, node: usize) -> Result<(), DomError> {
        let Some(parent) = self.parents[sibling] else {
            return Err(DomError::NoParent);
        };
        if sibling == node {
            return Ok(());
        }
        if self.is_ancestor(node, parent) {
            return Err(DomError::CycleForbidden);
        }
        self.detach(node);
        let position = self.children[parent]
            .iter()
            .position(|&child| child == sibling)
            .expect("model sibling remains parented");
        self.parents[node] = Some(parent);
        self.children[parent].insert(position, node);
        Ok(())
    }

    fn destroy(&mut self, node: usize) {
        self.detach(node);
        let mut pending = vec![node];
        while let Some(current) = pending.pop() {
            pending.extend(self.children[current].iter().copied());
            self.children[current].clear();
            self.parents[current] = None;
            self.alive[current] = false;
        }
    }

    fn live(&self, state: &mut u64, include_root: bool) -> usize {
        let choices: Vec<_> = self
            .alive
            .iter()
            .enumerate()
            .filter_map(|(id, &alive)| (alive && (include_root || id != 0)).then_some(id))
            .collect();
        choices[roll(state, choices.len())]
    }
}

fn roll(state: &mut u64, limit: usize) -> usize {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    usize::try_from(*state % u64::try_from(limit).expect("choice count fits u64"))
        .expect("choice fits usize")
}

fn assert_matches(dom: &Dom, document: NodeId, handles: &[NodeId], model: &Model) {
    for (index, &handle) in handles.iter().enumerate() {
        assert_eq!(dom.contains(handle), model.alive[index], "liveness {index}");
        if !model.alive[index] {
            assert!(dom.kind(handle).is_none(), "dead node {index} resolves");
            assert!(
                dom.children(handle).is_none(),
                "dead node {index} has children"
            );
            continue;
        }
        let expected_parent = if index == 0 {
            Some(document)
        } else {
            model.parents[index].map(|parent| handles[parent])
        };
        assert_eq!(dom.parent(handle), expected_parent, "parent {index}");
        let expected_children: Vec<_> = model.children[index]
            .iter()
            .map(|&child| handles[child])
            .collect();
        assert_eq!(
            dom.children(handle)
                .expect("live elements have children")
                .collect::<Vec<_>>(),
            expected_children,
            "children {index}"
        );
        // The intrusive links must agree with the child run the model keeps:
        // heads and tails, then every neighbour pair in both directions.
        assert_eq!(
            dom.first_child(handle),
            expected_children.first().copied(),
            "first child {index}"
        );
        assert_eq!(
            dom.last_child(handle),
            expected_children.last().copied(),
            "last child {index}"
        );
        for (position, &child) in expected_children.iter().enumerate() {
            assert_eq!(
                dom.previous_sibling(child),
                position
                    .checked_sub(1)
                    .map(|previous| expected_children[previous]),
                "previous sibling {index}/{position}"
            );
            assert_eq!(
                dom.next_sibling(child),
                expected_children.get(position + 1).copied(),
                "next sibling {index}/{position}"
            );
        }
    }
}

#[test]
fn mutations_match_an_independent_tree_model() {
    fn assert_send<T: Send>() {}
    assert_send::<Dom>();

    let mut dom = Dom::new();
    let document = dom.document();
    let root = dom.create_element(qn("root"), Vec::new());
    dom.append(document, root).expect("root");

    let mut handles = vec![root];
    let mut model = Model::default();
    model.create();
    for _ in 0..12 {
        handles.push(dom.create_element(qn("node"), Vec::new()));
        model.create();
    }
    for (child, _) in handles.iter().enumerate().skip(1).take(4) {
        assert_eq!(dom.append(root, handles[child]), model.append(0, child));
    }
    assert_eq!(dom.append(handles[1], handles[5]), model.append(1, 5));
    assert_eq!(dom.append(handles[5], handles[1]), model.append(5, 1));

    let mut state = 0x2545_F491_4F6C_DD1D_u64;
    let mut successes = 0_usize;
    let mut refusals = 0_usize;
    for _ in 0..800 {
        let has_non_root = model.alive.iter().skip(1).any(|&alive| alive);
        let operation = if has_non_root { roll(&mut state, 5) } else { 0 };
        match operation {
            0 => {
                handles.push(dom.create_element(qn("node"), Vec::new()));
                model.create();
            }
            1 => {
                let parent = model.live(&mut state, true);
                let child = model.live(&mut state, false);
                let expected = model.append(parent, child);
                let actual = dom.append(handles[parent], handles[child]);
                assert_eq!(actual, expected);
                if actual.is_ok() {
                    successes += 1;
                } else {
                    refusals += 1;
                }
            }
            2 => {
                let sibling = model.live(&mut state, false);
                let node = model.live(&mut state, false);
                let expected = model.insert_before(sibling, node);
                let actual = dom.insert_before(handles[sibling], handles[node]);
                assert_eq!(actual, expected);
                if actual.is_ok() {
                    successes += 1;
                } else {
                    refusals += 1;
                }
            }
            3 => {
                let node = model.live(&mut state, false);
                model.detach(node);
                dom.detach(handles[node]).expect("live element detaches");
                successes += 1;
            }
            4 => {
                let node = model.live(&mut state, false);
                model.destroy(node);
                dom.destroy(handles[node]).expect("live subtree destroys");
                successes += 1;
            }
            _ => unreachable!(),
        }
        assert_matches(&dom, document, &handles, &model);
    }
    assert!(
        successes > 100,
        "model exercised only {successes} mutations"
    );
    assert!(refusals > 10, "model exercised only {refusals} refusals");
}

#[test]
fn document_fragments_templates_and_clones_keep_their_contracts() {
    let mut dom = Dom::new();
    let document = dom.document();
    let doctype = dom.create_doctype("html", "", "");
    let html = dom.create_element(qn("html"), Vec::new());
    dom.append(document, doctype).expect("doctype");
    dom.append(document, html).expect("document element");

    let second_root = dom.create_element(qn("second"), Vec::new());
    assert_eq!(
        dom.append(document, second_root),
        Err(DomError::HierarchyRequest)
    );
    let text = dom.create_text("outside");
    assert_eq!(dom.append(document, text), Err(DomError::HierarchyRequest));

    let fragment = dom.create_fragment();
    let first = dom.create_element(
        qn("p"),
        vec![Attribute {
            name: qn("id"),
            value: "first".into(),
        }],
    );
    let second = dom.create_comment("second");
    dom.append(fragment, first).expect("fragment child");
    dom.append(fragment, second).expect("fragment child");
    dom.append(html, fragment).expect("fragment splice");
    assert_eq!(
        dom.children(html)
            .expect("html children")
            .collect::<Vec<_>>(),
        vec![first, second]
    );
    assert_eq!(dom.children(fragment).expect("fragment").count(), 0);

    let template = dom.create_element(qn("template"), Vec::new());
    let contents = dom.create_fragment();
    dom.set_template_contents(template, contents)
        .expect("template contents");
    let inner = dom.create_text("inside");
    dom.append(contents, inner).expect("template text");
    dom.append(html, template).expect("template");
    assert_eq!(
        dom.children(template).expect("template children").count(),
        0
    );
    assert_eq!(dom.template_contents(template), Some(contents));

    let clone = dom.clone_node(template, true).expect("deep template clone");
    let clone_contents = dom.template_contents(clone).expect("clone contents");
    assert_ne!(clone_contents, contents);
    assert_eq!(
        dom.children(clone_contents)
            .expect("clone children")
            .count(),
        1
    );

    dom.destroy(template).expect("destroy template subtree");
    assert!(!dom.contains(template));
    assert!(!dom.contains(contents));
    assert!(!dom.contains(inner));
    assert!(matches!(
        dom.kind(first),
        Some(NodeKind::Element { attributes, .. }) if attributes[0].value == "first"
    ));

    let other = Dom::new();
    assert_eq!(dom.append(html, other.document()), Err(DomError::StaleNode));
}

#[test]
fn connection_transitions_record_lifecycle_events() {
    let mut dom = Dom::new();
    let root = dom.document();
    let html = dom.create_element(qn("html"), Vec::new());
    let body = dom.create_element(qn("body"), Vec::new());
    let iframe = dom.create_element(qn("iframe"), Vec::new());

    dom.append(root, html).expect("html");
    dom.append(html, body).expect("body");
    // Only iframe transitions are tracked; ordinary elements are not.
    assert!(dom.take_lifecycle().is_empty());

    dom.append(body, iframe).expect("iframe");
    assert_eq!(dom.take_lifecycle(), vec![Lifecycle::Inserted(iframe)]);

    // A detached subtree records nothing until it is connected...
    let detached = dom.create_element(qn("div"), Vec::new());
    let nested = dom.create_element(qn("iframe"), Vec::new());
    dom.append(detached, nested).expect("detached iframe");
    assert!(dom.take_lifecycle().is_empty());

    // ...then every iframe in the inserted subtree is reported.
    dom.append(body, detached).expect("connect subtree");
    assert_eq!(dom.take_lifecycle(), vec![Lifecycle::Inserted(nested)]);

    // Moving a connected element is not a removal plus insertion.
    dom.append(body, nested).expect("move nested");
    assert!(dom.take_lifecycle().is_empty());

    // Detaching reports the removed iframe.
    dom.detach(nested).expect("detach");
    assert_eq!(dom.take_lifecycle(), vec![Lifecycle::Removed(nested)]);
}

#[test]
fn img_connection_transitions_record_lifecycle_events() {
    let mut dom = Dom::new();
    let root = dom.document();
    let html = dom.create_element(qn("html"), Vec::new());
    let body = dom.create_element(qn("body"), Vec::new());
    let img = dom.create_element(qn("img"), Vec::new());

    dom.append(root, html).expect("html");
    dom.append(html, body).expect("body");
    assert!(dom.take_lifecycle().is_empty());

    dom.append(body, img).expect("img");
    assert_eq!(dom.take_lifecycle(), vec![Lifecycle::Inserted(img)]);

    dom.detach(img).expect("detach");
    assert_eq!(dom.take_lifecycle(), vec![Lifecycle::Removed(img)]);
}

#[test]
fn connected_iframes_survive_replace_and_report_destroy() {
    let mut dom = Dom::new();
    let document = dom.document();
    let root = dom.create_element(qn("root"), Vec::new());
    let holder = dom.create_element(qn("holder"), Vec::new());
    let iframe = dom.create_element(qn("iframe"), Vec::new());
    dom.append(document, root).expect("root");
    dom.append(root, holder).expect("holder");
    dom.append(holder, iframe).expect("iframe");
    assert_eq!(dom.connected_iframe_count(), 1);
    let _ = dom.take_lifecycle();

    // `replaceChildren(holder.firstChild)`: the iframe stays connected across
    // the replace, so it must neither churn an event nor double the count.
    dom.replace_all(holder, iframe).expect("replace_all");
    assert_eq!(dom.connected_iframe_count(), 1);
    assert!(dom.take_lifecycle().is_empty());

    // `replaceChild` where the replacement sits inside the replaced node: the
    // transient detach must not read as removed-then-reinserted either.
    let wrapper = dom.create_element(qn("wrapper"), Vec::new());
    dom.replace_all(holder, wrapper).expect("wrap");
    let nested = dom.create_element(qn("iframe"), Vec::new());
    dom.append(wrapper, nested).expect("nested iframe");
    assert_eq!(dom.connected_iframe_count(), 1);
    let _ = dom.take_lifecycle();
    dom.replace_child(holder, nested, wrapper)
        .expect("replace_child");
    assert_eq!(dom.connected_iframe_count(), 1);
    assert!(dom.take_lifecycle().is_empty());

    // Destroying a connected iframe reports the removal and drops the count.
    dom.destroy(nested).expect("destroy");
    assert_eq!(dom.connected_iframe_count(), 0);
    assert_eq!(dom.take_lifecycle(), vec![Lifecycle::Removed(nested)]);
}

/// Asserts the intrusive links of `parent` match `expected` exactly: the
/// child run, its endpoints, and every neighbour in both directions.
fn assert_links(dom: &Dom, parent: NodeId, expected: &[NodeId]) {
    assert_eq!(
        dom.children(parent)
            .expect("live parent")
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(dom.first_child(parent), expected.first().copied());
    assert_eq!(dom.last_child(parent), expected.last().copied());
    for (position, &child) in expected.iter().enumerate() {
        assert_eq!(
            dom.parent(child),
            Some(parent),
            "parent of child {position}"
        );
        assert_eq!(
            dom.previous_sibling(child),
            position.checked_sub(1).map(|previous| expected[previous]),
            "previous sibling of child {position}"
        );
        assert_eq!(
            dom.next_sibling(child),
            expected.get(position + 1).copied(),
            "next sibling of child {position}"
        );
    }
}

#[test]
fn bulk_moves_keep_the_link_invariant() {
    let mut dom = Dom::new();
    let wrapper = dom.create_element(qn("wrapper"), Vec::new());
    let host = dom.create_element(qn("host"), Vec::new());
    let dest = dom.create_element(qn("dest"), Vec::new());
    dom.append(wrapper, host).expect("host");
    dom.append(wrapper, dest).expect("dest");

    let first = dom.create_element(qn("first"), Vec::new());
    let second = dom.create_element(qn("second"), Vec::new());
    let third = dom.create_element(qn("third"), Vec::new());
    for &child in &[first, second, third] {
        dom.append(host, child).expect("host child");
    }
    assert_links(&dom, host, &[first, second, third]);

    // replace_all detaches the standing children and links the replacement.
    let replacement = dom.create_element(qn("replacement"), Vec::new());
    dom.replace_all(host, replacement).expect("replace_all");
    assert_links(&dom, host, &[replacement]);
    for &detached in &[first, second, third] {
        assert_eq!(dom.parent(detached), None);
        assert_eq!(dom.previous_sibling(detached), None);
        assert_eq!(dom.next_sibling(detached), None);
    }

    // replace_all with a node already in the standing set.
    let kept = dom.create_element(qn("kept"), Vec::new());
    dom.append(host, kept).expect("kept");
    assert_links(&dom, host, &[replacement, kept]);
    dom.replace_all(host, kept)
        .expect("replace_all reusing a child");
    assert_links(&dom, host, &[kept]);
    assert_eq!(dom.parent(replacement), None);
    assert_eq!(dom.previous_sibling(replacement), None);
    assert_eq!(dom.next_sibling(replacement), None);

    // replace_child swaps exactly one node.
    let replaced = dom.create_element(qn("replaced"), Vec::new());
    dom.append(host, replaced).expect("replaced");
    let inserted = dom.create_element(qn("inserted"), Vec::new());
    dom.replace_child(host, inserted, replaced)
        .expect("replace_child");
    assert_links(&dom, host, &[kept, inserted]);
    assert_eq!(dom.parent(replaced), None);
    assert_eq!(dom.next_sibling(replaced), None);

    // reparent_children moves the whole run in order to the destination.
    let marker = dom.create_comment("marker");
    dom.append(dest, marker).expect("dest child");
    dom.reparent_children(host, dest)
        .expect("reparent_children");
    assert_links(&dom, host, &[]);
    assert_links(&dom, dest, &[marker, kept, inserted]);

    // Inserting a fragment splices its children before the reference.
    let fragment = dom.create_fragment();
    let frag_one = dom.create_element(qn("frag-one"), Vec::new());
    let frag_two = dom.create_element(qn("frag-two"), Vec::new());
    dom.append(fragment, frag_one).expect("frag_one");
    dom.append(fragment, frag_two).expect("frag_two");
    dom.insert_before(kept, fragment).expect("splice fragment");
    assert_links(&dom, dest, &[marker, frag_one, frag_two, kept, inserted]);
    assert_eq!(dom.children(fragment).expect("fragment").count(), 0);

    // Appending a fragment splices its children at the end.
    let tail = dom.create_fragment();
    let tail_one = dom.create_element(qn("tail-one"), Vec::new());
    let tail_two = dom.create_element(qn("tail-two"), Vec::new());
    dom.append(tail, tail_one).expect("tail_one");
    dom.append(tail, tail_two).expect("tail_two");
    dom.append(dest, tail).expect("append fragment");
    assert_links(
        &dom,
        dest,
        &[
            marker, frag_one, frag_two, kept, inserted, tail_one, tail_two,
        ],
    );
    assert_eq!(dom.children(tail).expect("tail").count(), 0);

    // replace_all with a fragment replaces the run with the fragment's children.
    let swap = dom.create_fragment();
    let swap_child = dom.create_element(qn("swap-child"), Vec::new());
    dom.append(swap, swap_child).expect("swap_child");
    dom.replace_all(dest, swap).expect("replace_all fragment");
    assert_links(&dom, dest, &[swap_child]);
}

#[test]
fn child_iteration_covers_each_child_once_across_both_directions() {
    let mut dom = Dom::new();
    let document = dom.document();
    let root = dom.create_element(qn("root"), Vec::new());
    dom.append(document, root).expect("root");

    let mut kids = Vec::new();
    for _ in 0..5 {
        let kid = dom.create_element(qn("node"), Vec::new());
        dom.append(root, kid).expect("child");
        kids.push(kid);
    }

    // Alternating ends must yield every child exactly once: the two cursors
    // retire together when they meet, so neither can repeat the other's node.
    let mut iter = dom.children(root).expect("root children");
    let mut front = true;
    let mut seen = Vec::new();
    while let Some(id) = if front { iter.next() } else { iter.next_back() } {
        seen.push(id);
        front = !front;
    }
    assert_eq!(seen.len(), kids.len());
    for kid in &kids {
        assert!(seen.contains(kid), "child yielded at most once");
    }

    // A lone child is yielded once, not once per direction.
    let only = dom.create_element(qn("only"), Vec::new());
    dom.append(root, only).expect("only container");
    let text = dom.create_text("x");
    dom.append(only, text).expect("only child");
    let mut iter = dom.children(only).expect("only children");
    assert_eq!(iter.next(), Some(text));
    assert_eq!(iter.next_back(), None);

    // An empty run yields nothing from either end.
    let empty = dom.create_element(qn("empty"), Vec::new());
    dom.append(root, empty).expect("empty container");
    let mut iter = dom.children(empty).expect("empty children");
    assert_eq!(iter.next(), None);
    assert_eq!(iter.next_back(), None);
}
