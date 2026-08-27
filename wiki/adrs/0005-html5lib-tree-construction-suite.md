# html5lib tree-construction suite

Hand-written parse fixtures miss foster parenting, the adoption agency, templates, and foreign content. The harness runs the html5lib tree-construction suite through `browser::parse_html_with_scripting` as a git submodule pinned to the last upstream revision that still contains `tree-construction/` (`9329e646`, 2026-06-20); later master deleted that directory.

Status: accepted

Every full-document case runs under each scripting-flag setting the markers demand; dumps compare byte-exactly. `#document-fragment` cases are counted and skipped until fragment parsing exists. New divergences fail the build: fix or document, never silently ignore.

Known upstream gap: `webkit02.dat` #44–47 (`<selectedcontent>` under relaxed `<select>`). html5ever leaves `maybe_clone_an_option_into_selectedcontent` unimplemented; `markup5ever_rcdom` diverges identically. Listed in `KNOWN_UPSTREAM_DIVERGENCES`; retires when the html5ever pin moves past it.

## Options considered

- **Committed snapshot** (what [webidl.md](../researches/webidl.md) prefers for IDL): reproducible offline, but ~2 MB in git history. A submodule pins the SHA and keeps the bytes out; clones need `git submodule update --init` (harness fails loudly with that instruction). Pin details live in [VENDORED.md](../../third_party/VENDORED.md).
- **Tracking html5lib master:** impossible after the June 2026 move into web-platform-tests.

## Consequences

Repoint the harness at WPT `html/syntax/parsing/resources/*.dat` when the js layer can run testharness wrappers. Until then the frozen pin trades freshness for zero churn against a ~2.6 GB monorepo. Trait-default no-ops (`associate_with_form`, declarative shadow, parse-position hooks) wait on those features.
