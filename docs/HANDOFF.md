# Handoff (2026-09-12)

State: `main` is clean and synchronized with `origin/main` at `44f957f` (PR #6
merged). No open PRs, nothing in flight.

Done:

- PR #6 "Harden the renderer boundary" merged as `44f957f`. Branch head
  `b3094be` merged `main` (`32cd01d`) and carried the CodeRabbit hardening set
  plus an ADR renumber. Proof: `git diff --check`, `cargo fmt --check`, strict
  Clippy, and full `nix develop --command cargo test --workspace --all-targets`
  all green in `/tmp/tinybrowser-pr6-fix.e0VUcd` before merge.
- CodeRabbit PR #6 findings were reviewed item by item and kept on merit:
  waiter/subscriber caps, fail-closed service replies, bounded renderer
  inbox/outbox, writer-failure shutdown, size-limited IPC encoding, and the
  docs registry-to-factory wording. The branch ADR was renamed
  `0016-renderer-seam-reference-monitor.md` because `0015-logging.md` landed in
  main first.
- PR #5 review fixes (`61adf4d`, `fc29a4a`) already in `main` were re-checked;
  they are real fixes (rotation state on failure, 0600/0700 private mode,
  line-break escaping), so they stay.

Next:

- Optional cleanup: remove the `/tmp/tinybrowser-pr6-fix.e0VUcd` and
  `~/.local/share/opencode/worktree/174cc2/nimble-cactus` worktrees, and delete
  the local `backup/pr6-coderabbit-findings` tag (pre-amend snapshot of the PR
  #6 merge).
- The next feature work is in ADR 0016's scope limits: browser-owned frame
  routing / OOPIF before cross-site iframe navigation ships, and the renderer
  sandbox. ADR 0014's frame tree steps are still open.

Decisions made:

- CodeRabbit is advisory. Apply findings on merit; do not chase the bot. The
  repo has no branch protection, so a CodeRabbit `CHANGES_REQUESTED` never
  blocks a merge.
- When a later merge finds its ADR number taken in `main`, renumber the
  unmerged ADR (0015 logging won; renderer seam became 0016).

Gotchas:

- Run Cargo commands inside `nix develop`; direct Cargo lacks OpenSSL/pkg-config.
- `backup/pr6-coderabbit-findings` is a local-only tag; delete it only after
  confirming nothing needs the pre-amend merge snapshot.
