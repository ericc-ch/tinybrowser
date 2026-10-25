# Handoff: tinybrowser review follow-ups

Date: 2026-10-25
Repo: `/home/erickc/projects/tinybrowser`
Main through `42f87271` (`Merge pull request #142 from ericc-ch/fix/reviewboo-remaining`)

## Current state

- PR #142 is merged: https://github.com/ericc-ch/tinybrowser/pull/142
- Merged work: #138 items 3, 7, and 10, plus the review round on #142
  (typed Runtime entry, corrected `DOM.getOuterHTML`/`getAttributes` with tests,
  typed `InstallError`, one-shot install, injected-clock tests, branded ids).
- #138 items 4 and 6 were already fixed on main before #142.
- Closed as fixed after verification: #137 and #139.
- Branch and worktree cleanup is complete: `main` only, one worktree, no
  stashes, no open PRs, clean tree. The local and remote
  `fix/reviewboo-remaining` branches are deleted.

## Not fixed

### 1. Typed CDP DOM contract (review item 2 on #142)

The DOM domain still runs JavaScript string templates in the page realm:
`REGISTER_PRELUDE`, `__tb_dom_nodes`, and `dom_eval` in
`crates/cdp/src/dispatch.rs`. `getOuterHTML`/`getAttributes` are now correct,
but still template-based. Review comment:
https://github.com/ericc-ch/tinybrowser/pull/142#discussion_r4370524377

The recommended shape is a typed DOM request keyed by the renderer tree's
`NodeId`, answered in Rust, with the CDP node-id map owned by the cdp session.
That deletes every template, `REGISTER_PRELUDE`, and `__tb_dom_nodes`. It needs
renderer protocol commands, engine-side DOM query methods, and a cdp `NodeId`
map, so it is a feature rather than a localized fix.

### 2. #136 outstanding items

Issue: https://github.com/ericc-ch/tinybrowser/issues/136

- **Item 1 (lint):** `allow_attributes = "deny"` is still off because
  `webidl-bindgen` emits `#[allow(dead_code)]` for dictionaries, enums, unions,
  and empty contract traits. It needs usage-tracked emission. The experiment
  was already run: removing the six generated `#[allow]`s produces 56
  dead-code errors (empty contract traits, unread dictionary fields, unused
  enum `as_str`/`from_value`, unused union variants).
- **Item 2:** `OffscreenCanvas.__tbCanvas` is still a page-visible back door and
  is still trusted by name at `crates/renderer/src/js/canvas/mod.rs:60`; it is
  published at `crates/renderer/src/js/scripts/web/offscreen_canvas.js:87`.
- **Item 3:** canvas serialization is still duplicated:
  `canvas_host.js:13`, `offscreen_canvas.js:172`,
  `crates/renderer/src/js/canvas/mod.rs:206`, and the JPEG quality copies at
  `crates/renderer/src/js/canvas/state.rs:2562` and `:2607`.
- **Item 4:** `document.fonts` is still a plain object; `check()` returns
  `true` unconditionally and `load()` splits on comma
  (`crates/renderer/src/js/scripts/web/fonts.js:207`).
- **Item 5:** `docs/handoff-canvas-followups.md` still exists and still points
  at `/tmp/opencode` reports and agent-session instructions.

### 3. #138 outstanding items

Issue: https://github.com/ericc-ch/tinybrowser/issues/138

Fixed on main: items 3, 4, 6, 7, 10. Still open:

- **Item 1:** cdp keeps three target representations. `Session` is flattened to
  `(TargetKind, Option<TabHandle>)` and the route table uses `kinds: u8`
  (`crates/cdp/src/lib.rs`, `crates/cdp/src/dispatch.rs`).
- **Item 2:** 45 cdp handlers parse `params: &Value` by hand with silent
  defaults; there is no typed params struct per method.
- **Item 5:** history traversal still round-trips
  `EntryId -> index -> i32 delta -> index`; `target()` returns a five-tuple and
  `HistorySnapshot::len`/`is_empty` still exist (`crates/browser/src/history.rs`,
  `crates/cdp/src/lib.rs`).
- **Item 8:** config parsing stops at `--proxy`; `--resolve`, `--tls-ca`, and
  the user agent stay raw until `Agent::new` (`src/cli.rs`,
  `crates/net/src/client.rs`).
- **Item 9:** `DialRequest.kind` and `DialRequest.cors` can disagree, and
  `apply_cors` maps `None` to `navigate` + `include`
  (`crates/renderer/src/protocol.rs`, `crates/browser/src/network.rs`).

## Suggested skills

- Use `code-conventions` for any Rust change; read `references/rust.md`. The
  workspace forbids `#[allow]`, unchecked casts, and wildcard matches on closed
  enums.
- Use `improve-codebase` for the typed DOM contract and #138 items 1/2/5; these
  are architectural refactors.
- Use `code-review` before opening the next PR.

## Environment and checks

- Run cargo through the dev shell: `nix develop --command cargo ...`.
  Plain `cargo` fails on the pinned nightly and on missing pkg-config/OpenSSL.
- Repo gate: `tools/check clippy` (fmt, workspace clippy, embedded JS,
  realm-free layer, rustdoc, Python). It passes on main at `42f87271`.
- Targeted tests used in the last session: `cargo test -p logging`,
  `cargo test -p renderer --lib`, `cargo test -p cdp --lib`,
  `cargo test --test browser cdp::`.
- Follow `AGENTS.md`: cite specs at implementation sites, never silence a lint,
  run the smallest test slice, and treat a WPT timeout as a failure.
