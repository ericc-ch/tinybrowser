//! tinybrowser contracts for the selector API: error classification, stale
//! handles, and non-element refusal. Selector *semantics* live in WPT
//! (`css/selectors/`), not here (AGENTS.md).

use dom::{
    Attribute, Dom, LocalName, Namespace, NodeId, ParseFailKind, QualName, SelectError,
    html_namespace,
};

fn qn(local: &str) -> QualName {
    QualName::new(None, html_namespace(), LocalName::from(local))
}

fn attr(name: &str, value: &str) -> Attribute {
    Attribute {
        name: QualName::new(None, Namespace::from(""), LocalName::from(name)),
        value: value.into(),
    }
}

fn append_element(dom: &mut Dom, parent: NodeId, name: &str, attrs: Vec<Attribute>) -> NodeId {
    let element = dom.create_element(qn(name), attrs);
    dom.append(parent, element).expect("fixture append");
    element
}

#[test]
fn selector_queries_observe_the_public_tree_boundary() {
    let mut dom = Dom::new();
    let document = dom.document();
    let html = append_element(&mut dom, document, "html", Vec::new());
    let body = append_element(&mut dom, html, "body", Vec::new());
    let main = append_element(&mut dom, body, "div", vec![attr("id", "main")]);
    let text = dom.create_text("not an element");
    dom.append(main, text).expect("fixture text");

    // A malformed selector keeps its syntax class.
    for invalid in ["", "div[", ":dir(up)", ":frobnicate"] {
        assert!(matches!(
            dom.select_all(document, invalid),
            Err(SelectError::Syntax(_))
        ));
    }
    let Err(SelectError::Syntax(failure)) = dom.select_all(document, "div[") else {
        panic!("malformed selector must retain a syntax class");
    };
    assert_eq!(failure.kind(), ParseFailKind::MalformedInput);

    // A non-element has no `matches` semantics.
    assert_eq!(dom.matches(text, "*"), Err(SelectError::NotAnElement));

    // Detaching hides a subtree from document queries; destroying it makes
    // every old handle stale at once.
    dom.detach(main).expect("detach");
    assert!(dom.select_all(document, "#main").expect("query").is_empty());
    dom.destroy(main).expect("destroy");
    assert_eq!(dom.select_all(main, "*"), Err(SelectError::StaleNode));
    assert!(dom.select_all(document, "*").is_ok());
}
