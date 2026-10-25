# Handoff: referrer-policy review follow-ups

## State

- PR #141 (`dev`) is merged into `main`: `d31e07d7`. No branch is open.
- Fix commits behind it: `42a5fc84` (contained review findings), `458104ca` (drain reported idle with adopted work), `e7cfada5` (progress note).
- Non-worker `referrer-policy/`: **890 strict of 897 files**, unchanged from the pre-merge baseline. The seven failures are the six `css-integration/svg/` paint-server files (no `filter: url()`, `marker-*`, or computed `clip-path: url()` in this style engine) and the pre-existing `popup-inheritance-form-submission.html` subtest. Chrome 157 passes 892 of its 899 for the same reason.
- Verified: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets`, `cargo test --workspace`, the full run above, and a release smoke (`--binary .../release/tinybrowser referrer-policy/generic/inheritance/`).

Everything the review called a safety or correctness bug is closed. What remains is the architectural half, listed below; item numbers are the review's, posted as the "re-review of `ce326e4`" review on the PR.

## Follow-ups

In dependency order. Each is its own PR; mixing them makes a regression untraceable.

1. **Joint session history in the browser (7–11).** `FrameHistory` (`crates/renderer/src/engine.rs`) holds child entries while `crates/browser/src/history.rs` holds the tab's, so a child's `history.length` is its own list and parent `back()` cannot traverse it. Records at navigation start instead of commit, seeds an entry for the initial blank, and replays traversal with the parent's *current* policy. One tab-level tree keyed by frame, commits reported by the renderer, retires items 8–11. Prerequisite for cross-process iframes.
2. **Auxiliary contexts are real browser tabs (12).** `open_auxiliary` (`crates/renderer/src/js/world.rs`) hosts blank/same-origin popups as hidden frames: not laid out, no webdriver handle, die with the opener, two `close()` paths. Needs item 1's commit reporting plus site-keyed process routing.
3. **Per-element response tainting and node ids in Blitz requests (68, 69, 70).** `image_origin_clean` (`crates/renderer/src/js/canvas/mod.rs`) keys origin-cleanliness by URL, so a same-origin redirect to a cross-origin no-CORS response can expose pixels. `subresource_target` (`crates/renderer/src/document/mod.rs`) finds the element by walking the tree for a matching URL, which also decides referrer policy and CORS mode, and `subresource_bytes` ignores mode entirely. Carry node id and tainting in the request.
4. **Blitz invalidation API (17, 65).** The drain settles style and layout whenever the shell asks for a redraw, per task. Needs a fork `needs_resolve()`-style dirty flag and one settle per rendering opportunity. Measure the win before landing.
5. **Realm creation inside a `Ctx` (13, 64).** Inline script insertion across realms is deferred because rquickjs refuses `Context::full` under a live `Ctx`; QuickJS itself allows it. Fork patch plus upstream PR, then defer only for a realm-less owner.
6. **Style-sheet referrer token (16).** `remember_sheet`/`referrer_authorized` (`crates/browser/src/network.rs`) accept any URL the renderer fetched "as a sheet", which is the trust this was meant to remove. Either mint a browser-side token per document and response, or delete the set and say so in the threat model. Decide before coding.
7. **Internal-test debt (44, 86, 33).** Everything the review found wrong was WPT-only: sandbox inheritance, refresh overflow, the sheet allowlist, per-hop `Referer`. `tests/renderer_process.rs` servers also accept one connection with no timeout, so a renderer that never connects hangs CI. Cheapest item; can start immediately.

Sandbox flags not gated yet (plugins, pointer lock, modals, downloads, orientation, presentation) are modelled in `SandboxFlags` but have no feature to gate; add the check with the feature, not before.

## Recorded decisions

- `Document.parseHTML` stays absent until a `Sanitizer` exists.
- `location.hash` stays read-only until fragment navigation exists; a full reload is the wrong answer.
- Child and auxiliary history in the renderer is a known limitation against the spec, removed by item 1.
- Fork changes need a spec or crash reason in `docs/upstream.md`; compile and size tricks stay upstream.
- A WPT timeout is a failure. The generated-suite timeouts were a drain bug parked by the budget, not a slow path — see `docs/progress.md`.

## Next move

Start with item 7 (or item 1 if the next session is a refactor). Branch from `main`. Each item gets a before/after WPT parity run on `referrer-policy/` plus the suite it touches, and a `--binary` smoke for any release change.

## Suggested skills

- Use `improve-codebase` for items 1 and 2; both are ownership moves, not feature work.
- Use `grilling` to settle item 6's trust decision before writing code.
- Use `research` for the spec and engine source lookups (WHATWG, Chromium, Servo) that each item cites.
- Use `code-review` before every follow-up PR.
- Use `technical-writing` for edits here, `docs/priorities.md`, or `docs/upstream.md`.

## References

- PR #141: https://github.com/ericc-ch/tinybrowser/pull/141
- Review (90 items, item numbers above): the "re-review of `ce326e4`" review on that PR
- Score record and the drain fix: `docs/progress.md`
- Fork gaps and their reasons: `docs/upstream.md`
- Existing priorities: `docs/priorities.md`
- Engine source lookups: `docs/engines.md`; fork and submodule workflow: `third_party/README.md`
- WPT harness policy: `tools/wpt/README.md`
