use dom::{
    Attribute, Dom, LocalName, Namespace, NodeId, ParseFailKind, QualName, QuirksMode, SelectError,
    html_namespace, xml_namespace,
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
    let html = append_element(&mut dom, document, "html", vec![attr("id", "top")]);
    let body = append_element(&mut dom, html, "body", Vec::new());
    let main = append_element(
        &mut dom,
        body,
        "div",
        vec![attr("id", "main"), attr("class", "a b")],
    );
    let first = append_element(&mut dom, main, "p", vec![attr("class", "x")]);
    let first_text = dom.create_text("first");
    dom.append(first, first_text).expect("fixture text");
    let link = append_element(&mut dom, main, "a", vec![attr("href", "/next")]);
    let second = append_element(&mut dom, main, "p", vec![attr("class", "x y")]);
    let list = append_element(&mut dom, body, "ul", Vec::new());
    let one = append_element(&mut dom, list, "li", vec![attr("data-v", "Alpha beta")]);
    let two = append_element(&mut dom, list, "li", vec![attr("class", "picked")]);
    let three = append_element(&mut dom, list, "li", Vec::new());

    for (selector, expected) in [
        ("p", vec![first, second]),
        ("#main.a", vec![main]),
        ("[data-v~='beta']", vec![one]),
        ("div > p.x", vec![first, second]),
        ("p + a", vec![link]),
        ("li:nth-child(2)", vec![two]),
        ("li:not(.picked)", vec![one, three]),
        ("p:first-child, li:last-child", vec![first, three]),
        (":scope", vec![html]),
        (":scope > body", vec![body]),
    ] {
        assert_eq!(
            dom.select_all(document, selector).expect("valid selector"),
            expected,
            "{selector}"
        );
    }
    assert_eq!(
        dom.select_first(document, ".x").expect("valid selector"),
        Some(first)
    );
    assert!(dom.matches(second, "div > p.y").expect("matches"));
    assert!(!dom.matches(link, "p").expect("matches"));

    let text = dom.create_text("not an element");
    assert_eq!(dom.matches(text, "*"), Err(SelectError::NotAnElement));
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

    dom.detach(main).expect("detach");
    assert!(dom.select_all(document, "#main").expect("query").is_empty());
    dom.destroy(main).expect("destroy");
    assert_eq!(dom.select_all(main, "*"), Err(SelectError::StaleNode));
}

#[test]
fn html_state_pseudos_use_static_markup_and_document_state() {
    let mut dom = Dom::new();
    let document = dom.document();
    let html = append_element(
        &mut dom,
        document,
        "html",
        vec![attr("lang", "en-Latn-US"), attr("dir", "rtl")],
    );
    let body = append_element(&mut dom, html, "body", Vec::new());

    let fieldset = append_element(&mut dom, body, "fieldset", vec![attr("disabled", "")]);
    let legend = append_element(&mut dom, fieldset, "legend", Vec::new());
    let exempt = append_element(&mut dom, legend, "input", Vec::new());
    let disabled = append_element(&mut dom, fieldset, "input", Vec::new());
    let checked = append_element(
        &mut dom,
        body,
        "input",
        vec![attr("type", "checkbox"), attr("checked", "")],
    );
    let placeholder = append_element(
        &mut dom,
        body,
        "input",
        vec![attr("type", "text"), attr("placeholder", "hint")],
    );
    let select = append_element(&mut dom, body, "select", Vec::new());
    let default_option = append_element(&mut dom, select, "option", Vec::new());
    let other_option = append_element(&mut dom, select, "option", Vec::new());
    let progress = append_element(&mut dom, body, "progress", Vec::new());
    let custom = append_element(&mut dom, body, "my-widget", Vec::new());
    let link = append_element(&mut dom, body, "a", vec![attr("href", "/")]);

    assert_eq!(
        dom.select_all(document, ":disabled").expect("disabled"),
        vec![fieldset, disabled]
    );
    assert!(dom.matches(exempt, ":enabled").expect("enabled"));
    assert_eq!(
        dom.select_all(document, ":checked").expect("checked"),
        vec![checked, default_option]
    );
    assert!(!dom.matches(other_option, ":checked").expect("checked"));
    assert!(
        dom.matches(placeholder, ":placeholder-shown")
            .expect("placeholder")
    );
    assert!(
        dom.matches(progress, ":indeterminate")
            .expect("indeterminate")
    );
    assert!(!dom.matches(custom, ":defined").expect("defined"));
    assert!(dom.matches(link, ":any-link").expect("link"));
    assert!(dom.matches(checked, ":default").expect("default"));
    assert!(
        dom.matches(disabled, ":lang(\"en-*-US\")")
            .expect("language")
    );
    assert!(dom.matches(disabled, ":dir(rtl)").expect("direction"));

    dom.set_quirks_mode(QuirksMode::Quirks);
    let mixed = append_element(
        &mut dom,
        body,
        "div",
        vec![attr("id", "Mixed"), attr("class", "Loud")],
    );
    assert!(dom.matches(mixed, "#mixed.loud").expect("quirks"));
    dom.set_quirks_mode(QuirksMode::NoQuirks);
    assert!(!dom.matches(mixed, "#mixed.loud").expect("standards"));

    let foreign_lang = append_element(
        &mut dom,
        body,
        "p",
        vec![Attribute {
            name: QualName::new(None, xml_namespace(), LocalName::from("lang")),
            value: "fr".into(),
        }],
    );
    dom.set_document_language(Some("de".into()));
    assert!(dom.matches(foreign_lang, ":lang(fr)").expect("xml lang"));
    assert!(dom.matches(body, ":lang(en)").expect("ancestor lang"));

    let mut fallback = Dom::new();
    let fallback_document = fallback.document();
    let root = append_element(&mut fallback, fallback_document, "html", Vec::new());
    fallback.set_document_language(Some("de".into()));
    assert!(fallback.matches(root, ":lang(de)").expect("document lang"));
}
