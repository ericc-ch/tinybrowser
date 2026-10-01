# Handoff (2026-10-01)

Goal: Replace tinybrowser's whole native JS binding layer with browser-specific
WebIDL codegen plus shared dispatch, so the shipping binary shrinks. The
`#[rquickjs::methods]` class in `crates/renderer/src/js/bindings/node.rs` is the
last big native surface; every interface is being moved to a checked-in
`.webidl` file under `crates/renderer/idl/` whose generated `OUT_DIR` module
installs members onto the interface prototype and dispatches to Rust platform
methods marked `#[qjs(skip)]`.

## State

- Branch `webidl-bindings`. HEAD is local-only; the branch is many commits
  ahead of `origin/webidl-bindings`. Draft PR:
  https://github.com/ericc-ch/tinybrowser/pull/39. Do not push without
  authorization.
- Engine forks: rquickjs `github.com/ericc-ch/rquickjs` (`third_party/rquickjs`
  submodule, own workspace), QuickJS-NG `github.com/ericc-ch/quickjs` (nested
  `sys/quickjs`). Upstream pulls happen inside the submodule, then the pointer is
  bumped here. Fresh worktrees need
  `git submodule update --init --recursive`.
- Shipping binary is 8,754,040 bytes (`docs/progress.md`). Verify with
  `nix develop --command ./tools/release`.
- Verification harness: `tools/check` (clippy, embedded JS, rustdoc),
  `cargo test --workspace`, the 30-case probe at
  `/tmp/opencode/jsbinding-research/probe.py <binary>`, and before/after WPT via
  `/tmp/opencode/jsbinding-research/compare-wpt.py BEFORE AFTER`. Playwright
  (`tools/playwright/run`) and Blink CDP (`tools/cdp-tests/run`) honor
  `TINYBROWSER_BINARY`.

## Migrated (generated IDL modules)

`Node`, `Document`, `DocumentFragment`, `Element`, `HTMLElement`, `SVGElement`,
`MathMLElement`, `CharacterData`, `DocumentType`, `ProcessingInstruction`,
`Attr`, `DOMException`, `DOMImplementation`, `DOMParser`, `XMLSerializer`,
`EventTarget`, `Event`, `MutationObserver`, `MutationRecord`, `DOMTokenList`,
`NamedNodeMap`, `NodeList`, `HTMLCollection`, `HTMLOptionsCollection`.

## Remaining

1. Native JS-visible members still on `JsNode` in `node.rs` (~130 bindings):
   - DOM core: ParentNode (`children`, `firstElementChild`, `lastElementChild`,
     `childElementCount`, `append`, `prepend`, `replaceChildren`,
     `querySelector`, `querySelectorAll`), ChildNode (`before`, `after`,
     `replaceWith`, `remove`), NonDocumentTypeChildNode
     (`previousElementSibling`, `nextElementSibling`), Element
     (`matches`, `closest`, `getElementsByClassName`,
     `innerHTML`/`outerHTML`/`insertAdjacentHTML`,
     `getBoundingClientRect`/`getClientRects`/`scrollIntoView`/`scrollLeft`/
     `scrollTop`, `attachShadow`/`shadowRoot`, `style`, `href`/`src`), ShadowRoot
     (`host`, `mode`), HTMLElement (`click`, `focus`, `blur`, `title`), Document
     (`write`, `activeElement`, `title`).
   - HTML form controls: `HTMLFormElement`, `HTMLInputElement`,
     `HTMLTextAreaElement`, `HTMLOptionElement`, `HTMLSelectElement`,
     `HTMLButtonElement`, `HTMLFieldSetElement`, `HTMLLabelElement`,
     `HTMLOptGroupElement`.
   - Other HTML: `HTMLIFrameElement`, `HTMLImageElement`, `HTMLMetaElement`.
2. Generator features those need: variadic union arguments
   (`(Node or DOMString)...`), overloaded operations, required dictionary
   fields, attribute/setter types beyond
   String/NullableString/Boolean/UnsignedLong/Long (nullable numerics, enums,
   `USVString`, nullable interface returns), interface names in
   argument/return position, `[PutForwards]` on nullable attributes.
3. Pure-JS interface shims in `crates/renderer/src/js/scripts/web/*.js` are a
   separate, larger scope (encodings, file/fetch, messaging, storage, CSSOM,
   XHR, navigator, streams, events). Not part of the native-layer goal unless
   asked.
4. Internal `__tb*` host bridges (about 48) still duplicate some spec behavior
   (`__tbWindowNamedValue`/`Has`, `__tb_refreshNamedNodeMap`, `__tbMakeDataset`,
   handler tables).

## Durable decisions

- Build-time Rust codegen with `weedle` from renderer `build.rs`; no npm step.
  Checked-in IDL plus explicit Rust mappings are the surface. Generated tables
  live in `OUT_DIR`. Implementation follows IDL; never edit IDL to match code.
- `[Rust=path]`, `[RustValue]`, `[RustFromJs=path]`, `[RustSetFromJs=path]`,
  `[RustAlternate=Type]`, `[RustPropertyHooks=Indexed|IndexedNamed|JavaScript]`,
  `[RustSupportedNames=path]` are the escape hatches. `[RustValue]` getters
  return the platform `Value` directly; arguments converted with
  `[RustFromJs]`/`[RustValue]` bypass generated conversion.
- One shared `NativeFunc` `HostCall` per member. Distinct JS function objects,
  one dispatch table per interface. Receiver downcast precedes arity and
  conversion. Generated wrappers use `host::instance` or the node-associated
  `host::instance_for_node` (document owner realm), never a runtime-wide
  prototype cache.
- `JsNode` is the single wrapper class for every node kind. `brands.js` builds
  the JS interface prototypes by copying native descriptors from
  `Node.prototype`, then each generated partial interface's `install()`
  overwrites its own members. Marking a native method `#[qjs(skip)]` is what
  removes it from the native prototype surface.
- `RealmRegistry` owns the agent-wide attribute registry (globally issued ids,
  attachment index, weak wrappers), mutation observers, custom-element reaction
  queues, and document-owner routing. Adoption preserves creation scope and
  wrapper identity.
- `[CEReactions]` lowers to `reactions::with_reactions` around the platform
  steps after argument conversion. Callback exceptions are reported; platform
  exceptions keep identity.
- Cross-document insertion materializes a copy; the shim upgrades the original
  argument wrappers and preserves function length/name.
- `dom::DomString` keeps exact UTF-16 code units and compacts ordinary Unicode
  to UTF-8. Attribute values, form values, and `Text.splitText` still use Rust
  `String`, so lone surrogates there are lossy.
- Decisions live in commit messages.

## Gotchas

- Correct WPT invocation:
  `TINYBROWSER_BINARY=<binary> nix develop --command tools/wpt/run --processes N
  --log-wptreport OUT.json <dirs...>`, compare with `compare-wpt.py`.
- Direct cargo and the runners build through `nix develop --command` (host shell
  may lack `pkg-config`/OpenSSL). `tools/check` enters the shell itself.
- Do not test web-platform behavior in cargo tests; that is WPT's job. When a
  spec regression would only be caught by a cargo test, the missing WPT run is
  the bug.
- Never add handwritten `unsafe` in tinybrowser-owned code; the workspace denies
  `unsafe_code`.
- The rquickjs checkout's exported slice (one `MethodImplementor` impl per
  class) is why all node kinds share `JsNode`.
