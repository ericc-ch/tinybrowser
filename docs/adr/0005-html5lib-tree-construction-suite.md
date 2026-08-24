# ADR 0005: html5lib tree-construction suite

Date: 2026-08-25
Status: Accepted. Closes the standing open item from
[testing.md](../testing.md) before the `js` layer builds on the parse seam.

## Context

`browser::parse_html` is about to carry page JavaScript on top of it; its
tree-construction behavior had only hand-written fixtures. The HTML spec's
recovery algorithms (foster parenting, adoption agency, template contents,
foreign content) are exactly what hand-written fixtures miss, and exactly
what the vendored html5lib tree-construction suite encodes — the same bar
production engines run.

## Decisions

### Suite: html5lib-tests as a pinned submodule under third_party/

- Upstream <https://github.com/html5lib/html5lib-tests>, git submodule at
  `third_party/html5lib-tests`, pinned to
  `9329e64694e7835d0dcff9811e22856ef6ad16f9`.
- **Why that pin**: upstream deleted `tree-construction/` in June 2026 (the
  tests moved into web-platform-tests); the pin is the final revision
  carrying the suite, so master cannot be tracked.
- **Submodule over a committed snapshot** (which [webidl.md](../webidl.md)
  prefers for IDL): a submodule pins automatically and keeps ~2 MB of data
  out of history. The cost — fresh clones need
  `git submodule update --init`, so tests are not offline-capable until then
  — is accepted for test data and recorded in
  [VENDORED.md](../../third_party/VENDORED.md). The harness fails loudly
  with the init instruction rather than skipping.

### Scope: full-document parses, both scripting flags

- Every case runs through the public API (`browser::parse_html_with_scripting`)
  under each scripting-flag setting the case's markers demand (`#script-on`,
  `#script-off`, absent → both), per the spec's
  [scripting flag](https://html.spec.whatwg.org/multipage/parsing.html#scripting-flag).
- Comparison is byte-exact against the `#document` dump, rendered from our
  `Dom` by the test-side serializer (`tests/html5lib/dump.rs`) following the
  upstream format README and html5ever's reference driver.
- **Deferred**: `#document-fragment` cases (fragment parsing / `innerHTML`
  does not exist yet) — counted and reported by the harness, never silently
  dropped. Tokenizer and encoding suites wait for the net/url layers where
  they belong.

### Gate: fix-or-document, no silent ignores

First full run: **16 divergences in 3165 cases**. Oracle-tested against
`markup5ever_rcdom` (html5ever's own test DOM) to attribute each:

- **Ours, fixed**: MathML `annotation-xml` children mis-nested to `<body>`.
  Root cause: the builder consults
  [`TreeSink::is_mathml_annotation_xml_integration_point`](https://html.spec.whatwg.org/multipage/parsing.html#html-integration-point)
  before breaking foreign-content tokens out to HTML; we inherited the trait
  default (`false`). The sink now records the flag html5ever computes at
  `create_element` time (`ElementFlags::mathml_annotation_xml_integration_point`)
  and answers from it.
- **Upstream, accepted**: `webkit02.dat` #44–47 — `<selectedcontent>` under
  the relaxed `<select>` parsing rules. html5ever leaves
  `maybe_clone_an_option_into_selectedcontent` unimplemented ("will result in
  a (slightly) incorrect DOM tree", per the trait docs); rcdom diverges
  identically. Listed in the harness's `KNOWN_UPSTREAM_DIVERGENCES` with the
  reason inline; **retires when the html5ever pin moves past the gap** or we
  implement the clone ourselves.

## Consequences

- Trait-default audit (what else was silently answering defaults): 
  `associate_with_form` no-op — form ownership unmodeled until form support;
  `attach_declarative_shadow` default-false — declarative shadow roots land
  with the js layer, no suite coverage exists today; `pop` /
  `mark_script_already_started` / `set_current_line` — notification hooks,
  irrelevant headless until parse-error positions matter.
- `Parsed` grew two public members: `template_contents` (previously dropped
  at `finish()`, making template contents unreachable — also the DOM-truthful
  counterpart of `template.content`) and `parse_html_with_scripting` as the
  flag-explicit entry point (`parse_html` delegates with the flag enabled).
- New divergences fail the workspace build with a diff report (first five
  full, rest summarized); they end as fixes or entries here, same as
  [ADR 0004](0004-dom-v1-audit-acceptances.md)'s rule.
