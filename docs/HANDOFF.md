# Handoff (2026-10-02)

Goal: replace implementation-shaped WebIDL with the unchanged, pinned upstream
contract. Every Rust and JS binding derives names, inheritance, descriptors,
arity, and conversions from that IDL. Delete the hand-written `Rust*` partial
declarations and the parallel support lists as each interface migrates. Keep the
private bridge hardening and the real partial implementations. See
`docs/bindings.md` for the policy and `AGENTS.md` for the rules.

Plan: finish the native contract path and the JS contract path, migrate the
remaining `crates/renderer/idl/*.webidl` interfaces one at a time, then delete the
legacy compiler and its inputs. Conformance comes from WPT, size from the
release binary.

State: branch `webidl-bindings`, HEAD `24d3aba`, working tree clean. The
repo-local gates are green: `cargo test --workspace` and `tools/check` pass
(log `~/.cache/tinybrowser/logs/parentnode-tests.log`, 36 suites ok). No pushes.
Draft PR https://github.com/ericc-ch/tinybrowser/pull/39 holds earlier work only.

Done (this session):

- `0bce44f` Reject scoped and unforgeable native members. Proof: generator tests
  `unforgeable_and_scoped_native_members_fail_the_build`.
- `f8dd676` Dispatch generated operations through their contract traits. Proof:
  renderer builds; DOM WPT `Element-matches` 668 PASS.
- `ec7859e` Generate JavaScript interface bindings from IDL contracts, including
  the private installer and the `TextEncoder` migration. Proof: WPT focused run
  `~/.cache/tinybrowser/logs/after-js-focused.json` moved 11 `encodeInto`
  subtests FAIL to PASS with no regressions; generator tests; Playwright 37/37
  (`js-contract-playwright.log`); CDP pass (`js-contract-cdp.log`).
- `e5aa2c0` Remove nonfunctional API placeholders. Proof: WPT focused run above;
  the only changes are the approved honesty losses in `/selection` and `/beacon`.
- `df580b1` Migrate HTMLElement and SVGElement to their IDL contracts, plus
  `getElementById("")` returns null. Proof: WPT
  `~/.cache/tinybrowser/logs/after-element-contracts2.json` (both getElementById
  files fully pass; dataset failures are pre-existing missing `DOMStringMap`).
- `87744a3` Generate enum-typed attribute bindings. Proof: generator tests.
  No interface consumes this yet.
- `24d3aba` Generate union conversions and migrate ParentNode and ChildNode.
  Unions flatten like Chromium, convert platform objects before strings per the
  WebIDL algorithm, and reject unsupported members at build time. `[Unscopable]`
  emits merging `@@unscopables`. Mixin members and install targets resolve from
  IDL includes. Proof: generator tests (conversion order, rejections,
  unscopables, mixin targets); WPT 9 tree files with zero status changes
  against a pre-migration baseline binary
  (`~/.cache/tinybrowser/logs/before-parentnode.json` vs
  `after-parentnode.json`); workspace tests and `tools/check` green; Playwright
  36/37 plus Sauce retry pass; CDP pass.

Earlier sessions (unchanged): `a21f8a9` IDL import, `064baa2` NodeList and
HTMLCollection, `9adbb7b` MutationRecord, `4930cbc` parsing and observers,
`02f9f7a` DOMTokenList, `fe0d197` shared node interfaces, `682d273` Attr.

Unfinished:

- Remaining `crates/renderer/idl/*.webidl`: `Document`, `DOMParser`, `Element`,
  `ElementReflections`, `Event`, `EventTarget`, `HTML*` forms,
  `HTMLOptionsCollection`, `NamedNodeMap`, `Node`, `NonDocumentTypeChildNode`,
  `ShadowRoot`. Next generator features, each landed with a consumer migration:
  - Union members beyond Node and DOMString: `(boolean or dictionary)` for
    `scrollIntoView`, `(TrustedHTML or DOMString)` for `innerHTML` et al,
    interface unions for `HTMLOptionsCollection.add`, nullable callbacks for
    `EventTarget`. Each needs its conversion arm plus a consumer migration.
  - `[Reflect]` / `[ReflectSetter]` handling (form elements, `ElementReflections`).
    Decide whether the generator emits the reflection algorithm (end goal) or
    treats the extended attribute as metadata the implementation owns.
  - Interface-typed arguments, e.g. `NamedNodeMap.setNamedItem(Attr attr)`.
    `argument_parameter` has no `ReturnType::PlatformObject` arm, and
    `NamedNodeMap` needs a JS-implemented property-hook mode the contract path
    lacks.
  - Enum attributes are generated (`87744a3`) but no interface consumes them
    yet. `Document.readyState` and `ShadowRoot.mode` will once their unions land.
  - Interface-level metadata such as `[LegacyFactoryFunction]` on
    `HTMLImageElement`, rejected by `validate_interface_attributes`.
- `brands.js` still carries hand-written per-interface member lists (a second
  support list). Reduce them as interfaces move to contracts. Do not blanket
  delete without checking that the list is not the only installer for a member.
- `/selection` lost the fixed placeholder and now has no Selection at all. There
  is no native `Selection` (grep in `crates/renderer/src`). Implement it as its
  own feature, not a placeholder.
- `docs/progress.md` still records 8,699,448 bytes from before this session. Only
  update it after measuring a new release binary.
- Older WPT comparisons (`before/after-domcore.json`, `before/after-forms.json`)
  are in `/tmp/opencode`; move to `~/.cache/tinybrowser/logs` before `/tmp` is
  cleared.

Next:

1. Design `[Reflect]` support in the generator and verify against one form
   element (`HTMLOptGroupElement` has a single `[CEReactions, Reflect] boolean
   disabled`), then batch the rest.
2. Add union support, starting with `(Node or DOMString)` for `ParentNode` and
   `ChildNode`, then `Element` and `Document`.
3. Add `ReturnType::PlatformObject` arguments for `NamedNodeMap`.
4. Keep running `cargo test --workspace`, `tools/check`, and the targeted WPT
   subset per migration. Measure the release binary at each larger checkpoint.

Decisions made:

- Discovery is the implementation itself: `impl {interface}_generated::{Interface}
  <'js> for Payload` for Rust, `__tbInstallInterface(class Interface { ... })`
  for JS. No second checklist. Unsupported implemented semantics fail the build.
- The IDL is never edited to fit code; reshape the implementation instead.
- Generated dispatch calls the contract trait explicitly
  (`Interface::method(receiver, ...)`) so shared payloads with same-named
  inherent methods cannot shadow it.
- JS results allocate in the receiver's realm via a per-instance realm token on
  the shared brand slot.
- Generated dictionaries allow unconsumed fields, since an algorithm may read a
  subset of the declared members.
- Nonfunctional placeholders are removed; feature detection must report them
  unsupported.
- Union conversion tries platform objects before strings, with a strict node
  probe (`Attr` stringifies). Mixin installs derive targets from IDL includes.

Gotchas:

- Never `git submodule update --init --recursive` in a worktree: it clones the
  1.2 GB WPT checkout fresh. Init `third_party/rquickjs` only; WPT resolves to
  the primary checkout, and the rquickjs fork pin may not fetch (copy the dir).
- Never `git add` a deleted path alongside modifications: the pathspec error
  aborts the whole add and the commit lands partial. Stage deletions via
  `git rm` first, or add surviving paths only.

- One WPT run at a time: the runner serializes on a venv lock and the build
  holds the Cargo target. The `/encoding` legacy multibyte files dominate runtime.
- `cargo build` writes generated contracts to
  `OUT_DIR`; `crates/renderer/src/js/blob.rs::blobs_round_trip` re-runs the JS
  compiler on the bundle, so a generator change must keep it consistent.
- The renderer lists each generated `install` by interface module name in
  `crates/renderer/src/js/bindings/node.rs`; deleting a legacy IDL file without
  adding the contract impl breaks that call.
- The Sauce Demo Playwright case hits a live site and is occasionally flaky; it
  passes on retry.
