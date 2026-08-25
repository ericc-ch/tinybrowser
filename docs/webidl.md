# WebIDL Verification

The DOM API surface we expose to page JS is verified against the WHATWG living standards mechanically, not hand-audited.

## Strategy: verify, don't generate

1. **Vendor IDL snapshots.** Check in the w3c/webref **curated** branch (`ed/idl/`) files for our target specs (dom, html, url, fetch, xhr, and friends) pinned to a commit. Curated snapshots are patched to parse cleanly and have partials/mixins conflict-resolved. Committing beats fetching at test time: reproducible and offline.
2. **Parse with `weedle`** (dev-dependency only; 0.13.x, active again since 2026-02). Covers interfaces, mixins, partials, namespaces, dictionaries, async iterables, maplike/setlike, stringifiers: everything WHATWG specs emit.
3. **Flatten and diff.** Test-time harness resolves inheritance + partials + mixins + `includes` into one flat view per interface, then diffs it against a manifest of what we actually implemented (member name, kind, arity set for overloads, readonly).
4. **CI fails on missing/extra** members unless allowlisted with a written reason. Intentional gaps (e.g. no layout-dependent APIs) get an explicit allowlist entry so drift is a decision, never an accident.

Zero binary cost: all of this is dev tooling.

## Explicitly rejected: build-time binding codegen

Generating rquickjs scaffolding from IDL at build time was evaluated and rejected:

- Generated dispatch shape fights hand-written classes; overload resolution, `[LegacyUnforgeable]`, named properties, and cross-spec partials each need bespoke handling.
- The wasm-bindgen failure mode is exactly wrong for verification: unsupported constructs are warn-and-skip, hiding drift.

## Later option (same snapshot, new consumer)

A compact reflection table (name/kind/arity/unforgeable flags) compiled from the same vendored IDL behind a cargo feature, so CDP `Runtime.getProperties` reports accurately. ~50–200 KB estimated; only build when CDP fidelity demands it.

## Prior art worth stealing from

- Gost-DOM (Go): moved from hand-written wrappers to webref-driven codegen; validates the webref-as-source-of-truth approach.
- FastRender (Rust): xtask extracts `<pre idl>` from spec sources → resolve pass → committed snapshot + allowlist-gated check mode. Copy its determinism/allowlist pattern.
- wasm-bindgen-webidl: the warn-and-skip anti-pattern (see above).

## Sources

- webref: <https://github.com/w3c/webref> (use curated `ed/`; `tr/` never contains WHATWG specs)
- weedle: <https://github.com/rustwasm/weedle>
