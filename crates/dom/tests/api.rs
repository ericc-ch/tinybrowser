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
            assert!(dom.get(handle).is_none(), "dead node {index} resolves");
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
                .copied()
                .collect::<Vec<_>>(),
            expected_children,
            "children {index}"
        );
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
            .copied()
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
        dom.get(first).map(|node| node.kind()),
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
