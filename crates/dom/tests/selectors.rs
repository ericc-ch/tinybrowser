//! Selector-search behavior of the dom layer.
//!
//! Same boundary rule as `api.rs`: everything goes through `Dom`'s public
//! methods and handles. The fixture below stands in for a parsed page until
//! the `TreeSink` adapter exists.

use dom::{Attribute, Dom, LocalName, Namespace, NodeId, QualName, SelectError, html_namespace};

/// Qualified element name in the HTML namespace, no prefix.
fn qn(local: &str) -> QualName {
    QualName::new(None, html_namespace(), LocalName::from(local))
}

/// Qualified attribute name with no namespace (where class/id/href live).
fn an(local: &str) -> QualName {
    QualName::new(None, Namespace::from(""), LocalName::from(local))
}

fn attr(name: &str, value: &str) -> Attribute {
    Attribute {
        name: an(name),
        value: value.into(),
    }
}

/// A page standing in for parser output:
///
/// ```text
/// document
/// └─ html#top
///    └─ body
///       ├─ div#main.a.b
///       │  ├─ p.x          "first"
///       │  ├─ span[href]   (text)
///       │  └─ p.y.z        "second"
///       ├─ ul
///       │  ├─ li.one
///       │  ├─ li.two
///       │  └─ li.three.x
///       └─ em              "tail"
/// ```
struct Page {
    d: Dom,
    html: NodeId,
    body: NodeId,
    main: NodeId,
    first_p: NodeId,
    span: NodeId,
    second_p: NodeId,
    list: NodeId,
    li1: NodeId,
    li2: NodeId,
    li3: NodeId,
    em: NodeId,
}

fn build() -> Page {
    let mut d = Dom::new();
    let doc = d.document();

    let html = d.create_element(qn("html"), vec![attr("id", "top")]);
    d.append(doc, html).unwrap();
    let body = d.create_element(qn("body"), Vec::new());
    d.append(html, body).unwrap();

    let main = d.create_element(qn("div"), vec![attr("id", "main"), attr("class", "a b")]);
    d.append(body, main).unwrap();

    let first_p = d.create_element(qn("p"), vec![attr("class", "x")]);
    d.append(main, first_p).unwrap();
    let first_text = d.create_text("first");
    d.append(first_p, first_text).unwrap();

    let span = d.create_element(qn("span"), vec![attr("href", "https://example.com/page")]);
    d.append(main, span).unwrap();
    let link_text = d.create_text("link");
    d.append(span, link_text).unwrap();

    let second_p = d.create_element(qn("p"), vec![attr("class", "y z")]);
    d.append(main, second_p).unwrap();
    let second_text = d.create_text("second");
    d.append(second_p, second_text).unwrap();

    let list = d.create_element(qn("ul"), Vec::new());
    d.append(body, list).unwrap();
    let li1 = d.create_element(qn("li"), vec![attr("class", "one")]);
    let li2 = d.create_element(qn("li"), vec![attr("class", "two")]);
    let li3 = d.create_element(qn("li"), vec![attr("class", "three x")]);
    for &li in &[li1, li2, li3] {
        d.append(list, li).unwrap();
    }

    let em = d.create_element(qn("em"), Vec::new());
    d.append(body, em).unwrap();
    let tail_text = d.create_text("tail");
    d.append(em, tail_text).unwrap();

    // a comment sibling that must never match anything
    let comment = d.create_comment("note");
    d.append(html, comment).unwrap();

    Page {
        d,
        html,
        body,
        main,
        first_p,
        span,
        second_p,
        list,
        li1,
        li2,
        li3,
        em,
    }
}

#[test]
fn type_selector_matches_by_local_name_in_document_order() {
    let page = build();
    let hits = page.d.select_all(page.d.document(), "p").unwrap();
    assert_eq!(hits, vec![page.first_p, page.second_p]);
}

#[test]
fn universal_selects_every_element_but_nothing_else() {
    let page = build();
    let hits = page.d.select_all(page.d.document(), "*").unwrap();
    assert_eq!(
        hits,
        vec![
            page.html,
            page.body,
            page.main,
            page.first_p,
            page.span,
            page.second_p,
            page.list,
            page.li1,
            page.li2,
            page.li3,
            page.em,
        ],
        "every element in document order"
    );
}

#[test]
fn class_and_id_match_token_lists() {
    let page = build();
    assert_eq!(
        page.d.select_all(page.d.document(), "#main").unwrap(),
        vec![page.main]
    );
    assert_eq!(
        page.d.select_all(page.d.document(), ".b").unwrap(),
        vec![page.main]
    );
    assert_eq!(
        page.d.select_all(page.d.document(), ".x").unwrap(),
        vec![page.first_p, page.li3]
    );
    // multiple classes on one element
    assert_eq!(
        page.d.select_first(page.d.document(), ".y.z").unwrap(),
        Some(page.second_p)
    );
}

#[test]
fn attribute_operators_follow_css_rules() {
    let page = build();

    assert_eq!(
        page.d.select_first(page.d.document(), "[href]").unwrap(),
        Some(page.span)
    );
    assert_eq!(
        page.d
            .select_first(page.d.document(), r#"[href="https://example.com/page"]"#)
            .unwrap(),
        Some(page.span)
    );
    assert!(page.d.matches(page.span, r#"[href^="https://"]"#).unwrap());
    assert!(page.d.matches(page.span, r#"[href$="/page"]"#).unwrap());
    assert!(page.d.matches(page.span, "[href*=\"example\"]").unwrap());

    // ~= token match and |= dash-prefix match need purpose-built fixtures
    let mut d = Dom::new();
    let doc = d.document();
    let rel = d.create_element(qn("a"), vec![attr("rel", "tag nofollow")]);
    let lang = d.create_element(qn("p"), vec![attr("lang", "en-US")]);
    d.append(doc, rel).unwrap();
    d.append(doc, lang).unwrap();

    assert!(d.matches(rel, r#"[rel~="nofollow"]"#).unwrap());
    assert!(!d.matches(rel, r#"[rel~="follow"]"#).unwrap());
    assert!(d.matches(lang, "[lang|=\"en\"]").unwrap());
    assert!(!d.matches(lang, "[lang|=\"fr\"]").unwrap());
    // `lang` is on the HTML list of value-case-insensitive attributes...
    assert!(d.matches(lang, "[LANG=\"en-us\"]").unwrap());
    // ...`href` is not: exact compare misses, the `i` flag hits
    assert!(
        !page
            .d
            .matches(page.span, r#"[href="HTTPS://EXAMPLE.COM/PAGE"]"#)
            .unwrap()
    );
    assert!(
        page.d
            .matches(page.span, r#"[href="HTTPS://EXAMPLE.COM/PAGE" i]"#)
            .unwrap()
    );
}

#[test]
fn combinators_respect_tree_structure() {
    let page = build();
    let doc = page.d.document();

    assert_eq!(
        page.d.select_all(doc, "ul > li").unwrap(),
        vec![page.li1, page.li2, page.li3]
    );
    assert_eq!(
        page.d.select_all(doc, "body > p").unwrap(),
        Vec::<NodeId>::new(),
        "the paragraphs sit under div, not body"
    );
    // adjacent sibling: text nodes are skipped
    assert_eq!(
        page.d.select_first(doc, "span + p").unwrap(),
        Some(page.second_p)
    );
    // subsequent sibling across the ul subtree boundary
    assert_eq!(page.d.select_first(doc, "div ~ em").unwrap(), Some(page.em));
    // ancestors above the scope stay visible to combinators
    assert_eq!(
        page.d.select_first(page.main, "body > *").unwrap(),
        None,
        "div's descendants cannot have body as parent"
    );
}

#[test]
fn structural_pseudos_work_through_the_engine() {
    let page = build();
    let doc = page.d.document();

    assert_eq!(page.d.select_first(doc, ":root").unwrap(), Some(page.html));
    assert_eq!(
        page.d.select_first(page.main, "p:first-child").unwrap(),
        Some(page.first_p)
    );
    assert_eq!(
        page.d.select_first(page.main, "p:last-child").unwrap(),
        Some(page.second_p)
    );
    assert_eq!(
        page.d.select_all(page.list, "li:nth-child(2n+1)").unwrap(),
        vec![page.li1, page.li3]
    );
    assert_eq!(
        page.d.select_all(doc, "li:not(.x)").unwrap(),
        vec![page.li1, page.li2]
    );
    assert_eq!(
        page.d.select_all(doc, ":is(h1, em)").unwrap(),
        vec![page.em]
    );
    assert_eq!(
        page.d.select_all(doc, "div:has(> span)").unwrap(),
        vec![page.main]
    );
}

#[test]
fn empty_matches_only_truly_empty_elements() {
    let mut d = Dom::new();
    let doc = d.document();
    let hollow = d.create_element(qn("div"), Vec::new());
    let commented = d.create_element(qn("div"), Vec::new());
    d.append(doc, hollow).unwrap();
    d.append(doc, commented).unwrap();
    let note = d.create_comment("only a comment");
    d.append(commented, note).unwrap();

    assert!(d.matches(hollow, ":empty").unwrap());
    assert!(
        d.matches(commented, ":empty").unwrap(),
        "comments do not count"
    );

    let filled = d.create_element(qn("div"), Vec::new());
    let letter = d.create_text("x");
    d.append(filled, letter).unwrap();
    assert!(!d.matches(filled, ":empty").unwrap());
}

#[test]
fn detached_subtrees_are_invisible_from_the_document() {
    let mut d = Dom::new();
    let doc = d.document();
    let keep = d.create_element(qn("section"), Vec::new());
    let orphan = d.create_element(qn("aside"), Vec::new());
    d.append(doc, keep).unwrap();
    d.append(keep, orphan).unwrap();

    // while attached, the query sees it
    assert_eq!(d.select_all(doc, "aside").unwrap(), vec![orphan]);

    // after detaching, the whole subtree vanishes from document queries...
    d.detach(orphan).unwrap();
    assert_eq!(d.select_all(doc, "*").unwrap(), vec![keep]);

    // ...yet the subtree itself stays intact: the orphan is still live and
    // matchable (a scoped query excludes its own scope node, so `*` from the
    // childless orphan is empty — that is correct, not a miss)
    let note = d.create_text("island");
    d.append(orphan, note).unwrap();
    assert!(d.contains(orphan));
    assert_eq!(d.parent(note), Some(orphan));
    assert_eq!(
        d.select_all(orphan, "*").unwrap(),
        Vec::<NodeId>::new(),
        "scope node excluded; text nodes never match"
    );
    assert!(d.matches(orphan, "aside").unwrap());
}

#[test]
fn the_scope_node_is_never_its_own_hit() {
    let page = build();
    let hits = page.d.select_all(page.main, "*").unwrap();
    assert_eq!(
        hits,
        vec![page.first_p, page.span, page.second_p],
        "descendants only, document order"
    );
    // but the scope itself is still matchable through `matches`
    assert!(page.d.matches(page.main, "div#main").unwrap());
}

#[test]
fn results_are_document_ordered_regardless_of_selector_order() {
    let page = build();
    let a = page.d.select_all(page.d.document(), "em, #main").unwrap();
    let b = page.d.select_all(page.d.document(), "#main, em").unwrap();
    assert_eq!(a, vec![page.main, page.em]);
    assert_eq!(a, b);
}

#[test]
fn select_first_returns_first_hit_or_none() {
    let page = build();
    assert_eq!(
        page.d.select_first(page.d.document(), "li").unwrap(),
        Some(page.li1)
    );
    assert_eq!(page.d.select_first(page.d.document(), "h1").unwrap(), None);
}

#[test]
fn stale_handles_are_reported_not_ignored() {
    let mut page = build();
    let ghost_target = page.em;
    page.d.destroy(ghost_target).unwrap();

    assert_eq!(
        page.d.select_all(ghost_target, "*"),
        Err(SelectError::StaleNode)
    );
    assert_eq!(
        page.d.select_first(ghost_target, "*"),
        Err(SelectError::StaleNode)
    );
    assert_eq!(
        page.d.matches(ghost_target, "em"),
        Err(SelectError::StaleNode)
    );
}

#[test]
fn matches_rejects_non_element_nodes() {
    let mut d = Dom::new();
    let text = d.create_text("plain");
    assert_eq!(d.matches(text, "*"), Err(SelectError::NotAnElement));

    // a destroyed handle still reports staleness, not kind confusion
    d.destroy(text).unwrap();
    assert_eq!(d.matches(text, "*"), Err(SelectError::StaleNode));
}

#[test]
fn bad_selectors_are_syntax_errors_with_readable_messages() {
    let page = build();
    for selector in ["", "  ", "div >", "..x", "p::before", ":hover"] {
        let error = page
            .d
            .select_all(page.d.document(), selector)
            .expect_err(selector);
        match error {
            SelectError::Syntax(message) => {
                assert!(
                    !message.is_empty(),
                    "{selector:?} produced an empty message"
                );
            }
            other => panic!("{selector:?}: expected syntax error, got {other:?}"),
        }
    }
}

#[test]
fn pseudo_element_queries_are_refused_like_browsers_do() {
    let page = build();
    assert!(matches!(
        page.d.select_all(page.d.document(), "p::before"),
        Err(SelectError::Syntax(_))
    ));
}

#[test]
fn type_selectors_without_namespace_qualifier_match_any_namespace() {
    let mut d = Dom::new();
    let svg_circle = d.create_element(
        QualName::new(
            None,
            Namespace::from("http://www.w3.org/2000/svg"),
            LocalName::from("circle"),
        ),
        Vec::new(),
    );
    d.append(d.document(), svg_circle).unwrap();

    assert_eq!(
        d.select_all(d.document(), "circle").unwrap(),
        vec![svg_circle]
    );
    assert_eq!(
        d.select_all(d.document(), "*|circle").unwrap(),
        vec![svg_circle]
    );
}

#[test]
fn hand_built_mixed_case_names_match_like_tokenized_trees_would() {
    let mut d = Dom::new();
    let doc = d.document();
    let shouty = d.create_element(
        qn("BUTTON"),
        vec![Attribute {
            name: an("CLASS"),
            value: "Big".into(),
        }],
    );
    d.append(doc, shouty).unwrap();

    // tag and attribute NAME casing is normalized away...
    assert!(d.matches(shouty, "button").unwrap());
    assert!(d.matches(shouty, ".Big").unwrap());
    // ...but class VALUE casing never is, in standards mode
    assert!(!d.matches(shouty, ".big").unwrap());
}
