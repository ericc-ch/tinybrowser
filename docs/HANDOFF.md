# Handoff (2026-09-11)

State: Branch `wpt-pass-chasing`, HEAD `2ad4ffd`. WPT runs for real: full
`dom/nodes` went from 62 passing subtests to **3740** (run 5, at `2ad4ffd`;
241 OK / 68 TIMEOUT / 19 ERROR files). `cargo test --workspace` green,
clippy + fmt clean on every commit.

Done (branch commits, each with its verification):

- `d66eb69` WPT unblock + DOMException + core Node/CharacterData. WebDriver
  Release Actions is a no-op and the WPT product skips the focus click.
- `ce089ac` element interface table, ParentNode/ChildNode, querySelector,
  attributes, classList.
- `eb6b639` Attr + NamedNodeMap, qualified-name lookup rules, collection
  proxies (has/ownKeys traps, platform-method binding).
- `be7e33b` secondary documents, `createDocument`/`createHTMLDocument`,
  `DOMParser` (+ in-process test `created_documents_are_second_trees`).
- `923a836` creation metadata, spec-order insertion validation,
  ProcessingInstruction + CDATASection, XML xmlns.
- `21c6e70` MutationObserver (recording in the arena, microtask delivery,
  `takeRecords`; test `mutation_observers_queue_and_deliver_records`).
- `2ad4ffd` constructible Text/Comment/DocumentFragment/Document/XMLDocument,
  textContent replace-all, normalize in tree order, MutationObserverInit
  presence rules + attributeFilter + attributeNamespace, `parentElement`.

In flight:

- None; the run-5 sweep is the measured state. Rerun with
  `direnv exec . ./tools/wpt/run --log-wptreport=/tmp/opencode/dom-nodes-5.json dom/nodes`.
- Remaining buckets in run 5: 672 NOTRUN (mostly timeouts), 488
  `documentElement of undefined` + 110 `DOMException of undefined` + 26
  `document undefined` (all iframe XML/XHTML tests), 148 not-a-function
  (adoptNode, moveBefore, Range, customElements), 67 wrong-domain throw
  tests, 15 cycle cases, 11 doctype identity.

Next:

1. Parse the run-5 report and record the number here; chase the largest
   non-iframe bucket.
2. MutationObserver residual mismatches: replaceChild/normalize record
   counts, fragment insertion records, `attributeFilter` presence cases
   (`dom/nodes/MutationObserver-*.html`).
3. `Document-adoptNode`/`adoptNode` and `moveBefore` are missing; ~40
   subtests. Doctypes across documents copy instead of adopting, so node
   identity across documents still fails (~12 subtests, architectural).
4. `document.createRange()` and Range APIs unlock the observer Range cases
   and the `dom/ranges/` directory.
5. iframes/browsing contexts remain the largest single blocker (~600
   subtests across `frames[...]`, `contentDocument`, `DOMException`).
6. html5lib submodule removal waits until WPT `html/syntax/parsing/` runs
   the tree-construction corpus (ADR 0005, ADR 0008).

Decisions made:

- Failures from missing subsystems stay visible; no expectations files.
- Rust-vs-JS split per ADR 0008/0010: all platform objects hand-written in
  Rust; DOMException is a Rust class, not a JS polyfill.
- Mutation recording is off unless an observer exists, so parsing stays
  unaffected.

Gotchas:

- Build/test only inside `direnv exec .`; bare cargo fails in openssl-sys.
- Parse wptreport with `third_party/wpt/_venv3/bin/python`.
- A TIMEOUT is often testharness waiting on an iframe `load`, not a hang in
  our code; the `Document-createElement*` iframe variants sit for 10s each.
- The renderer child speaks JSON on stdout; never `println!` in renderer
  code. `cargo check` while a WPT run is live is safe; `cargo build` is not.
- `queueMicrotask` drives MutationObserver delivery, and `cargo test -p
  renderer` is the fast loop for new bindings.
