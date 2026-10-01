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
- Shipping binary is 8,670,008 bytes (`docs/progress.md`). Verify with
  `nix develop --command ./tools/release`.
- Last migration evidence (`/tmp/opencode/`): `before-domcore.json` vs
  `after-domcore.json` over `dom/nodes`, `dom/collections`, `shadow-dom`,
  `css/cssom-view`, `html/dom`, `domparsing` gives 749 FAIL-to-PASS, 38
  MISSING-to-PASS, 1 ERROR-to-OK, and no `PASS`/`OK`-to-worse change; the
  only `MISSING`/`TIMEOUT` entries are two long files
  (`shadow-dom/declarative/gethtml.html`, `html/dom/reflection-embedded.html`)
  that the runner interrupts in both runs. `before-forms.json` vs
  `after-forms.json` over `html/semantics/forms` gives 2 FAIL-to-PASS and no
  real regression.
- Verification harness: `tools/check` (clippy, embedded JS, rustdoc),
  `cargo test --workspace`, the 30-case probe at
  `/tmp/opencode/jsbinding-research/probe.py <binary>`, and before/after WPT via
  `/tmp/opencode/jsbinding-research/compare-wpt.py BEFORE AFTER`. Playwright
  (`tools/playwright/run`) and Blink CDP (`tools/cdp-tests/run`) honor
  `TINYBROWSER_BINARY`.

## Migrated (generated IDL modules)

The whole native binding layer is generated. Every `JsNode` member is either a
generated table entry or a `#[qjs(skip)]` platform method; no interface member
is installed by `#[rquickjs::methods]` anymore.

DOM: `Node`, `Document`, `DocumentFragment`, `Element`, `HTMLElement`,
`SVGElement`, `MathMLElement`, `CharacterData`, `DocumentType`,
`ProcessingInstruction`, `Attr`, `DocumentImplementation`, `DOMParser`,
`XMLSerializer`, `EventTarget`, `Event`, `MutationObserver`, `MutationRecord`,
`DOMTokenList`, `NamedNodeMap`, `NodeList`, `HTMLCollection`,
`HTMLOptionsCollection`, `DOMException`.

Mixins and shared groups: `ParentNode`, `ChildNode`,
`NonDocumentTypeChildNode`, `ShadowRoot`, `ElementReflections` (the element-wide
`name`/`href`/`src`/`content` reflections).

HTML form controls and elements: `HTMLFormElement`, `HTMLInputElement`,
`HTMLTextAreaElement`, `HTMLSelectElement`, `HTMLOptionElement`,
`HTMLButtonElement`, `HTMLFieldSetElement`, `HTMLOptGroupElement`,
`HTMLIFrameElement`, `HTMLImageElement`.

## Remaining

1. Pure-JS interface shims in `crates/renderer/src/js/scripts/web/*.js` are a
   separate, larger scope (encodings, file/fetch, messaging, storage, CSSOM,
   XHR, navigator, streams, events, and the form shims that extend generated
   interfaces: `type`, `files`, `validity`, `setCustomValidity`, `size`,
   `item`, `namedItem`, label `form`, form `elements`/`length`).
2. Internal `__tb*` host bridges (about 48) still duplicate some spec behavior
   (`__tbWindowNamedValue`/`Has`, `__tb_refreshNamedNodeMap`, `__tbMakeDataset`,
   handler tables).
3. Preexisting conformance gaps unrelated to the binding layer: lossy Rust
   `String` attribute/form storage, `Text.splitText`, `attachInternals`,
   copied cross-document adoption, iframe `Window` identity, incomplete
   iterator methods.
4. `docs/progress.md` WPT totals are still the pre-migration overnight dump;
   rerun the full scorer to refresh the scored groups.

## Durable decisions

- Build-time Rust codegen with `weedle` from renderer `build.rs`; no npm step.
  Checked-in IDL plus explicit Rust mappings are the surface. Generated tables
  live in `OUT_DIR`. Implementation follows IDL; never edit IDL to match code.
- `[Rust=path]`, `[RustValue]`, `[RustFromJs=path]`, `[RustSetFromJs=path]`,
  `[RustAlternate=Type]`, `[RustPropertyHooks=Indexed|IndexedNamed|JavaScript]`,
  `[RustSupportedNames=path]`, `[RustInstall="A,B"]` (mixin install targets),
  and `[RustOwnedCtx]` (attribute getters/setters take `Ctx` by value) are the
  escape hatches. `[RustValue]` getters return the platform `Value` directly;
  arguments converted with `[RustFromJs]`/`[RustValue]` bypass generated
  conversion. Generated operation dispatch always passes an owned `Ctx`;
  attribute dispatch borrows unless `[RustOwnedCtx]` is set.
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
