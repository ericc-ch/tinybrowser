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
  handwritten platform algorithms. Handwritten conversion adapters must preserve
  the spec algorithm and order.
- Share the `JsNode` payload across node interfaces. JS brands and generated
  interface prototypes provide the public interface hierarchy.

Keep host capabilities and shim backing state private. Captured operations and
private storage must prevent page-replaced methods, constructors, or inherited
hooks from observing or mutating browser internals. Rust owns native lifetimes.
RealmRegistry owns the lifetime of shared weak-slot storage.

## IDL inputs and implementation mappings

Import an unmodified, pinned upstream IDL snapshot. WPT's `interfaces/` directory
is one source. Its non-tentative files are synced from curated `@webref/idl`
extracts. Keep updates explicit and builds offline. See the
[WPT interface source policy](https://github.com/web-platform-tests/wpt/blob/master/interfaces/README.md).

Keep implementation mappings and support selection outside the imported IDL.
Mappings identify Rust backing types and methods, JS implementations, and
conversion adapters. Mappings do not redefine signatures, defaults, or standard
extended attributes. Resolve inheritance, partial interfaces, and mixin includes
from the imported declarations.

Imported declarations do not require exposing every API. Select the implemented
members separately. Keep unsupported APIs absent rather than installing dummy
methods that mislead feature detection. If an enabled binding needs unsupported
generator semantics, fail the build instead of changing the declaration or
silently omitting the binding.

The input-source migration is pending. `crates/renderer/idl/` contains
hand-maintained partial declarations and `Rust*` annotations. Generated native
bindings alone do not complete the migration to authoritative IDL inputs.

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
