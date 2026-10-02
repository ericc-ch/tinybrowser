# Handoff (2026-10-02)

Goal: replace implementation-shaped WebIDL with the unchanged, pinned upstream
contract. Every Rust and JS binding derives names, inheritance, descriptors,
arity, and conversions from that IDL. Delete the hand-written `Rust*` partial
declarations and the parallel support lists as each interface migrates. Keep the
private bridge hardening and the real partial implementations. See
`docs/bindings.md` for the policy and `AGENTS.md` for the rules.

Plan: finish the generator's remaining IDL coverage, port the remaining
interfaces, then delete the legacy path (`crates/renderer/idl/`, the legacy
`model.rs`/`emit.rs` halves, `CtxMode`, `RustAlternate`, `RustInstall`) in one
large commit. Verification is WPT before/after per batch against the previous
binary.

State: branch `webidl-bindings`, HEAD `7a34075`, working tree clean (only
`docs/HANDOFF.md` uncommitted as this note is written). `cargo test --workspace`
and `tools/check` pass (36 suites). No pushes. Draft PR
https://github.com/ericc-ch/tinybrowser/pull/39 holds earlier work only.

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
