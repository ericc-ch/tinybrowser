# Handoff (2026-09-06)

## State

`main` is at `d743e23` (`Trim test suite to high-signal gates`). The trim is committed; post-trim workspace verification is not recorded yet.

## Do next

1. Initialize `third_party/html5lib-tests`, then run the Nix-based workspace test, fmt, and clippy checks.
2. Finish the remaining test trim: `send_loopback.rs` still duplicates method-grammar coverage from `token_grammar.rs`; keep one wire-level custom-method case. Remove shallow one-use test helpers where inlining is clearer.
3. Continue the page-engine milestone: execute loaded `<script>` elements and add the first real DOM bindings to JS.

Keep the compact public-boundary gates. If verification fails, repair those gates instead of restoring deleted one-off tests.

## Remaining product work

- CLI `serve` and `fetch` commands are still stubs.
- Loaded HTML does not execute script elements.
- JS host is minimal: no DOM bindings beyond `document.cookie`, full Fetch/CORS, timer cancellation, or script execution budget.
- WebIDL verification is designed but not implemented; no vendored webref snapshot, `weedle` harness, manifest diff, or CI gate exists yet.
- Transport still has known limits around HTTP forward-proxy request form and deadline-bounded system DNS.
- Current size probe is a page-engine checkpoint, not a finished browser-size claim.

## Deferred milestones

- Full WPT harness once the JS/DOM surface is large enough.
- Canonical Chrome-like h1/h2 + TLS transport on `btls`.
- CDP crate/server and later profile persistence.

## Test facts

- html5lib expected run count: 3,549.
- Accepted upstream divergences: 10.
- Keep corpus/oracle tests and deterministic loopback transcripts; trim duplicated behavior matrices.
