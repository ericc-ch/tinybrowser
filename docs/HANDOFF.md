# Handoff (2026-09-08)

State: `main` is `16c604cc5d2afa1f8f4c52fc3ceeacd90f9714ba` (`Finish test suite trim`). JS/WPT worktree `js-wpt-a3f8c2d1` was aborted and removed; nothing from it was merged. Local `wiki/` → `docs/` moves may still be uncommitted on this tree.

Done:

- Test-suite trim on `main`: `16c604cc5d2afa1f8f4c52fc3ceeacd90f9714ba` (commit on origin-tracking `main`; not re-run in this abort).

In flight:

- None. Discarded worktree had uncommitted TreeSink extract, `Rc<RefCell<Dom>>`, prelude wrappers, and a testharness boot that stubbed Window in the test instead of implementing it in the engine.

Next:

1. Split html5ever TreeSink out of `crates/browser/src/lib.rs` (mechanical; html5lib must stay green).
2. Put the live `Dom` on `Page` in `Rc<RefCell<Dom>>` so host ops can touch the tree during `eval` (same reason cookies already use a cell).
3. Grow the **engine** JS host: Rust owns the tree/net/timers; self-hosted prelude owns `window` / `Document` / `Node` / `Element` names. One `Element` type, not a rquickjs class per tag. Implement the Window/DOM APIs testharness needs **in that host**, not as test fakes.
4. First gate: `Page::eval` WPT `testharness.js` + `idlharness.js`, inject `interfaces/dom.idl` as text (no WPT HTTP server, no full WPT checkout). Commit pass/fail snapshot. Almost all FAIL is success until rows go green (`getElementById`, then classic `<script>`).
5. Do not generate IDL tests in Rust, do not codegen rquickjs classes, do not inject jsdom.

Decisions made:

- Oracle is WPT DOM idlharness, not a homegrown generator ([idlharness](https://web-platform-tests.org/writing-tests/idlharness.html)).
- rquickjs stays `std` only (`docs/adrs/0007-engine-charter.md`).
- Fake Window in the test wrapper is the wrong seam; testharness must see the page engine.

Gotchas:

- `JsHost` cannot take `&mut Page` from QuickJS callbacks; the tree must be a side cell like the cookie jar.
- `Page::eval` today does not run HTML `<script>` nodes; that is after the first DOM surface, not a second WPT.
- Work in a git worktree if implementing again; do not mix with the uncommitted docs path move.
