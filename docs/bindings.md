# Web API bindings

Rules for JS/Rust ownership, binding inputs, and conformance reports.

## Rust binding vs JS

Choose the language by ownership of state and authority, not by the API's name.

- Rust owns the DOM tree, parsing, selector matching, layout, and native wrapper
  identity. Tree mutation, cloning, and adoption algorithms stay in Rust.
- Rust owns event dispatch, listener bookkeeping, propagation state, and trusted
  event flags. JS can hold event payloads such as `CustomEvent.detail`.
- Rust owns I/O, browser task scheduling, frame and realm lifetimes, cross-realm
  routing, and origin checks. JS requests these operations through the private
  host bridge. Rust validates each request.
- JS implements algorithms over JS values and private shim state. Examples
  include header-list operations, FormData list operations, and Intl option
  processing. Existing native parsers and formatters remain Rust-backed.
- A JS adapter may call existing DOM bindings to reflect an attribute. Calling
  a DOM method does not make the adapter the owner of the tree algorithm.
- Split mixed APIs at the host operation. For example, JS prepares fetch options
  while Rust performs the network request. JS prepares Intl options while Rust
  formats with ICU4X. Keep each mutable state authoritative in one place.
- Generate binding mechanics from IDL for either implementation language. Keep
  names, inheritance, descriptors, arity, and WebIDL conversions out of
  handwritten platform algorithms. Shared conversion machinery must preserve
  the spec algorithm and order.
- Share the `JsNode` payload across node interfaces. JS brands and generated
  interface prototypes provide the public interface hierarchy.

Keep host capabilities and shim backing state private. Captured operations and
private storage must prevent page-replaced methods, constructors, or inherited
hooks from observing or mutating browser internals. Rust owns native lifetimes.
RealmRegistry owns the lifetime of shared weak-slot storage.

## IDL inputs and implementation contracts

Import an unmodified, pinned upstream IDL snapshot. WPT's `interfaces/` directory
is one source. Its non-tentative files are synced from curated `@webref/idl`
extracts. Keep updates explicit and builds offline. See the
[WPT interface source policy](https://github.com/web-platform-tests/wpt/blob/master/interfaces/README.md).

Implementations follow a standardized contract derived from IDL. JS members use
the IDL names and getter/setter structure. Rust names and signatures follow one
deterministic convention, with distinct operation kinds for methods, attributes,
constructors, and special operations. Validate implementations against the
generated contract. Rename or reshape the implementation when it does not fit.

Do not maintain explicit mapping tables, per-member rename annotations, or
conversion overrides. Generate conversions from the declared IDL types and
standard extended attributes. Resolve inheritance, partial interfaces, and mixin
includes from the imported declarations. Share conversion machinery rather than
adding exceptions for individual implementations.

Imported declarations do not require exposing every API. Derive supported
members from actual implementations, not a second handwritten support list.
Reject conflicting or invalid implementations rather than silently falling back
to another backend. Keep unsupported APIs absent rather than installing dummy
methods that mislead feature detection. If an implemented member needs
unsupported generator semantics, fail the build instead of changing the
declaration or silently omitting the binding.

The imported snapshot lives in `crates/webidl-bindgen/idl/`. Run
`tools/webidl/import` to refresh it from the pinned WPT revision. Write the
`manifest.json` revision and `sha256` map in the same commit. `tools/webidl/import
--check` runs in `tools/check` and rejects local edits to imported files.

The generator lives in `crates/webidl-bindgen/`. `compile_contracts` reads the
imported extracts and Rust implementations. It discovers
`impl foo_generated::Foo<'js> for Payload` blocks in `crates/renderer/src/js`,
resolves partials, mixins, and inheritance across files, and emits one Rust
module per implemented interface. The renderer build writes that module to
`OUT_DIR`. The renderer compiles it, so the generated trait checks each
implementation's signatures. An implemented member whose IDL semantics the
generator cannot yet emit fails the build.

Rust constructors use `constructor`. Attribute methods use `get_name` and
`set_name`. Operations use the snake-case IDL name. Every entry point receives
`Ctx` first, after `self` for instance methods. An operation may also receive
the JS receiver as a leading `Object<'js>` before its IDL arguments. Discovery
recognizes this signature and dispatch supplies the receiver. The generated
trait checks the remaining arguments against IDL-derived types.

If a payload already declares `#[rquickjs::class]`, the generator emits an
installer for its interface prototype instead of another `JsClass`
implementation. This lets node interfaces share one native identity. Existing
classes cannot replace constructors or exotic property hooks through this path.
The payload implements `host::SharedClass` to check each interface's native brand
before generated dispatch calls an algorithm.

Dictionary field names use snake case. Dictionary conversion visits inherited
declarations first and sorts each declaration's members, including partials,
lexicographically. Unsupported defaults and conversions fail the build.

`compile_javascript` discovers `__tbInstallInterface(class InterfaceName { ... })`
calls in embedded scripts. Use the interface's IDL name for the class and each
algorithm method. Use plain parameter names. IDL supplies argument defaults and
conversions. Keep private state outside the class in the shim's private storage.
The build parses the class, checks its members against the imported declarations,
and inserts the binding contract into the installer call before compression.

`crates/renderer/src/js/scripts/bindings.js` installs these JS contracts in the
private bridge scope. The installer supplies brands, descriptors, argument
conversion, and result conversion. The algorithm class stays private. Native,
legacy, and generated JS implementations of the same interface conflict at build
time. Generator dependencies belong to build tooling rather than the browser
runtime.

Implementations follow the pinned upstream extracts in
`crates/webidl-bindgen/idl/` (see the manifest there for the WPT source pin).
The renderer build rejects an interface that has both a legacy and a contract
binding.

The cleanup covers Rust-backed and JS-backed bindings. Audit existing members
before migration. Remove confirmed nonfunctional placeholders, but keep real
partial implementations and report their conformance gaps through WPT. Fix
binding and ownership problems during the cleanup. Implement unrelated missing
browser features as separate work.

## Coverage and conformance

Use WPT for the published Web-platform conformance report, regardless of whether
the implementation is JS or Rust. Include applicable IDL-harness tests for API
structure and behavioral tests for algorithms. IDL-harness passes alone do not
prove behavioral conformance.

Report the suite revision, tested groups, execution context, exclusions, and
test/subtest statuses. Keep failures, errors, crashes, timeouts, skipped cases,
and missing results visible. A focused passing run does not establish full
coverage. An expected failure is still a conformance failure.

An implementation inventory may identify Rust-backed, JS-backed, and missing
members for planning. Derive the inventory from production binding registrations
and check installed APIs against IDL. A registration or property being present
does not establish correct behavior. Treat the inventory as implementation data,
not a second conformance score or a handwritten support checklist.

Cargo tests cover generator and browser-specific behavior. Browser-internal
visibility and automation isolation use the integration/security runners.
Web-platform assertions belong in WPT as required by `AGENTS.md`.
