# Handoff (2026-10-01)

Goal: Replace tinybrowser's whole native JS binding layer with browser-specific
WebIDL codegen plus shared dispatch, to shrink the shipping binary. Whole-layer
migration is in progress. The parser, character-data, `Attr`, and
`Event`/`EventTarget` slices have landed; `Document`, `Element`, form controls,
and the exotic collections still use the old bindings.

Plan: Extend the generator with each migrated interface. Verify each step with
`tools/check`, workspace tests, and a before/after WPT report comparison.

State: Branch `webidl-bindings` has commits `e2b3498`, `178ef61`, and
`d80b626` (MutationObserver and Node text accessors). Draft PR:
https://github.com/ericc-ch/tinybrowser/pull/39. The rquickjs submodule has
local commits `d73fbda` and `7a541e0`; the QuickJS fork commit `712757e` is
published. `tools/check` and workspace tests pass.

Done:

- First WebIDL slice, commit `e2b3498`: `DOMException`, `MutationRecord`, and
  eleven `Node` members (four mutation operations plus seven readonly
  attributes) migrated to build-time generated shared dispatch.
  Verification: `tools/check` clean, `cargo test --workspace` clean, 57-file
  webidl/mutation WPT slice 177 FAIL-to-PASS with zero regressions and
  identical inventories, scratch realm/constructor WPT 12/12, Playwright 15/15,
  Blink CDP corpus PASS, standalone Valgrind zero errors/lost bytes.
  Shipping binary 8850248 bytes, 19760 under the 8870008 baseline
  (`docs/progress.md` updated).
- Fork publishes: QuickJS-NG fork commit `712757e`, rquickjs commit `d73fbda`
  (realm-aware C closures, `Function::new_native`, `Function::realm`).
- Recursive submodule/path-patch wiring, commit `178ef61`. The editable
  rquickjs checkout is `third_party/rquickjs`; QuickJS is its nested
  `sys/quickjs` checkout.

Implemented in the worktree:

- `MutationObserver` uses generated construction and member dispatch.
  Callback functions, dictionaries, undefined operation results, record
  sequences, and forwarding the receiver object are supported.
- Dictionary conversion preserves presence, sorts member lookups, and applies
  nullish defaults directly. String sequences use the iterable protocol and
  native ToString. Filters retain arbitrary UTF-16 code units. Returned arrays
  define own indexed properties.
- `js/observers.rs` holds one coordinator in the shared `RealmRegistry`.
  The observer receiver stores a weak registry reference and an agent-wide id.
  Borrowed foreign methods resolve the receiver's coordinator.
  Notification snapshots pending ids and takes each queue at its delivery turn.
  Callback mutations schedule another microtask. Callbacks execute and report
  exceptions in the callback function's realm.
- DOM journals share an injected `MutationOrder`. The coordinator merges
  journal entries by position before matching them. Reattaching the same clock
  preserves queued positions, including parser reinsertion.
- Generated attributes install getter and setter together. `nodeValue` and
  `textContent` now use generated dispatch. Omitted setter arguments convert
  from undefined, following WebIDL despite Chromium/Firefox's arity check.
- rquickjs `String::to_utf16()` safely copies the engine's UTF-16 buffer, and
  `String::from_utf16()` builds one. The fork unit test and
  allocation-failure/recovery probe pass under Valgrind.
- Renderer teardown: the `Engine` owns the lazy `SharedJsRuntime`; frames hold
  a weak `JsRuntimeHandle` and each live realm clones the rquickjs `Runtime`.
  The old design kept the runtime inside every frame, so JavaScript host
  closures held by the heap pointed back at the world and the heap, and a
  dropped renderer leaked. `Engine::drop` now clears the frames and registry
  and then collects while it still owns the heap.
- `dom::DomString` stores a DOM string as UTF-16 code units, compacting the
  all-Unicode case to a Rust `String` and promoting only for an unpaired
  surrogate. Character data (`Text`, `Comment`, `CDATASection`,
  `ProcessingInstruction`), `MutationRecord.oldValue`, and the DOM journal use
  it. The HTML and XML serializers emit code units through a UTF-16 buffer, so
  `innerHTML`, `outerHTML`, and `XMLSerializer` round-trip unpaired surrogates.
- Character-data entry points convert through `WebIdlCodeUnits` (exact code
  units) instead of `WebIdlString` (lossy UTF-8): `createTextNode`,
  `createComment`, `createCDATASection`, `createProcessingInstruction`,
  `new Text`/`new Comment`, `CharacterData.data`, `appendData`, `insertData`,
  `deleteData`, `replaceData`, `substringData`, and `HTMLTitleElement.text`.

## Parser, Attr, and custom-element reactions

- Generated bindings now cover `Node` (every declared member), `Attr`,
  `CharacterData`, `DocumentType`, `ProcessingInstruction`, `DOMParser`,
  `XMLSerializer`, and `DOMImplementation`, alongside the earlier
  `DOMException`, `MutationRecord`, and `MutationObserver`.
- The generator gained interface inheritance (prototype and constructor),
  enumerations, booleans with defaults, unsigned short/long, nullable and
  optional strings, `USVString`, callbacks, dictionaries, and
  `NodeReference` alternate dispatch so `Attr` reuses inherited `Node`
  operations.
- `RealmRegistry` owns one `AttributeRegistry` for the agent: globally issued
  ids, the `(element, namespace, local)` attachment index, weak wrappers, and
  the attribute listener lists. Adoption changes the node document and never
  the creation scope, so an `Attr` created in a frame and attached to the main
  document keeps one identity and resolves methods through its owning realm.
- `reactions.rs` owns the custom-element reaction stack, element queues, backup
  queue, and per-frame collector hooks. `custom_elements.js` keeps the
  lifecycle algorithms but defers callback invocation to the native stack.
  `define()` captures prototype callbacks at definition time, guards reentry,
  and reads platform operations and callback storage through captured
  intrinsics.
- The four `Node` event operations route through the generated `EventTarget`
  path for both nodes and attributes.
- `DOMImplementation.createDocument` and `createHTMLDocument` publish new
  documents in the receiver's realm, not the caller's.
- `locate_namespace`/`locate_prefix` share one algorithm with XML and XMLNS
  handling and namespace declarations. `compareDocumentPosition` follows the
  attribute rules, and `tree_order` handles ancestor pairs.
- `dom::DomString` stores code-unit sequences and round-trips unpaired
  surrogates for character data, `MutationRecord.oldValue`, and serialization.

This milestone's verification files are in `/tmp/opencode/jsbinding-research/`:

- `attr-global-check.log`: `tools/check` clean (clippy, embedded JS, rustdoc).
- `final2-comparison.json`: 485-file WPT slice (`dom/nodes`, `domparsing`,
  `custom-elements/reactions`) with 1,451 FAIL-to-PASS, 2 ERROR-to-OK, 6
  previously-missing subtests now passing, and zero status regressions.
- `attr-global-reaction.log` and `attr-global-event.log`: focused probes for
  attribute identity, namespace lookup, adoption, reactions, and borrowed
  attribute event methods.
- `attr-global-playwright.log`: 14/15; the hosted SauceDemo checkout failed on
  a site flake and passed 3/3 when run alone in `attr-global-saucedemo.log`.
- `attr-global-cdp.log`: default Blink CDP gate PASS.
- `lifecycle2-memcheck-*.log`: zero errors and zero definite/indirect leaks
  across the attribute-cycle and reaction lifecycle probe.
- `release2.log`: stripped shipping binary 8,862,872 bytes.

## Event and EventTarget

- `Event` and `EventTarget` now use generated construction and member
  dispatch. `CustomEvent` stays a JavaScript shim over the native `Event`.
- The generator gained boolean attribute setters, `DOMHighResTimeStamp`
  (`double`), `sequence<Interface>` results, nullable interface getters,
  dictionary constructor arguments, and integer-constant lexing (weedle reads
  `0` as an octal literal).
- The trusted-event bridge (`__tbDispatchTrusted`) moved off the macro and is
  installed on the `EventTarget` prototype. Nodes still receive only the three
  listener methods through `brands.js`.
- The custom-element shim now upgrades the arguments of insertion operations
  too, because the engine adopts a cross-document node by materializing a copy.
  It also enqueues a reaction per insertion instead of deduplicating, which
  restores the spec's constructor-recursion ordering.

Verification for this slice (`/tmp/opencode/jsbinding-research/` was cleared by
a server restart and rebuilt; `probe.py` and `compare-wpt.py` are the current
harness):

- `check.log`: `tools/check` clean.
- `probe-current.log`: 13/13 public-boundary cases pass; `probe-before.log`
  shows the baseline fails two (no `Attr` Node inheritance, no synchronous
  disconnect).
- `after.json` vs `before.json` (826-file `dom/nodes`, `domparsing`,
  `dom/events`, `custom-elements`): 1,545 FAIL-to-PASS, 5 ERROR-to-OK, 6
  previously-missing subtests now pass.
- The only status regressions are four `custom-elements/state/*` files:
  they call the unimplemented `attachInternals`, and custom-element callback
  errors now reach `window.onerror` as the spec requires, so the harness marks
  the file as an error instead of a failing subtest. No behavior these files
  test is implemented either way.
- `release.log`: stripped shipping binary 8,833,368 bytes.

Verification files are in `/tmp/opencode/jsbinding-research/`:

- `final4-tools-check.log` and `final4-workspace.log` pass.
- `final4-scratch.json`: 29/29 subtests pass across observer conversion,
  routing, callback cancellation, reentry, and Node setters. Before the review
  fixes, all 13 observer cases failed and the separate reentry file crashed.
- `final4-wpt.json`: identical 757-entry inventory and zero status changes
  against `observer-review-wpt-after.json` plus `setter-before.json`.
  The inventory includes harness results. Existing failures remain failures.
- `final4-playwright.log`: 15/15. `final4-cdp.log`: corpus PASS.
- `final-fork-clippy.log`: rquickjs-core all-target clippy passes with warnings
  denied. `final2-utf16-valgrind.log` exercises `to_utf16` and `from_utf16`
  round trips with zero errors and zero lost bytes.
- `final2-miri.log` and `final2-valgrind-tests.log` pass.
  `journal-order.log` and `journal-order-miri.log` verify chronological merge
  and idempotent reattachment through the public DOM journal API.
- `final4-release.log`: stripped shipping binary 8,866,568 bytes.
- Teardown fix: `/tmp/opencode/jsbinding-research/observer-lifecycle/src/main.rs`
  creates three `EmbeddedRenderer`s, each with iframes and a cross-realm
  observer, then asserts its host is released after drop. `teardown-owner-after.log`
  passes; the pre-fix run held a second host reference (`teardown-owner-before.log`).
  `final2-teardown-valgrind.log`: zero errors, zero definite/indirect bytes.
  The prior pre-fix run (`observer-coordinator-valgrind.log`) kept about 14.4 MB.
- Surrogate fix: `surrogate-after3.json` runs 15/15 subtests in
  `webidl/node-setters.html` for character-data code units, including
  `innerHTML`/`outerHTML`/`cloneNode`/`isEqualNode` and `MutationRecord.oldValue`.
  `final4-scratch.json` repeats the full 29-subtest set on the final tree.
  `surrogate-before.json` shows the two code-unit cases dying with a UTF-8
  conversion error before the fix.

Next:

1. Migrate `Document`, `Element`, and form-control members still using
   `#[rquickjs::methods]` (`crates/renderer/src/js/bindings/node.rs`).
2. Add exotic collections (`NodeList`, `HTMLCollection`, `DOMTokenList`
   indexed/named hooks) and richer argument/result mappings: variadics,
   dictionaries with inheritance, and interface results such as `Document?`
   and `Element?`.
3. Migrate the remaining private host helpers.
4. Publish the rquickjs commits and bump the submodule pointer when push is
   authorized. Keep the fork's checks separate from the browser workspace.

Decisions made:

- Build-time Rust codegen with `weedle` through renderer `build.rs`; no
  Node/npm step. Checked-in IDL plus explicit Rust mappings are the surface;
  generated tables live in `OUT_DIR`.
- One shared `NativeFunc` `HostCall` per member; distinct JS function objects,
  separate platform algorithms. Receiver checks precede arity/conversion.
- Submodules over subtree/monorepo for the engine forks: single checkout for
  context, `path` patches for wiring, upstream pulls stay `git fetch` inside
  the submodule. See `AGENTS.md` Dependencies section.
- Checked-in IDL is the interface surface: implementation follows IDL, never
  the reverse. Do not edit IDL to match our code.
- Mutation routing belongs to the shared window agent, not a thread-global list
  of worlds. One ordered pending set and one notification flag cover its frames.
  Matching follows ancestor/registration order and pending insertion order.
  Chromium's creation-priority sorting disagrees with that DOM algorithm.
- Borrow discipline: no `World` borrow is held across JS execution, parser
  delivery, or the drain fan-out. `Document::adopt_pending_frames` no longer
  holds its world's guard across frame materialization for exactly this
  reason. A `borrow_mut` temporary in a `for` head lives for the whole loop;
  always bind taken collections first.
- Fork policy: keep forks' own CI as the gate; keep downstream patches
  minimal and tested so upstream pulls stay clean. Do not copy tinybrowser's
  lint config into the forks.
- DOM strings are code-unit sequences. `dom::DomString` is the storage type;
  features that need UTF-8 (layout text, selector matching, form values) call
  `to_string_lossy` and accept U+FFFD for an unpaired surrogate. Serialization
  keeps code units so `innerHTML` matches the browsers.
- One owner per resource. The renderer JS runtime lives in the `Engine`; every
  other holder is a weak handle, and realms clone the rquickjs `Runtime` only
  while the realm is alive.

Gotchas:

- Fresh clones and worktrees need `git submodule update --init --recursive`.
- Correct WPT syntax: `tools/wpt/run --score webidl/ --save-report FILE --
  --exclude=worker --processes 4`, with `TINYBROWSER_BINARY` selecting a
  preserved build. This milestone's baseline is
  `/tmp/opencode/jsbinding-research/before.json` (built from `d80b626` into
  `tinybrowser-before`); compare with
  `python3 /tmp/opencode/jsbinding-research/compare-wpt.py BEFORE AFTER`. Older
  baselines are `webidl-full-before.json` plus `mutation-before.json`.
- Never add handwritten `unsafe` in tinybrowser-owned code; workspace denies
  `unsafe_code`. Do not test spec conformance in cargo tests.

## Remaining limitations

- Callback declarations validate the invocation shape. Observer delivery still
  supplies the typed argument contract by hand.
- Dictionary inheritance and fields other than booleans/string sequences are
  rejected. Constructors support strings and callbacks.
- Setters support method-mapped DOMString and DOMString? only. Other types and
  extended attributes still require generator work.
- The DOM's UTF-8 storage predates this migration. Character data now keeps
  exact UTF-16 code units. Attribute values (`setAttribute`, `Attr.value`) and
  form-control values still store Rust `String`, so a lone surrogate there is
  rejected or replaced. `Text.splitText` is not implemented.
- `[CEReactions]` lowers to `reactions::with_reactions` around the platform
  step, but only the migrated operations carry it; the shim still pushes its own
  scope for `define()` and `upgrade()`.
- Renderer teardown no longer leaks: the runtime has a single owner and
  JavaScript host closures can no longer form a cycle with the heap.
- `Text.normalize()` removes an empty receiver in the existing implementation,
  whereas the DOM algorithm visits descendant Text nodes. The chronology probe
  that exposed the routing bug used that separate, preexisting spec defect.
- `Document`, `Element`, form controls, exotic collections, and most private
  helpers still use rquickjs macros.
