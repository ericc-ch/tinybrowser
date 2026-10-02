# Handoff (2026-10-02)

Goal: Hide browser internals from page scripts while retaining the JS Web APIs.
All shim-private state, CDP storage, and WebDriver helpers are in scope.
Plan: Private initialization and callbacks, private object state, privileged-path
hardening, then security tests, WPT comparisons, and shipping-size measurement.
Finish when only intended APIs are reachable and the audited paths do not leak
capabilities. Fix regressions before committing locally. Pushing is not authorized.

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
- Shipping binary is 8,697,592 bytes (`docs/progress.md`). Verify with
  `nix develop --command ./tools/release`.
- Last migration evidence (`/tmp/opencode/`): `before-domcore.json` vs
  `after-domcore.json` over `dom/nodes`, `dom/collections`, `shadow-dom`,
  `css/cssom-view`, `html/dom`, `domparsing` gives 749 FAIL-to-PASS, 38
  MISSING-to-PASS, 1 ERROR-to-OK, and no `PASS`/`OK`-to-worse change; the
  interrupted files include
  (`shadow-dom/declarative/gethtml.html`, `html/dom/reflection-embedded.html`)
  that the runner interrupts in both runs. Those reports also contain adverse
  FAIL-to-MISSING changes. `before-forms.json` vs `after-forms.json` over
  `html/semantics/forms` gives 2 FAIL-to-PASS. Both reports need focused retests
  of adverse ERROR/TIMEOUT changes before claiming conformance is unchanged.
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

## Private bridge

`crates/renderer/src/js/bridge.rs` passes a private host object to initialization
closures. Rust retains that object in each World. `scripts/private_state.js`
stores object state in shared weak maps owned by RealmRegistry. The shared
factory runs in an inert context. Realm leases release the factory after the
last realm drops. Author evaluation uses a separate global evaluator. CDP and
WebDriver encode author code before passing it to that evaluator.

Private lists use captured indexed operations and null-prototype descriptors.
Private registries use captured collection operations. Blob bytes, decoder
state, and XHR received bytes live in the inert context. Input delivery uses
captured constructors and native trusted dispatch. Debugger metadata uses
private tables and captured serialization. Shared Window identity covers realm
globals and WindowProxy objects. Private array iterators retain terminal state.

Verification for the shipping candidate:

- `tools/check`: `/tmp/opencode/bridge-check.log`.
- Workspace tests: `/tmp/opencode/bridge-tests.log`.
- Playwright 37/37, including 22 bridge regressions:
  `/tmp/opencode/bridge-shipping-playwright.log`.
- Blink CDP 1/1: `/tmp/opencode/bridge-cdp.log`.
- Read-only reviews found Window receiver routing and iterator exhaustion
  regressions. Runtime regressions pass after the fixes. A missed Window-table
  rename also passes the follow-up serialization check. The trusted-message
  regression sends an object to cover that path.
- Valgrind's JS ownership tests reported zero definite leaks and zero errors:
  `/tmp/opencode/bridge-valgrind-js.log`. The full renderer report's definite
  leaks trace to Stylo thread-local caches:
  `/tmp/opencode/bridge-valgrind-full.log`.
- Final WPT comparisons cover 417 files with no adverse status changes.
  The 278 storage/event/URL/Blob/XHR/FileReader files have identical statuses.
  The 139 messaging and structured-clone files have 7 harness TIMEOUT-to-OK,
  5 subtest TIMEOUT-to-PASS, and 2 subtest NOTRUN-to-PASS changes. Report pairs
  are `before-bridge-wpt.json` versus
  `final-bridge-wpt.json`, and `before-bridge-messaging-wpt.json` versus
  `after-bridge-messaging-wpt.json`, all in `/tmp/opencode/`. The additional XHR
  and FileReader reports are `before-bridge-io-wpt.json` and
  `after-bridge-io-wpt.json`. Comparison files are `bridge-wpt-changes.json`,
  `bridge-io-wpt-changes.json`, and `bridge-messaging-wpt-changes.json`.
  The runs still exit 1 because existing failures remain. These focused
  comparisons do not establish full Web-platform conformance.

## Remaining

1. Replace hand-maintained IDL partials and embedded `Rust*` annotations with
   pinned upstream IDL inputs, separate implementation mappings, and support
   selection. Resolve spec inheritance and mixins in the generator. Follow
   `docs/bindings.md` for ownership and coverage rules.
2. Resolve the older WebIDL-migration adverse status changes listed above before
   claiming migration conformance is unchanged.
3. Preexisting conformance gaps unrelated to the bridge: lossy Rust
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
