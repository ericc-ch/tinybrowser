# Handoff (2026-09-06)

State: Test-suite trim is implemented on `main` and remains uncommitted.
The worktree contains the compact public-boundary gates and deleted redundant
fixtures; verification was intentionally skipped at the pause point.

Done:

- Hardened the html5lib gate with pinned execution/divergence counts and folded
  the three selectedcontent regressions into it.
- Replaced the DOM mutation storm with an independent model and reduced the
  DOM, selector, page, cookie, HTTP, WebSocket, token, and error suites.
- Removed ignored live tests, proptest fixtures/dependency, and internal-only
  cookie/dial tests; updated `wiki/researches/testing.md` to the new budget.

In flight:

- Commit and push the current working tree on `main`; no tests or clippy run
  after the final trim, by explicit request.

Next:

1. Run the Nix-based workspace checks when work resumes.
2. If anything fails, repair the compact gate rather than restoring deleted
   one-off tests.

Decisions made:

- Keep corpus cases and independent oracles; remove duplicated test functions.
- Use deterministic loopback transcripts for page/network behavior.

Gotchas:

- `third_party/html5lib-tests` must be initialized for the parser gate.
- The html5lib expected run count is pinned at 3,549 with 10 accepted runs.
