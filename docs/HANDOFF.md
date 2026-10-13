# Handoff (2026-10-13)

Status: complete on branch `refactor/dom-layer`, 12 commits on `d78b30e`.
Nothing pushed. The PR is the record; this note keeps the decisions, the
evidence, and the follow-ups that did not fit a commit message.

Goal: extract the realm-free DOM layer out of the JS bindings, split the
bindings by interface family, migrate the last natively installed members onto
generated contracts, cut the `js`/`document` cycle, and unify the
platform-object lifecycle.

Layering: `documents` + `dom` <- `js` <- `document` <- `engine`. Below `js/`
there is no `Ctx`, no `Value`, no `Persistent`, and no `JsNode`; `tools/check`
enforces that for `dom/` and `documents.rs` (`run_layers`). Tree algorithms
return ordered effects (navigables to destroy, an `InsertionStep` list), not
callbacks.

What landed:

- `f70c0d4` node identity and tree walks -> `dom/tree.rs`.
- `78b4e24` serialization and node equality -> `dom/serialization.rs`,
  `dom/equality.rs`; `nodes_equal` takes `&DocumentStore`.
- `4d1716a` insertion core -> `dom/insertion.rs`; `JournalEntry` ->
  `documents.rs`; `replace_all_*` return navigables to destroy.
- `50401a7` pre-insert validity -> `dom/insertion.rs` returning `TreeError`.
- `9017fea` `MoveTarget`, `ensure_can_move`, `move_parsed`, `replace_parsed`;
  one `apply_inserted` realm-side application point.
- `8a1a84f` adoption snapshots and materialization -> `dom/adoption.rs`.
- `133e122` form state (`dom/forms.rs`), rendered text (`dom/text.rs`), infra
  parsers (`dom/parse.rs`), namespaces (`dom/namespaces.rs`); the three copies
  of the option/select helpers become one.
- `13c470f` `bindings/node.rs` (10,892 lines) splits into
  `bindings/node/{mod,core,document,element,forms,media,html_elements}.rs`.
- `b24a514` `Document.adoptNode`, `Document.scripts`,
  `HTMLElement.innerText`/`outerText` move onto generated contracts; the
  `#[rquickjs::methods]` block holds only the constructor; brands.js loses the
  native member copy and the `interfaceMembers` table.
- `f9e6598` `FrameRuntime` -> `js/runtime.rs`; frame creation inverted through
  a frame-host hook so `js` never names `Document`; `Stop` -> `js/runtime.rs`;
  encoding sniffing -> neutral `sniff.rs`; the layer gate lands in
  `tools/check`.
- `05241fb` `WrapperCache<K>` for the wrapper caches; the same-object cache is
  keyed by IDL interface name instead of the `Wrapper` enum; one construction
  point (`host::instance_proto`) with named prototype rules;
  `docs/bindings.md` gains the Platform objects section.

Verification (all against `d78b30e` as the pre-refactor baseline):

- `tools/check` exit 0 (clippy, embedded JS, layer gate, rustdoc).
- `cargo test --workspace` 34/34 test binaries.
- WPT `dom/nodes`: identical failure list (422 tests, 317 pass, 84 unexpected,
  15 timeout, 6 error).
- WPT `html/dom/elements/the-innertext-and-outertext-properties` plus both
  `Document-adoptNode` tests: identical failure lists.
- WPT `document.scripts.html` 1/1; `dom/collections` + classList tests 13/13.
- WPT iframe-element slice (nosrc, javascript-url initial insertion,
  not-about-blank, append-to-child-document): identical failure list.
- Playwright gate: 35 passed, the same 9 pre-existing screenshot/forms
  failures as baseline.
- Blink CDP default corpus passes.
- Release binary 11,103,368 bytes vs 11,116,328 on `d78b30e` (12,960 bytes
  smaller). `docs/progress.md` size snapshot updated.

Decisions:

- DOM validity algorithms return `TreeError`; the boundary maps it to a
  DOMException. Messages collapsed to one wording per exception class
  (unspecified; no WPT delta).
- Inherent methods on `JsNode` stay `pub(super)`: an invisible inherent method
  that shadows a generated trait method turns delegation into infinite trait
  recursion.
- One `#[rquickjs::methods]` block per class is forced by the macro; it holds
  only the constructor now. The other members delegate from generated trait
  impls to inherent `pub(super)` methods.
- Generated dispatch groups carry `clippy::too_many_lines`: a group batches
  four members with one receiver borrow.
- `select_options` uses the owner-checking walk, so nested-select options
  belong to the inner select (fixes `select-selectedOptions-nesting`).
- Platform objects: one `JsNode` wrapper per node; same-object members cached
  per `(node, interface name)`; three prototype-resolution rules kept because
  they are spec-distinct (ctx realm for standalone classes, node's owning
  realm for node-associated objects, creator realm first for node wrappers).
- The attribute entry keeps its own `wrapper` field: per-entry storage, not a
  map; splitting it would need two structures kept in sync on removal.

Follow-ups:

1. Candidate 2, webidl-bindgen, as its own pass: the IDL-to-Rust type mapping
   is written three times (`contracts.rs` `native_type` + friends, `emit.rs`
   per-context conversion, `javascript.rs`). `model::Interface` has 22 fields
   and carries pre-rendered tokens; module names are computed twice. Silent
   fallbacks: `hook_members` swallows errors, `lower_members` skips
   unimplemented members, `elect` returns `Ok(())` with no payload owner,
   `legacy_codes` always emits nothing, eight `unreachable!` sites,
   `validate_sources` has no production caller. Direction: one lowering pass
   with a closed conversion enum; trait contract, dispatch, and the JS-shim
   check all render from it. Checks: `cargo test -p webidl-bindgen`,
   `tools/check`, `tools/webidl/import --check`.
2. WPT work carried over from the 2026-10-10 note: the explicit manifest step
   (`tools/wpt/manifest.py` calling `load_and_update`), the `createEvent`
   init-path proof, per-move suppression in
   `js/scripts/web/custom_elements.js`, and the `dom/nodes` fail-list buckets.
3. `docs/bindings.md` still documents brands.js-held members (`type`, `files`,
   `hidden`, URL-decomposition attributes) as cutover gaps; each needs
   generator support before its shim code can go.

Gotchas:

- Code moves by name, never by line range. A bad range leaves a stray `}` that
  surfaces as the build-script error "invalid Rust: cannot parse string into
  token stream". Parse-check with
  `nix develop --command rustfmt --edition 2024 --emit stdout <file>` first.
- The generator runs at build time over every `.rs` under `src/js`.
- Each generated `<Interface>.rs` starts `use super::<Payload>;`, so an
  `include!` must sit in a module with `JsNode` in scope.
- `js/world.rs` `materialize_iframe` leaves frames registered when a World has
  no frame host (test worlds); a later `take_pending_frames` drain picks them
  up.
- No host `python3`, `pkg-config`, or OpenSSL. Run cargo, `tools/check`, and
  every runner through `nix develop --command`.
- WPT `dom/nodes` flake patterns: `NodeList-static-length-getter-tampered-*`
  (passes alone) and `moveBefore` timing. Compare full `▶` failure blocks, not
  the truncated `needs attention` print.
- The worker/serviceworker idlharness variants fail at setup on both revisions
  (pre-existing harness gap); use the window variants when probing.
