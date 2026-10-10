# Handoff (2026-10-10)

Goal: Land the PR 45 follow-ups on `main` and keep `dom/nodes` moving toward Chrome/Firefox range. `main` is at `0308a0a` (pushed, `ls-remote` verified). Everything below is on `main`; the `cursor/wpt-fast-loop-6d59` branch is superseded.

Plan: Work the remaining `dom/nodes` fail-list (see `docs/progress.md` slice line, 287/354 stable) in fork-domain order, then take the three deferred items in Next order. No suite/slice runs without approval per `AGENTS.md`; `tools/wpt/retest` is the fast loop.

State: `cargo test -p renderer --lib` 17/17 green, `cargo test -p browser --lib` 35/35 green, `cargo check -p renderer` clean. Clippy fails only on pre-existing `webidl-bindgen too_many_lines` (`contracts.rs:541`, untouched by this work). Forks pinned and pushed before pinning: blitz `41074d7` (`branch = master` in `.gitmodules`), html5ever `02ecdaa`. `VENDORED.md` pins exact SHAs.

## What a manifest is (asked 2026-10-10)

`third_party/wpt/MANIFEST.json` (~40MB) is WPT's inventory of every test: file path → content hash + metadata (test type, timeout, variants). The runner reads it instead of scanning 100k+ files per run. Verified in-tree:

- The file exists and has that shape (`items.<type>.<path>...: [sha, meta]`).
- wptrunner rewalks (rebuilds it) on **every** run by default: `tools/wptrunner/wptrunner/wptcommandline.py:594-595` sets `manifest_update = True` when unset. Our `tools/wpt/run` passes `--no-manifest-update` only when its stamp says the tree is unchanged.
- Our stamp (`manifest_token` in `tools/wpt/run`): git HEAD + `git status` + content hashes of tracked-modified and untracked files + manifest bytes, stored under `~/.cache/tinybrowser/`. Current = skip the walk.
- The 2026-10-10 slow path additionally gates stamping on mtime change + no-signal + parses-OK (`tools/wpt/run:601,633-635`).
- Correction to earlier discussion: there is **no** `./wpt manifest` subcommand at our pin (`third_party/wpt/tools/wpt/*.py` has no `manifest.py`). The deferred explicit-step follow-up must drive `load_and_update` in `third_party/wpt/tools/manifest/manifest.py:365` directly (small driver under `tools/wpt/`, run with the venv python).

Done:

- `8190ed7` — code-review fixes across forks/renderer/scripts/runner (forks pushed, `ls-remote` verified; `VENDORED.md` exact pins). Verified: `cargo test -p renderer --lib` 17/17, `cargo check --workspace` clean.
- `0308a0a` — PR 45 hostile-review (nitboo) follow-ups, coderabbit ignored. Verified: renderer lib 17/17, browser lib 35/35, `bash -n tools/wpt/run` OK, reorder-guard stub harness (typo → exit 1, swallowed path → exit 1, legit `--metadata a/b` → exit 0). SHAs and per-item reasoning are in that commit message.
- Fork branch record: `.gitmodules` pins `branch = master`; fork default `main` confirmed diverged via `ls-remote` (74fe1ab vs 41074d7) — documented in `VENDORED.md`.

Unfinished:

- **Explicit manifest step** (deferred, low risk). Resume: write a `tools/wpt/manifest.py` driver calling `load_and_update(tests_root, manifest_path, url_base, ...)` with the venv python under the manifest lock, snapshot `manifest_token` before, stamp only on exit 0, then run tests with `--no-manifest-update`. Current mtime gate covers argparse-death/SIGTERM/torn-manifest; only mid-run tree edits can over-stamp (self-heals on next tree edit; stamp deletable in `~/.cache`).
- **`createEvent` init-path proof** (deferred, needs approved WPT run). Resume: read `initEvent`/`initUIEvent` writes in `crates/renderer/src/js/events.rs` against the `__tbEventEntry` lazy slots, then run `dom/events` createEvent cases via `tools/wpt/run` (needs approval). Mechanism (lazy `fresh()` defaults + forge-proof `host.__tbIsEvent`) already in place; only proof missing. Legacy paths only; `new UIEvent()` unaffected.
- **Per-move suppression** (deferred, needs approved WPT run). Resume: `noteConnectedMove`/`collectRecords` in `crates/renderer/src/js/scripts/web/custom_elements.js:88-106,378-390`. Two synchronous `moveBefore()`s on one node before observer delivery cause spurious disconnect+connect. Fix must tag suppression per move generation without breaking `schedule_mutation_delivery`, the `connected` set, or the shadow-including walk. Gate on the `moveBefore` WPT slice (incl. iframe-crash test; note the 15 known timing flakes).
- **`dom/nodes` fail-list**: 59 unexpected + 8 error at 287/354 stable (entity-xhtml ERROR, script-count 4-vs-6, NodeList `interrupted`, surrogates/shadow/owner-docs buckets). Previous handoff detail was in the superseded branch note; re-derive from a fresh `retest` before working.

Next:

1. Manifest explicit step (self-contained, no approval needed; verify with forced stale/fresh runs).
2. `createEvent` init-path read + approved `dom/events` slice run.
3. Per-move suppression + approved `moveBefore` slice run.
4. `dom/nodes` buckets in score order.

Decisions made:

- Coderabbit comments ignored wholesale (noise); every nitboo comment verified against the tree, fixed only what held up. Rejected-with-evidence: `brands.js` fail-fast (broke 4 realm tests; silent skip is correct for script-implemented members), `decode_xml_response` String→bytes (Latin-1 projection round-trips losslessly; documented), drift-check import path (cwd is `$WPT`, correct as-is).
- `cssRules.length` throws `not implemented` instead of reporting 0 — fails loudly per the no-workaround policy; tests expecting counts fail honestly until real rules land.
- Network `Referer` check mirrors policy (downgrade-only reject) instead of overriding it; renderer stays the policy decider.
- `parsererror` reuses the parsed document (keeps base URL/providers); `xml_document` passed in; shared `PARSERERROR_NS`; XHR checks namespace, not just local name.

Gotchas:

- No host `python3` — everything Python must run via `nix develop --command` (also required for cargo: host lacks `pkg-config`/OpenSSL).
- `tools/wpt/run` slow-path signal semantics are now load-bearing (INT-as-INT, re-wait loop, fds closed for children); test runner edits with the stub harness pattern from `0308a0a`, not live runs.
- `docs/progress.md`: replace binary size, total, and scored groups/slices only.
- Scratch probes go under `third_party/wpt/` and must be deleted after use.
