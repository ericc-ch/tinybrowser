# Handoff (2026-09-11)

State: Branch `wpt-pass-chasing`, base and main at `d8833ca`. WPT now actually
executes testharness and reports subtests. `cargo test -p webdriver` green.
No DOM conformance work started yet on the branch.

Done:

- WPT execution unblocked, in `d8833ca`. WebDriver Release Actions is a no-op
  (`crates/webdriver/src/lib.rs`) and the WPT product skips the setup focus
  click (`tools/wpt/tinybrowser_wpt.py`); `create_test_window` override.
  Evidence: `./tools/wpt/run dom/nodes/Node-appendChild.html` returned real
  subtest results, and the full `dom/nodes` sweep completed with 225 OK files.
- Baseline `dom/nodes` wptreport: `/tmp/opencode/wpt-dom-nodes.json`.
  225 OK / 35 ERROR / 68 TIMEOUT / 6 SKIP files; 62 PASS vs 2759 non-PASS
  subtests. Rerun with
  `direnv exec . ./tools/wpt/run --log-wptreport=/tmp/opencode/wpt-dom-nodes.json dom/nodes`.

In flight:

- Nothing edited on the branch yet. Planned first change: missing DOM methods
  in `crates/renderer/src/js/bindings.rs` and `DomError` mapping in
  `crates/dom/src/arena.rs`.

Next (ranked by baseline impact in `/dom/nodes`):

1. Standard DOM methods (935 "not a function" subtests): Element
   `hasAttribute`/`hasAttributeNS`/`getAttributeNS`/`setAttributeNS`/
   `removeAttribute[NS]` (case.html 280, attributes.html 63); ParentNode
   `querySelector`/`querySelectorAll`/`append`/`prepend`/`replaceChildren`
   (querySelector-escapes 68); ChildNode `before`/`after`/`replaceWith`/`remove`
   (123); CharacterData `appendData`/`deleteData`/`insertData`/`replaceData`/
   `substringData` (110); Node `cloneNode`/`isEqualNode`/`isSameNode`/
   `lookupNamespaceURI`/`lookupPrefix`/`insertBefore`/`removeChild`/
   `replaceChild` (200+).
2. `document.implementation` + DOMImplementation: `hasFeature` 136,
   `createHTMLDocument` 115, `createDocument` 17, `createDocumentType` 8.
   If bindings cannot parse in-process, reuse the browser/renderer parse path.
3. DOMException: hand-written Rust global
   (`webidl.spec.whatwg.org/#idl-DOMException`), `DomError` -> name per
   `dom.spec.whatwg.org/#dom-domerror`, replacing `Exception::throw_type` in
   bindings. Rust-vs-JS split per ADR 0008 and ADR 0010.
4. Window aliases: `frames` and `length` = 0; `window`/`self`/`parent`/`top`
   are already set in `bindings.rs::install`.
5. Later buckets: MutationObserver 84, DOMParser 46, HTMLElement/SVGElement
   globals ~55, characterSet-normalization TIMEOUT files 654 NOTRUN.
6. Remove the html5lib submodule and its harness only after WPT
   `html/syntax/parsing/` runs the tree-construction corpus green (ADR 0005,
   ADR 0008); upstream moved the corpus into WPT, so the submodule pin no
   longer tracks master. Delete the browser-crate JS/DOM stand-ins when the
   first testharness file is green, per ADR 0008.

Decisions made:

- Missing-subsystem failures stay visible; no wptrunner expectations files.
  Click and Perform Actions stay `unsupported operation`; only Release Actions
  is a no-op, and the product override covers the runner's focus click.
- Generated `docs/architecture.html` is gitignored; `docs/architecture.md` is
  the source of truth (`tools/docs/build.sh`).
- Metric: `dom/nodes` per change, `dom/` per milestone, one commit per green fix.

Gotchas:

- Build and test only inside `direnv exec .` (nix dev shell); a bare
  `cargo build` fails in openssl-sys because pkg-config is missing.
- Parse wptreport with `third_party/wpt/_venv3/bin/python`; plain `python3` is
  not on PATH in or out of the dev shell.
- A TIMEOUT can be the 5s QuickJS budget, not a logic bug.
- The renderer child speaks JSON on stdout; never `println!` in renderer code.
- `tools/wpt/run` reinstalls the product into the WPT venv on every run.
