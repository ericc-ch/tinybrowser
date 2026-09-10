# Handoff (2026-09-10)

State: `main` at `0cab2f8`, with the autonomous-page architecture refactor in
the working tree. The implementation, tests, ADRs, and size ledger agree.

Done:

- Parser-blocking scripts now run against the partial document and can mutate
  parser state; parser-time `document.write` is fed back into tokenization.
- Navigation decodes bytes using BOM, HTTP charset, meta prescan, then the HTML
  Windows-1252 fallback.
- The product boundary is `Browser` / `BrowserHandle` / `PageHandle`. Page actors
  advance themselves, support non-monopolizing waits, and publish typed events.
- CDP emits `Page.loadEventFired`; protocol navigation remains asynchronous and
  the CLI explicitly waits for that event.
- QuickJS has memory, stack, deadline, and shutdown interruption limits.
- Blocking network work runs on a browser-owned fixed 16-worker executor with a
  bounded 256-job queue and cancellation checks.
- Profiles have one-writer locking, corrupt-cookie quarantine, durable atomic
  writes, and explicit close errors.
- Public DOM objects use their correct WebIDL prototype families; `NodeList` and
  `HTMLCollection` results are live.

Proof:

- `cargo clippy --workspace --all-targets --offline -- -D warnings`
- `cargo test --workspace --all-targets --offline`: 67 active passed, one ignored
  maintenance helper
- stripped x86_64 release: `tinybrowser` 3,613,712 bytes; `page_probe` 3,325,968
  bytes

Known platform-completeness work, not architectural blockers: full async/defer/
module script semantics, the complete `document.write` overload and exceptional
cases, and broader WebIDL/WPT surface coverage.
