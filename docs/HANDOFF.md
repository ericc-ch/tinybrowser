# Handoff (2026-10-04)

Goal: record the merged WebIDL migration and its deferred follow-ups so a
fresh session can pick them up. See `docs/bindings.md` for the binding policy
and `AGENTS.md` for the working rules.

Plan: fix each item under Unfinished when its trigger fires (corpus hit or
failing test), smallest slice first per the WPT fast-loop rule. Nothing there
blocks normal work on `main`.

State: `main` at `1bd06d1` (merge of PR #39 `webidl-bindings`), working tree
clean. Verification green at merge: `cargo build`, `cargo clippy -p renderer
-p webidl-bindgen`, `cargo test -p webidl-bindgen` (19 pass), plus targeted
WPT singles vs the pre-migration binary (see Done).

Done:

- WebIDL migration merged (`1830376` migration complete through `1bd06d1`
  merge). Regression-against-`main` verdict over the 2,887-test remote slice
  (`~/projects/wpt-reports-aadc5d0/slice-report.json`, WPT pin `92054a74`):
  4 real regressions found, all fixed — `fd5ae1e` (`frame.name`/`src`
  contract claim, verified 0 frame fails in
  `html/dom/reflection-obsolete.html`), `f96d530` (FormData reentrancy flag,
  `form-submission-algorithm.html` back to OK), `4431d6b` (honest media
  states + constants on interface objects, 72-file media blast clean),
  `1701594` (base-href fallback, `document-metadata` dir clean).
- Code-review pass applied (`9eb1b6d`, verified with the checks above plus
  single-file WPT: `base_href_unspecified.html`, `networkState_initial.html`
  and frame-element getters diff clean; `resource-selection-remove-src.html`
  keeps only its known deferred-timing failure). No CodeRabbit review was
  possible — 465 files exceed its 100-file limit (check passes as skipped);
  three subagent reviews substituted (see Decisions).

Unfinished (deferred follow-ups, fix when triggered):

- Generator latent bugs, none triggered by the current IDL corpus (verify
  with `grep` in `crates/webidl-bindgen/idl/` before touching):
  `collect_reference` drops `NullableUnion` dictionary refs
  (`crates/webidl-bindgen/src/contracts.rs:750`); nullable-union defaults
  ignored at emit (`crates/webidl-bindgen/src/emit.rs:1066` vs the
  `NullableUnion` arm); non-node union members hit `unreachable!`
  (`crates/webidl-bindgen/src/emit.rs:1705`) — must become a lowering
  `Error`; `[ReflectSetter]` path skips attribute validation; partial/mixin
  scope flags dropped for property hooks (`contracts.rs:1014`);
  signatures compared by arity only (`contracts.rs:1818`, `absorb` at `:94`);
  range routing over non-contiguous op ids; `expect()` on hook
  preconditions (`emit.rs:375,465,471`); constructor prototype lookup runs
  after arg conversion (order-vs-comment question).
- `scroll_into_view` discards its IDL argument
  (`crates/renderer/src/js/bindings/node.rs:3629` forwards to the arg-less
  inherent at `:641`). Pre-existing on `main`, not a regression; needs the
  boolean-vs-options branch from cssom-view.
- Element-level `src`/`href`/`name`/`content` shim kept for main-parity
  (`crates/renderer/src/js/scripts/brands.js`, see comment above
  `interfaceMembers`). Per spec most elements must not expose them; remove
  only after per-element contracts cover every declarer (`applet`,
  `a[name]`).
- Two resource-selection subtests expect a deferred revert to
  `NETWORK_EMPTY` at a stable state; the synchronous engine has no later
  task boundary to model that. Track loading/error events unimplemented
  (`track-mode` timeout, `src-empty-string` failure) — same as `main`.
- ~52 `arg_0.to_string()?` → `WebIdlString` rewraps in `node.rs` (alleged
  IDL-conversion bypass, unproven — needs a failing case before rework).

Next:

1. When new IDL hits a generator latent bug, fix that bug first with a
   generator unit test, then land the IDL.
2. When a WPT slice covers `scrollIntoView` options, media stable-state
   timing, or track events, implement the smallest spec step that flips it.
3. Before dropping the Element reflection shim, add the missing
   per-element contracts and diff `html/dom` + `html/semantics` pieces.

Decisions made:

- Merged PR #39 without CodeRabbit (file-limit skip) on the strength of the
  slice-verified no-regression verdict plus the applied review pass.
- Plain merge, not squash: per-commit messages carry the decision history.
- Kept the Element shim and the synchronous media approximation for
  main-parity; spec-pure behavior waits on the contracts/pipeline above.
- Subagent reviews treated as hypotheses, not verdicts: ~9 of ~25
  "must-fix" items were real; two (networkState mapping, WeakSet identity)
  were refuted against test expectations and wrapper-interning evidence.

Gotchas:

- WPT fast loop is now in `AGENTS.md`: smallest sufficient slice, `retest`
  for failures, never pipe runs through `tail`, clean orphan `tinybrowser
  webdriver` processes and `~/.cache/tinybrowser/wpt-*.lock` after kills.
- Pre-migration baseline binary:
  `/home/erickc/.cache/tinybrowser/baseline-target/debug/tinybrowser`.
  Per-piece baselines in `~/.cache/tinybrowser/logs/piece-*.json`.
- Unless already inside the dev shell, run cargo/runners through
  `nix develop --command`; `tools/check` handles it for clippy.

---

# Handoff (2026-10-03)

Goal: replace implementation-shaped WebIDL with the unchanged, pinned upstream
contract. Every Rust and JS binding derives names, inheritance, descriptors,
arity, and conversions from that IDL. See `docs/bindings.md` for the policy
and `AGENTS.md` for the rules.

State: branch `webidl-bindings`, HEAD `1701594`, working tree clean.
The migration is COMPLETE:

- All 8 remaining legacy files ported: `EventTarget` (two payloads),
  `Node` (two payloads), `Element`, `Document`, `Event`,
  `HTMLOptionsCollection`, `NamedNodeMap`, `ElementReflections` (deleted;
  members moved to per-element contracts incl. the `HTMLHyperlinkElementUtils`
  mixin, `HTMLBaseElement`, link/media/embed/script/source/track/meta/map/
  object/output/param/slot, template).
- Legacy path deleted in the same line: `crates/renderer/idl/`, the build.rs
  legacy loop, `compile()`, the `Rust*` annotation parser (~1500 lines of
  model.rs). Model keeps only data plus constant/callback/enumeration
  validators. Dead conversions removed (`from_js`, `CtxMode::Owned`,
  `PropertyHooks::JavaScript`, `GetterMapping::Field`,
  `ReturnType::NodeList`/`Value`, `legacy_code`).
- Generator grew: multi-payload dispatch, `[PutForwards]`, `[ReflectURL]`,
  stringifier attributes, nullable/required/enum/interface/string dictionary
  members, nullable unions, restricted/unrestricted doubles,
  `Promise<undefined>`, anonymous indexed setters, inherited hook getters,
  overload sharing, `[LegacyUnforgeable]`/`[LegacyOverrideBuiltIns]` scopes.
- Blocker from the old note resolved as designed: the generator allows many
  payloads per interface; the prototype owner is elected by native class name
  (`elect`), never file order; each payload carries its own receiver check.
- `relatedTarget` dropped (belongs to unmodeled `FocusEvent`; every in-scope
  reader constructs `FocusEvent`).

Verification so far: `cargo test --workspace` + `tools/check` green (36 suites);
attributes.html 66/1 x8; template-element +202/-0; Playwright 37/37; CDP clean.

Still open: full-suite `--score /` running in background
(`~/.cache/tinybrowser/logs/full-score.log`, report `full-report.json`);
then release size (`nix develop --command ./tools/release`), `docs/progress.md`
snapshot (binary size, total, scored groups only), commit docs. No pushes;
draft PR https://github.com/ericc-ch/tinybrowser/pull/39 holds earlier work.

## Generator capabilities

Native contracts (`crates/webidl-bindgen/src/contracts.rs`, `emit.rs`):
- Discovery from `impl {name}_generated::{Interface}<'js> for Payload`.
- Attributes: `constructor`, `get_x`/`set_x`; operations snake-cased; optional
  JS receiver as a leading `Object<'js>`.
- `[Reflect]` / `[Reflect="custom"]` generated with no trait method (Blink-style,
  `bind_gen/interface.py`). Only DOMString and boolean so far.
- `[ReflectSetter]`: generated setter, implemented getter.
- `[Unscopable]` merging `@@unscopables`.
- Unions (`lower_union`): interface (node -> `NodeReference`, other ->
  `Value`), dictionary, boolean, long, string; nested and typedef'd unions
  flatten; Trusted Types interfaces collapse to string; conversion order
  platform-object, dictionary, boolean, number, string, per `es-union`.
- Enum attributes (getter + setter).
- Nullable `unsigned long?` (`NullableUnsignedLong`).
- Mixins install on every including interface, targets from IDL includes.
- `LegacyFactoryFunction` accepted as JS-shim metadata; `WindowProxy` as an
  opaque platform object.
- Dictionaries and enums allow unconsumed generated members.

JS contracts (`crates/webidl-bindgen/src/javascript.rs`,
`crates/renderer/src/js/scripts/bindings.js`): Oxc discovery of
`__tbInstallInterface`, private installer with brands, descriptors, safe
argument reads, realm-correct `Uint8Array` results. `TextEncoder` migrated.

## Migrated interfaces

Native: DOMException, NodeList, HTMLCollection, MutationRecord, DOMImplementation,
XMLSerializer, MutationObserver, DOMTokenList, CharacterData, DocumentType,
ProcessingInstruction, Attr, DocumentFragment, HTMLElement, SVGElement,
MathMLElement, ParentNode, ChildNode, NonDocumentTypeChildNode,
HTMLOptGroupElement, HTMLButtonElement, HTMLFieldSetElement, HTMLSelectElement,
HTMLTextAreaElement, HTMLInputElement, HTMLFormElement, HTMLOptionElement,
ShadowRoot, HTMLIFrameElement, HTMLImageElement, DOMParser.

JS: TextEncoder. Placeholders removed: sendBeacon, pipeThrough, scrollTo,
Selection, matchMedia listeners.

## Remaining legacy (8 files) — all ported in `1830376`, kept for archeology

`Document`, `Element`, `ElementReflections`, `EventTarget`, `Event`,
`HTMLOptionsCollection`, `NamedNodeMap`, `Node`.

Each needed:

- **Node** — has `RustAlternate=JsAttr` (drop it; `Attr` has its own
  contract) **and** a blocker: `Node.webidl` also lists `addEventListener`,
  `removeEventListener`, and `dispatchEvent` with `JsNode`-receiver dispatchers.
  rquickjs brand-checks methods by defining class, so `JsNode` elements cannot
  call `JsEventTarget.prototype`'s methods. The JS prototype chain
  (`Node.prototype` -> `EventTarget.prototype`) is not enough. Porting `Node`
  removes those dispatchers and breaks `element.addEventListener`. Resolve by
  giving `EventTarget`'s contract install targets that include descendant
  payload prototypes (`Node`), or by a general "install a base interface on a
  second class payload" mechanism. The generator currently allows one impl per
  interface trait, so this needs a design decision.
- **Element** — `[PutForwards]` on contract attributes (`classList`),
  `(boolean or ScrollIntoViewOptions)` union (dictionary|boolean supported),
  `[LegacyNullToEmptyString]`.
- **Document** — `[PutForwards]`, `(TrustedHTML or DOMString)` union (collapse
  supported), dictionaries, sequences.
- **EventTarget** — nullable callback argument (`EventListener?` is a
  `callback interface`); union `(AddEventListenerOptions or boolean)`.
  Payload is `JsEventTarget` in `crates/renderer/src/js/events.rs`, not JsNode.
- **Event** — `[LegacyUnforgeable]` (`isTrusted`), `EventInit` dictionary,
  `sequence<EventTarget>` result (`composedPath`). Needs unforgeable own-property
  semantics (define per instance, non-configurable) or a documented decision.
- **HTMLOptionsCollection** — interface unions (supported), nullable union
  (`(HTMLElement or long)?`), inherited property hooks from `HTMLCollection`.
- **NamedNodeMap** — interface-typed arguments (`Attr`), JS-implemented
  property hooks (`RustPropertyHooks=JavaScript`; upstream Blink uses
  `LegacyPlatformObject`).
- **ElementReflections** — a fake interface; delete it and move `name`/`href`/
  `src`/`content` to the real element interfaces that declare them.

## Decisions

- Discovery is the implementation itself; no second checklist. Unsupported
  implemented semantics fail the build.
- The IDL is never edited to fit code.
- Generated dispatch calls the contract trait explicitly.
- JS results allocate in the receiver's realm via a per-instance realm token.
- Reflection is generator-owned (Blink-style), not hand-written.
- Trusted Types interfaces collapse to their string member until Trusted Types
  is implemented.
- `Attribute::isTrusted` should get real unforgeable semantics, not be dropped.
- `ElementReflections` will be deleted in favor of the real interfaces.

## Gotchas

- One WPT run at a time; the runner serializes on a venv lock.
- Never `git submodule update --init --recursive` in a worktree: it clones the
  1.2 GB WPT checkout. Init `third_party/rquickjs` only, or copy it.
- Never `git add` a deleted path with modifications: the pathspec error aborts
  the add. Use `git rm` first, or add only surviving paths.
- A dead `#[qjs(skip)]` method becomes a `dead_code` error once its last legacy
  consumer is deleted; the compiler drives the cleanup.
- `Ctx` by value vs `&Ctx` matters: contract trait methods take `Ctx` for
  operations and `&Ctx` for attributes. Inherent methods may differ, so inline
  or rename on collision.
- Enum getters with no argument/setter use and dictionary fields can be unused;
  both are allowed in generated code with a reason.
- Baseline binary for WPT comparison: `/home/erickc/.cache/tinybrowser/baseline-target/debug/tinybrowser`
  is pre-session; rebuild per checkpoint or compare against the previous batch's
  JSON in `~/.cache/tinybrowser/logs/`.

## Next

1. Design the base-interface/second-payload install for `EventTarget` so `Node`
   can drop its duplicated listener dispatchers (see the Node bullet above).
   Then port `EventTarget` (JsEventTarget) and `Node`, then `Event` with
   `[LegacyUnforgeable]`, then add nullable callback arguments.
2. Add `[PutForwards]` and port `Element`; then `Document`.
3. `HTMLOptionsCollection` (nullable union + inherited hooks), `NamedNodeMap`
   (interface args + hooks), delete `ElementReflections`.
4. Delete the legacy path and run full WPT; measure the release binary and
   update `docs/progress.md`.
