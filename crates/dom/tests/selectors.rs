//! Selector-search behavior of the dom layer.
//!
//! Same boundary rule as `api.rs`: everything goes through `Dom`'s public
//! methods and handles. The fixture below stands in for a parsed page until
//! the `TreeSink` adapter exists.

use dom::{
    Attribute, Dom, LocalName, Namespace, NodeId, ParseFailKind, QualName, QuirksMode, SelectError,
    html_namespace,
};

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

/// A `<body>` under the document: the legal landing spot for fixtures that
/// hang several elements off it (a document takes one element child only).
fn body_under(d: &mut Dom) -> NodeId {
    let body = d.create_element(qn("body"), Vec::new());
    d.append(d.document(), body).unwrap();
    body
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
    let _doc = d.document();
    let body = body_under(&mut d);
    let rel = d.create_element(qn("a"), vec![attr("rel", "tag nofollow")]);
    let lang = d.create_element(qn("p"), vec![attr("lang", "en-US")]);
    d.append(body, rel).unwrap();
    d.append(body, lang).unwrap();

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
    let _doc = d.document();
    let body = body_under(&mut d);
    let hollow = d.create_element(qn("div"), Vec::new());
    let commented = d.create_element(qn("div"), Vec::new());
    d.append(body, hollow).unwrap();
    d.append(body, commented).unwrap();
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
    // childless orphan is empty; that is correct, not a miss)
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
fn nth_child_of_indexes_the_matching_subset() {
    let mut d = Dom::new();
    let doc = d.document();
    let ul = d.create_element(qn("ul"), Vec::new());
    d.append(doc, ul).unwrap();

    // ul > li.a, li.b, p.x, li.x, li.a  (positions 1-5)
    let a1 = d.create_element(qn("li"), vec![attr("class", "a")]);
    let b2 = d.create_element(qn("li"), vec![attr("class", "b")]);
    let px = d.create_element(qn("p"), vec![attr("class", "x")]);
    let lx = d.create_element(qn("li"), vec![attr("class", "x")]);
    let a5 = d.create_element(qn("li"), vec![attr("class", "a")]);
    for id in [a1, b2, px, lx, a5] {
        d.append(ul, id).unwrap();
    }

    // `of S` renumbers within the matching subset: the two `.x` elements sit
    // at overall positions 3 and 4, so the *second* one is li.x, not b2.
    assert_eq!(d.select_first(ul, ":nth-child(2 of .x)").unwrap(), Some(lx));
    assert_eq!(
        d.select_first(ul, "li:nth-child(1 of li.a)").unwrap(),
        Some(a1)
    );
    assert_eq!(
        d.select_first(ul, "li:nth-child(2 of li.a)").unwrap(),
        Some(a5)
    );

    // plain nth-child still counts every element sibling
    assert_eq!(d.select_first(ul, ":nth-child(3)").unwrap(), Some(px));
}

#[test]
fn link_pseudo_class_matches_html_links_under_either_spelling() {
    let mut d = Dom::new();
    let doc = d.document();
    let body = body_under(&mut d);

    let anchor = d.create_element(qn("a"), vec![attr("href", "https://example.net")]);
    let bare_anchor = d.create_element(qn("a"), Vec::new());
    let shouty = d.create_element(
        QualName::new(None, html_namespace(), LocalName::from("AREA")),
        vec![attr("href", "#top")],
    );
    let link_el = d.create_element(qn("link"), vec![attr("href", "/style.css")]);
    // href on a non-link element does not make it :link
    let div_href = d.create_element(qn("div"), vec![attr("href", "/not-a-link")]);
    for id in [anchor, bare_anchor, shouty, link_el, div_href] {
        d.append(body, id).unwrap();
    }

    let hits = d.select_all(doc, ":link").unwrap();
    assert_eq!(hits, vec![anchor, shouty, link_el]);

    // CSS keywords are case-insensitive
    assert_eq!(
        d.select_all(doc, ":LINK").unwrap(),
        hits,
        "keyword spelling changed the result"
    );
}

#[test]
fn user_state_pseudo_classes_parse_but_never_match() {
    let page = build();

    // The runtime-context set: pointer, keyboard focus, browsing history,
    // URL fragment, numeric range validation, autofill activity; none
    // exist in a headless tree, and a fresh page in a real browser answers
    // no matches for all of them. (`:indeterminate` and `:default` have
    // statically knowable subsets and are covered by their own tests.)
    for selector in [
        ":hover",
        ":active",
        ":focus",
        ":visited",
        ":focus-within",
        ":focus-visible",
        ":target",
        ":in-range",
        ":out-of-range",
        ":autofill",
    ] {
        assert_eq!(
            page.d.select_all(page.d.document(), selector).unwrap(),
            Vec::new(),
            "{selector} matched a static headless tree"
        );
    }
    // ...and they parse inside compounds too.
    assert!(
        !page.d.matches(page.span, "a:hover").unwrap(),
        "compound with a state pseudo must not leak through"
    );
}

#[test]
fn unknown_state_pseudo_classes_still_refuse() {
    let page = build();
    assert!(matches!(
        page.d.select_all(page.d.document(), ":frobnicate"),
        Err(SelectError::Syntax(_))
    ));
}

#[test]
fn quirks_mode_flips_class_and_id_case_rules() {
    let mut page = build();

    // The fixture's div is `id="main" class="a b"`; exact spellings match
    // under every mode.
    assert!(page.d.matches(page.main, ".b").unwrap());

    // Standards and limited-quirks keep class/id values case-sensitive.
    assert!(!page.d.matches(page.main, ".B").unwrap());
    page.d.set_quirks_mode(QuirksMode::LimitedQuirks);
    assert!(!page.d.matches(page.main, "#MAIN").unwrap());

    // Full quirks applies the WHATWG id/class quirk: ASCII-insensitive.
    page.d.set_quirks_mode(QuirksMode::Quirks);
    assert!(page.d.matches(page.main, ".B").unwrap());
    assert!(page.d.matches(page.main, "#MAIN").unwrap());
}

#[test]
fn bad_selectors_are_syntax_errors_with_readable_messages() {
    let page = build();
    // Unknown pseudo-element names refuse like unknown pseudo-classes;
    // KNOWN ones parse and match nothing (see the pseudo-elements test).
    for selector in ["", "  ", "div >", "..x", ":frobnicate", "::frobnicate"] {
        let error = page
            .d
            .select_all(page.d.document(), selector)
            .expect_err(selector);
        match error {
            SelectError::Syntax(fail) => {
                assert!(
                    !fail.to_string().is_empty(),
                    "{selector:?} produced an empty message"
                );
            }
            other => panic!("{selector:?}: expected syntax error, got {other:?}"),
        }
    }
}

#[test]
fn syntax_errors_carry_a_machine_readable_class() {
    let page = build();
    let class_of = |selector: &str| match page.d.select_all(page.d.document(), selector) {
        Err(SelectError::Syntax(fail)) => fail.kind(),
        other => panic!("{selector:?}: expected syntax error, got {other:?}"),
    };

    assert_eq!(class_of(""), ParseFailKind::EmptySelector);
    assert_eq!(class_of("div >"), ParseFailKind::DanglingCombinator);
    assert_eq!(class_of(":frobnicate"), ParseFailKind::UnsupportedPseudo);
    assert_eq!(class_of("::frobnicate"), ParseFailKind::UnsupportedPseudo);
    assert_eq!(
        class_of("svg|circle"),
        ParseFailKind::UnknownNamespacePrefix
    );

    // the class travels with the text, not instead of it
    let Err(SelectError::Syntax(fail)) = page.d.select_all(page.d.document(), ":frobnicate") else {
        unreachable!("expected a syntax error for :frobnicate");
    };
    assert_eq!(
        fail.to_string(),
        "unsupported pseudo-class or element `frobnicate`",
        "message drifted from its class"
    );
}

/// Known pseudo-elements parse like browsers' and match nothing: real
/// `querySelectorAll("p::before")` returns an empty list, never throws
/// (MDN, "querySelectorAll"). Unknown names are still syntax errors.
#[test]
fn pseudo_elements_parse_but_never_match() {
    let page = build();
    for selector in ["p::before", "p::after", "p::first-line", "::selection"] {
        assert_eq!(
            page.d.select_all(page.d.document(), selector).unwrap(),
            Vec::<NodeId>::new(),
            "{selector} matched a tree that builds no boxes"
        );
    }
    // compound with a known pseudo-element also just matches nothing
    assert!(!page.d.matches(page.first_p, "p::marker").unwrap());
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

// ── form-control UI states (HTML §pseudo-classes) ───────────────────────────

/// The attribute-derived UI states match exactly what static markup
/// determines; audit finding L7: these used to throw `SyntaxError` where
/// every browser returns matches.
#[test]
fn form_states_match_from_static_markup() {
    let mut d = Dom::new();
    let body = body_under(&mut d);

    let text = d.create_element(qn("input"), vec![attr("type", "text")]);
    let locked = d.create_element(
        qn("input"),
        vec![attr("type", "text"), attr("readonly", "")],
    );
    let off = d.create_element(qn("input"), vec![attr("disabled", "")]);
    let mandatory = d.create_element(qn("input"), vec![attr("required", "")]);
    let box_checked = d.create_element(
        qn("input"),
        vec![attr("type", "checkbox"), attr("checked", "")],
    );
    let box_plain = d.create_element(qn("input"), vec![attr("type", "radio")]);
    let hinted = d.create_element(qn("textarea"), vec![attr("placeholder", "hi")]);
    let filled_hint = d.create_element(qn("textarea"), vec![attr("placeholder", "hi")]);
    let filler = d.create_text("typed");
    d.append(filled_hint, filler).unwrap();
    let option_picked = d.create_element(qn("option"), vec![attr("selected", "")]);
    let plain_div = d.create_element(qn("div"), Vec::new());
    for id in [
        text,
        locked,
        off,
        mandatory,
        box_checked,
        box_plain,
        hinted,
        filled_hint,
        option_picked,
        plain_div,
    ] {
        d.append(body, id).unwrap();
    }
    let hits = |selector: &str| d.select_all(body, selector).unwrap();

    // disableable population only: a div is neither enabled nor disabled
    assert_eq!(
        hits(":enabled"),
        vec![
            text,
            locked,
            mandatory,
            box_checked,
            box_plain,
            hinted,
            filled_hint,
            option_picked,
        ]
    );
    assert_eq!(hits(":disabled"), vec![off]);
    // checked needs the right input type or a selected option
    assert_eq!(hits(":checked"), vec![box_checked, option_picked]);
    assert_eq!(hits(":required"), vec![mandatory]);
    assert_eq!(hits(":optional").len(), 7); // inputs/selects/textareas minus required
    // readonly (or disabled) makes an input read-only; other inputs and
    // textareas are read-write; non-form elements are neither: the Chrome
    // reading of Selectors 4 §rw-pseudos
    assert_eq!(hits(":read-only"), vec![locked, off]);
    assert!(hits(":read-write").contains(&text));
    assert!(!hits(":read-write").contains(&locked));
    assert!(!hits(":read-only").contains(&plain_div));
    assert!(!hits(":read-write").contains(&plain_div));
    // placeholder shows while the value is empty
    assert_eq!(hits(":placeholder-shown"), vec![hinted]);
}

/// Disability inherits per HTML §4.15 (*Disabled elements*): EVERY form
/// control under a disabled `<fieldset>` is disabled (not just options);
/// option/optgroup additionally answer to their nearest disabled
/// `<select>`, and an `option` answers to its directly enclosing disabled
/// `<optgroup>` (§4.10.11). The first-`legend` exemption stays an unmodeled
/// approximation (see `state::is_disabled`); nothing here exercises legends.
#[test]
fn fieldset_and_select_disability_inherits() {
    let mut d = Dom::new();
    let body = body_under(&mut d);

    let fieldset = d.create_element(qn("fieldset"), vec![attr("disabled", "")]);
    d.append(body, fieldset).unwrap();
    let inside = d.create_element(qn("input"), Vec::new());
    d.append(fieldset, inside).unwrap();
    // not a form control: out of both populations whatever its ancestry
    let bystander = d.create_element(qn("div"), Vec::new());
    d.append(fieldset, bystander).unwrap();

    let dead_select = d.create_element(qn("select"), vec![attr("disabled", "")]);
    d.append(body, dead_select).unwrap();
    let group = d.create_element(qn("optgroup"), Vec::new());
    d.append(dead_select, group).unwrap();
    let chosen = d.create_element(qn("option"), Vec::new());
    d.append(group, chosen).unwrap();

    let live_select = d.create_element(qn("select"), Vec::new());
    d.append(body, live_select).unwrap();
    let free_option = d.create_element(qn("option"), Vec::new());
    d.append(live_select, free_option).unwrap();

    // …and a live select whose optgroup is disabled: its direct option
    // children go with it.
    let dead_group = d.create_element(qn("optgroup"), vec![attr("disabled", "")]);
    d.append(live_select, dead_group).unwrap();
    let sheltered = d.create_element(qn("option"), Vec::new());
    d.append(dead_group, sheltered).unwrap();

    let hits = |selector: &str| d.select_all(body, selector).unwrap();

    // The reviewed code walked ancestors only for option/optgroup, so this
    // input answered `:enabled` while sitting in a dead fieldset (R3-3).
    assert!(hits(":disabled").contains(&inside));
    assert!(!hits(":enabled").contains(&inside));
    assert!(
        hits(":disabled").contains(&chosen),
        "option inherits its nearest select's disability"
    );
    assert!(!hits(":enabled").contains(&chosen));
    assert!(hits(":enabled").contains(&free_option));
    assert!(!hits(":disabled").contains(&free_option));
    assert!(
        hits(":disabled").contains(&sheltered),
        "option inherits a directly enclosing disabled optgroup's disability"
    );
    assert!(!hits(":enabled").contains(&sheltered));
    assert!(!hits(":disabled").contains(&bystander));
    assert!(!hits(":enabled").contains(&bystander));
}

/// Selectedness has a static default (HTML *concept-option-selectedness*):
/// in a select without `multiple`, the first option of its list of options
/// is selected when nothing in that list carries `selected`, and the list
/// flattens `optgroup`s, so wrapped options answer like bare ones. Fresh
/// parsed pages therefore match `:checked` exactly as browsers do
/// (subagent review R3-4; the optgroup shapes were missed by that round's
/// fixtures and caught by the pass-4 review probes).
#[test]
fn checked_defaults_apply_without_selected_attributes() {
    let mut d = Dom::new();
    let body = body_under(&mut d);

    // No `selected` anywhere: the first option is the checked one.
    let single = d.create_element(qn("select"), Vec::new());
    d.append(body, single).unwrap();
    let s1 = d.create_element(qn("option"), Vec::new());
    let s2 = d.create_element(qn("option"), Vec::new());
    d.append(single, s1).unwrap();
    d.append(single, s2).unwrap();

    // `multiple` has no default selectedness.
    let multi = d.create_element(qn("select"), vec![attr("multiple", "")]);
    d.append(body, multi).unwrap();
    let m1 = d.create_element(qn("option"), Vec::new());
    let m2 = d.create_element(qn("option"), Vec::new());
    d.append(multi, m1).unwrap();
    d.append(multi, m2).unwrap();

    // An explicit `selected` beats position.
    let picked_select = d.create_element(qn("select"), Vec::new());
    d.append(body, picked_select).unwrap();
    let p1 = d.create_element(qn("option"), Vec::new());
    let p2 = d.create_element(qn("option"), vec![attr("selected", "")]);
    d.append(picked_select, p1).unwrap();
    d.append(picked_select, p2).unwrap();

    // The option list flattens optgroups: a lone wrapped option is the
    // default pick…
    let grouped = d.create_element(qn("select"), Vec::new());
    d.append(body, grouped).unwrap();
    let lone_group = d.create_element(qn("optgroup"), Vec::new());
    d.append(grouped, lone_group).unwrap();
    let g_lone = d.create_element(qn("option"), Vec::new());
    d.append(lone_group, g_lone).unwrap();

    // …and an explicit pick inside a group suppresses the default for its
    // bare sibling, both facts at once in this shape.
    let mixed = d.create_element(qn("select"), Vec::new());
    d.append(body, mixed).unwrap();
    let pick_group = d.create_element(qn("optgroup"), Vec::new());
    d.append(mixed, pick_group).unwrap();
    let g_pick = d.create_element(qn("option"), vec![attr("selected", "")]);
    d.append(pick_group, g_pick).unwrap();
    let bare = d.create_element(qn("option"), Vec::new());
    d.append(mixed, bare).unwrap();

    let hits = d.select_all(body, ":checked").unwrap();
    assert_eq!(
        hits,
        vec![s1, p2, g_lone, g_pick],
        "first-option default plus explicit pick, across optgroup wrapping"
    );
}

/// A placeholder is *shown* only where one can render and only while the
/// control's value is empty
/// (<https://html.spec.whatwg.org/#attr-input-placeholder>; subagent review
/// R3-11: a checkbox carrying `placeholder` used to match).
#[test]
fn placeholder_shown_respects_input_types_and_values() {
    let mut d = Dom::new();
    let body = body_under(&mut d);

    let shown = d.create_element(
        qn("input"),
        vec![attr("type", "text"), attr("placeholder", "hi")],
    );
    let filled = d.create_element(
        qn("input"),
        vec![
            attr("type", "text"),
            attr("placeholder", "hi"),
            attr("value", "typed"),
        ],
    );
    let checkbox = d.create_element(
        qn("input"),
        vec![attr("type", "checkbox"), attr("placeholder", "hi")],
    );
    let searchable = d.create_element(
        qn("input"),
        vec![attr("type", "SEARCH"), attr("placeholder", "hi")],
    );
    let area_empty = d.create_element(qn("textarea"), vec![attr("placeholder", "hi")]);
    let area_filled = d.create_element(qn("textarea"), vec![attr("placeholder", "hi")]);
    let typed = d.create_text("words");
    d.append(area_filled, typed).unwrap();
    for id in [shown, filled, checkbox, searchable, area_empty, area_filled] {
        d.append(body, id).unwrap();
    }

    let hits = d.select_all(body, ":placeholder-shown").unwrap();
    assert_eq!(
        hits,
        vec![shown, searchable, area_empty],
        "empty-capable inputs plus an empty textarea"
    );
}

/// Statically knowable subsets of two states whose full semantics need a
/// forms model: `:default` answers default-checked/-selected controls (the
/// default-submit-button clause stays deferred), `:indeterminate` answers a
/// `progress` without a value attribute (radio groups stay deferred);
/// see `state::is_default` / `state::is_indeterminate` (R3-10).
#[test]
fn default_and_indeterminate_answer_their_static_subsets() {
    let mut d = Dom::new();
    let body = body_under(&mut d);

    let box_checked = d.create_element(
        qn("input"),
        vec![attr("type", "checkbox"), attr("checked", "")],
    );
    let box_plain = d.create_element(qn("input"), vec![attr("type", "checkbox")]);
    let submit = d.create_element(qn("input"), vec![attr("type", "submit")]);
    let picked = d.create_element(qn("option"), vec![attr("selected", "")]);
    let bare = d.create_element(qn("option"), Vec::new());
    let loading = d.create_element(qn("progress"), Vec::new());
    let measured = d.create_element(qn("progress"), vec![attr("value", "50")]);
    for id in [
        box_checked,
        box_plain,
        submit,
        picked,
        bare,
        loading,
        measured,
    ] {
        d.append(body, id).unwrap();
    }

    let defaults = d.select_all(body, ":default").unwrap();
    assert_eq!(
        defaults,
        vec![box_checked, picked],
        "default submit buttons need the forms model; everything else is markup"
    );

    let indeterminate = d.select_all(body, ":indeterminate").unwrap();
    assert_eq!(indeterminate, vec![loading]);
}

/// `:any-link` shares the hyperlink rule; both hit SVG `<a href>` too
/// (audit finding L9: browsers match hyperlinks in any namespace).
#[test]
fn any_link_and_link_cover_html_and_svg() {
    let mut d = Dom::new();
    let doc = d.document();
    let body = body_under(&mut d);

    let svg_ns = Namespace::from("http://www.w3.org/2000/svg");
    let svg_link = d.create_element(
        QualName::new(None, svg_ns.clone(), LocalName::from("a")),
        vec![attr("href", "#top")],
    );
    let html_link = d.create_element(qn("a"), vec![attr("href", "/page")]);
    let svg_span = d.create_element(
        QualName::new(None, svg_ns, LocalName::from("tspan")),
        Vec::new(),
    );
    for id in [svg_link, html_link, svg_span] {
        d.append(body, id).unwrap();
    }

    for selector in [":link", ":any-link"] {
        assert_eq!(
            d.select_all(doc, selector).unwrap(),
            vec![svg_link, html_link],
            "{selector} should treat svg <a> as a hyperlink"
        );
    }
    // :visited parses beside :link and misses everything, as everywhere
    assert!(d.select_all(doc, ":visited").unwrap().is_empty());
}

/// `:defined` (<https://html.spec.whatwg.org/#selector-defined>): true for
/// everything except valid-but-unregistered custom-element names: HTML
/// names containing `-`, minus the reserved hyphenated set. That set is
/// fully static here, so the old "always true" shortcut was a lie for
/// `<my-widget>`-shaped markup (subagent review R3-9).
#[test]
fn defined_covers_everything_but_unregistered_custom_names() {
    let mut d = Dom::new();
    let doc = d.document();
    let body = body_under(&mut d);

    let widget = d.create_element(qn("my-widget"), Vec::new());
    let shouty_widget = d.create_element(qn("MY-WIDGET"), Vec::new());
    let reserved = d.create_element(qn("font-face"), Vec::new());
    let reserved_svg = d.create_element(
        QualName::new(
            None,
            Namespace::from("http://www.w3.org/2000/svg"),
            LocalName::from("annotation-xml"),
        ),
        Vec::new(),
    );
    let plain = d.create_element(qn("div"), Vec::new());
    for id in [widget, shouty_widget, reserved, reserved_svg, plain] {
        d.append(body, id).unwrap();
    }

    assert!(!d.matches(widget, ":defined").unwrap());
    // hand-built uppercase custom names are the same custom name
    assert!(!d.matches(shouty_widget, ":defined").unwrap());
    // reserved hyphenated SVG/HTML legacy names are not custom elements
    assert!(d.matches(reserved, ":defined").unwrap());
    // foreign elements are always defined, even with reserved-ish names
    assert!(d.matches(reserved_svg, ":defined").unwrap());
    assert!(d.matches(plain, ":defined").unwrap());

    // and the split shows up in queries: only the two widgets miss
    // (body, reserved, reserved-svg, plain are all defined)
    let hits = d.select_all(doc, ":defined").unwrap();
    assert_eq!(hits.len(), 4);
}

/// `:lang()` / `:dir()` inherit from ancestors per Selectors 4; keywords
/// are case-insensitive as everywhere else in CSS.
#[test]
fn lang_and_dir_inherit_from_ancestors() {
    let mut d = Dom::new();
    let doc = d.document();
    let html = d.create_element(qn("html"), vec![attr("lang", "en-US")]);
    d.append(doc, html).unwrap();
    let rtl = d.create_element(qn("div"), vec![attr("dir", "RTL")]);
    d.append(html, rtl).unwrap();
    let inner = d.create_element(qn("p"), Vec::new());
    d.append(rtl, inner).unwrap();

    // range matching: en-US satisfies :lang(en), not :lang(fr)
    assert!(d.matches(inner, ":lang(en)").unwrap());
    assert!(d.matches(inner, ":LANG(en-US)").unwrap());
    assert!(!d.matches(inner, ":lang(fr)").unwrap());

    // dir inherits from the nearest HTML ancestor carrying one; documents
    // without any dir default to ltr
    assert!(d.matches(rtl, ":dir(rtl)").unwrap());
    assert!(!d.matches(rtl, ":dir(ltr)").unwrap());
    assert!(d.matches(inner, ":dir(RTL)").unwrap());

    let other = d.create_element(qn("aside"), Vec::new());
    d.append(html, other).unwrap();
    assert!(d.matches(other, ":dir(ltr)").unwrap());

    // a bad direction is a syntax error, like browsers throw for :dir(up)
    assert!(matches!(
        d.select_all(doc, ":dir(up)"),
        Err(SelectError::Syntax(_))
    ));
}

/// `:lang()` argument grammar and RFC 4647 §3.3.2 extended filtering
/// (Selectors 4 §lang-pseudo): comma-separated ranges, `*` wildcard subtags,
/// insignificant whitespace, and no way to make a malformed attribute
/// value crash the matcher (subagent review R3-1: byte slicing used to
/// panic on multibyte `lang` values).
#[test]
fn lang_ranges_follow_extended_filtering() {
    let mut d = Dom::new();
    let doc = d.document();
    let html = d.create_element(qn("html"), vec![attr("lang", "en-Latn-US")]);
    d.append(doc, html).unwrap();
    let p = d.create_element(qn("p"), Vec::new());
    d.append(html, p).unwrap();
    let hits = |selector: &str| d.select_all(doc, selector).unwrap().contains(&p);

    // Extended filtering is positional: range subtags correspond to tag
    // subtags in order, `*` consumes exactly one, trailing tag subtags are
    // free specificity.
    assert!(hits(":lang(en)"));
    assert!(hits(":lang(en-latn)"), "trailing -US is free specificity");
    assert!(!hits(":lang(en-us)"), "-Latn cannot be skipped over");
    assert!(
        hits(":lang(EN-LATN-US)"),
        "ranges compare case-insensitively"
    );

    // Wildcards. Compound ones use the quoted spelling (Selectors 4's own
    // example is `E:lang(sr, "*-Cyrl")`) because CSS lexes a bare `*` as a
    // delimiter token separate from the following subtags; the bare `*`
    // form is reserved for the all-matching wildcard.
    assert!(hits(r#" :lang("*-Latn-US") "#.trim()));
    assert!(!hits(r#" :lang("*-Cyrl") "#.trim()));
    assert!(hits(":lang(*)"));

    // Comma-separated lists match when any one range matches; quoting a
    // range changes nothing.
    assert!(!hits(":lang(de, fr)"));
    assert!(hits(":lang(de, EN)"));
    assert!(hits(r#" :lang("en") "#.trim()));
    // Whitespace inside the argument block is insignificant too.
    assert!(hits(":lang( en )"));

    // Grammar misuse refuses at parse time like every selector error:
    // empty argument lists, empty ranges, dangling commas, junk after a
    // complete range.
    for bad in [
        ":lang()",
        ":lang( )",
        r#" :lang("") "#.trim(),
        ":lang(en,)",
        ":lang(en fr)",
    ] {
        assert!(
            d.select_all(doc, bad).is_err(),
            "{bad} should refuse to parse"
        );
    }
}

/// Regression pin for subagent review R3-1: `lang` values are untrusted
/// markup, and matching used to slice them by byte index: a multibyte
/// value plus an unlucky range crashed the whole process. Every query here
/// must answer, not panic; the middle one used to die on a char boundary.
#[test]
fn multibyte_lang_values_never_crash_the_matcher() {
    let mut d = Dom::new();
    let doc = d.document();
    let host = d.create_element(qn("div"), vec![attr("lang", "añx")]);
    d.append(doc, host).unwrap();

    // Range shorter than the tag, splitting inside 'ñ': must be Ok(false).
    assert_eq!(d.matches(host, ":lang(a)"), Ok(false));
    // Exact match across the multibyte character still works.
    assert_eq!(d.matches(host, ":lang(añx)"), Ok(true));
    assert_eq!(d.matches(host, ":lang(añ)"), Ok(false));
}

/// Pinned v1 behavior: with no explicit scope element in the matching
/// context, the engine resolves bare `:scope` against the document element,
/// which is exactly what `document.querySelectorAll(":scope")` answers in
/// browsers. Element-base scoping (`el.qSA(":scope div")` resolving against
/// `el`) still needs wiring and lands with the js layer; recorded in the
/// findings doc.
#[test]
fn document_scoped_queries_resolve_scope_to_the_document_element() {
    let page = build();
    assert_eq!(
        page.d.select_all(page.d.document(), ":scope").unwrap(),
        vec![page.html]
    );
    // and it composes like any other compound
    assert_eq!(
        page.d
            .select_all(page.d.document(), ":scope > body")
            .unwrap(),
        vec![page.body]
    );
}
