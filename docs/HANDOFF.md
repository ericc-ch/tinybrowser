# Handoff (2026-10-03)

Goal: replace implementation-shaped WebIDL with the unchanged, pinned upstream
contract. Every Rust and JS binding derives names, inheritance, descriptors,
arity, and conversions from that IDL. See `docs/bindings.md` for the policy
and `AGENTS.md` for the rules.

State: branch `webidl-bindings`, HEAD `1830376`, working tree clean.
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

## Remaining legacy (8 files)

`Document`, `Element`, `ElementReflections`, `EventTarget`, `Event`,
`HTMLOptionsCollection`, `NamedNodeMap`, `Node`.

Each needs:

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
