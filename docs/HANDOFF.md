# Handoff (2026-09-16)

State: Draft PR https://github.com/ericc-ch/tinybrowser/pull/16 on `chase/dom-events` (`4084361`). Clippy, `cargo test --workspace`, and stripped size were green at land. The full WPT suite was killed; `/tmp/wpt-suite/report.json` is empty. No repo-local doctor skill.

Done:

- WPT harness: unexpected PASS in `tools/wpt/score.py`; WebDriver names hidden from Window (`f104e09`, merged via PR #14). Proof: that commit’s tests/`score.py --selftest`.
- Events chase + honest refuse of copy-then-destroy adopt (`8368788`). Proof in PR #16: clippy `-D warnings` clean; `cargo test --workspace` pass; `python3 tools/wpt/launch.py --selftest` pass; `git diff --check` clean.
- Progress size `5,792,032` (`4084361`). Proof: `stat` on `target/release/tinybrowser` is `5792032`.

In flight:

- Full suite stopped after ~64m (`--exclude=worker --processes 8`, debug binary). About 573 of 36069 tests visible (141 TIMEOUTs), still in FileAPI/IndexedDB/WebCryptoAPI/CSP. `/tmp/wpt-suite/report.json` empty. Resume is not “restart the suite”; see Next.
- Focused `dom/events/` score was not re-run after `4084361` (venv lock). `/tmp/wpt-events-2.json` is 194 files, still 2 ERROR / 106 TIMEOUT (pre-ERROR-fix). Close with `nix develop --command ./tools/wpt/score dom/events/ -- --exclude=worker`.
- PR #16 still draft. CodeRabbit not run. Not merged.
- Spec adopt (same node, retargetable `NodeId`) not implemented.

Next:

1. Re-score `dom/events/` `--exclude=worker` and put the numbers in `docs/progress.md`.
2. Score other groups that can finish (html, rest of `dom`, url, cookies). Do not pathless-run the suite; css/actions TIMEOUT wait the clock.
3. Land PR #16 only on explicit go (`gh pr ready`, CodeRabbit, merge commit). Follow `.agents/skills/land-pr/SKILL.md`.

Decisions made:

- Events first; named path; `--exclude=worker` omits variants (not “fail honestly”).
- Cross-document insert throws until a handle can retarget ([concept-node-adopt](https://dom.spec.whatwg.org/#concept-node-adopt)).
- Tree/Event algorithms: Rust binding. Abort flags / Web IDL sugar: JS. One `JsNode`. Experiments in `/tmp/`.
- `docs/progress.md` is latest-only; unscored test groups are intentional.

Gotchas:

- One WPT run at a time (shared venv lock). `--exclude=worker` shrinks the score denominator.
- `git diff main...HEAD` was empty while work was uncommitted; look at the PR commits.
- Primary checkout is on `chase/dom-events`; do not move `main` there. No cargo tests for web-platform behavior.
