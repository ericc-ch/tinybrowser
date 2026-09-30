# Handoff (2026-09-30)

Goal: Replace tinybrowser's whole native JS binding layer with browser-specific
WebIDL codegen plus shared dispatch, to shrink the shipping binary. Whole-layer
migration is in progress; only the first slice has landed.

Plan: Extend the `webidl-bindgen` type subset one feature at a time
(callbacks, then setters, exotic collections, dictionaries, missing result
types), migrating one interface per feature, verifying each step with
`tools/check`, `cargo test --workspace`, and the WPT slice before moving on.

State: `tools/check` and `cargo test --workspace` are green on the current
worktree. The committed slice (see Done) is fully verified. The submodule
wiring (see Unfinished) is verified through `tools/check` + tests but
uncommitted.

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

Unfinished:

- Submodule mono-checkout (worktree only, uncommitted): `third_party/rquickjs`
  submodule at `d73fbda` with nested `sys/quickjs` at `712757e`;
  `[patch.crates-io]` switched from `git rev` to `path` in `Cargo.toml`;
  workspace `exclude` added; `AGENTS.md` and `third_party/VENDORED.md` updated.
  Resume point: verify `git status` shows `.gitmodules`, `Cargo.toml`,
  `Cargo.lock`, `AGENTS.md`, `third_party/VENDORED.md`, and the
  `third_party/rquickjs` gitlink, then commit. Only the `sys/quickjs/test262`
  corpus is unchecked out (not needed for the build).
- Whole-layer migration: everything except `DOMException`, `MutationRecord`,
  and the eleven `Node` members is still macro-bound.

Next:

1. Commit the submodule wiring.
2. Implement callback support in `host.rs` (storage, invocation, error
   reporting), then migrate `MutationObserver`.
3. Add setter support, migrate `nodeValue`/`textContent` plus form controls.
4. Add dictionary support, migrate `Event`/`EventTarget` construction.
5. Continue across remaining nodes, collections, observers, constructors, and
   private host helpers.

Decisions made:

- Build-time Rust codegen with `weedle` through renderer `build.rs`; no
  Node/npm step. Checked-in IDL plus explicit Rust mappings are the surface;
  generated tables live in `OUT_DIR`.
- One shared `NativeFunc` `HostCall` per member; distinct JS function objects,
  separate platform algorithms. Receiver checks precede arity/conversion.
- Submodules over subtree/monorepo for the engine forks: single checkout for
  context, `path` patches for wiring, upstream pulls stay `git fetch` inside
  the submodule. See `AGENTS.md` Dependencies section.
- Fork policy: keep forks' own CI as the gate; keep downstream patches
  minimal and tested so upstream pulls stay clean. Do not copy tinybrowser's
  lint config into the forks.

Gotchas:

- Blocker details live below; the short version is the generator's type
  subset. The easy types are done. Everything left needs a type the compiler
  rejects today.
- Fresh clones and worktrees need `git submodule update --init --recursive`.
- Correct WPT syntax: `tools/wpt/run --score webidl/ --save-report FILE --
  --exclude=worker --processes 4`, with `TINYBROWSER_BINARY` selecting a
  preserved build. Baseline
  `/tmp/opencode/jsbinding-research/webidl-full-before.json` plus
  `mutation-before.json`.
- Never add handwritten `unsafe` in tinybrowser-owned code; workspace denies
  `unsafe_code`. Do not test spec conformance in cargo tests.

## Migration blockers (generator type subset)

Immediate: callbacks. `MutationObserver` takes a `MutationCallback` in its
constructor. The generator only accepts `DOMString` constructor args and
`Node`/`Node?` operation args. A callback is not a value you convert and
forget. It needs rooting so GC keeps it alive across tasks, later invocation
from `deliver_mutations` with `(records, observer)` args, execution in the
creation realm, and exception reporting in that realm. The cross-realm
callback WPT test fails today, which is exactly this behavior. No IDL lands
for `MutationObserver` until callback storage, invocation, and error reporting
exist in `host.rs`.

Behind it, in order of difficulty:

- Setters. `nodeValue`, `textContent`, and form controls have them. The model
  only accepts `readonly` attributes, and install must define getter and
  setter as one accessor, not two calls where the second wipes the first.
- Exotic collections. `NodeList`/`HTMLCollection` need indexed and named
  properties through exotic hooks, not plain members. Different machinery from
  the member table.
- Dictionaries. `Event`/`MutationObserverInit` constructors take option
  dictionaries (`EventInit`, `{childList, subtree, ...}`). No dictionary
  parsing, defaults, or conversion exists.
- Missing result types. `ownerDocument` returns `Document?`, which the
  compiler has no name for.

Not blockers anymore: the fork is published and pinned (`d73fbda`),
realm-aware callbacks and constructor prototype fallback work, and partial
interfaces with operations and readonly attributes are proven by 11 migrated
members with zero WPT regressions.
