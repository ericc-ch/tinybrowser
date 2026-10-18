# third_party

Submodules for living forks we rebase; vendored copies for small frozen
deltas; snapshots for test input. Only the forks and vendored crates
ship in the binary.

## Forks (submodules, ship in binary)

- rquickjs + QuickJS-NG at third_party/rquickjs (nested sys/quickjs).
  [patch.crates-io] paths; own workspace, excluded from ours.
- blitz at third_party/blitz, branch master (main tracks upstream).
  [patch.crates-io] blitz-dom, blitz-html, blitz-paint, blitz-traits.
- html5ever at third_party/html5ever, branch master.
  [patch.crates-io] xml5ever only; builds against registry markup5ever.
- Update: pull upstream in the submodule, push the fork branch, bump
  the pointer here. Check pins with git submodule status. Fresh clones:
  git submodule update --init --recursive.

## Vendored crates (ship in binary)

- weedle4: sagudev/weedle4 at 7dd96ea plus our async_sequence grammar
  patch (WebIDL async-iterable). Build-only dep via
  crates/webidl-bindgen/Cargo.toml. Update: copy new tree over,
  re-apply the grammar patch, rebuild over all pinned extracts.
  Upstream tip is our pin (2026-10-15); recent commits are bot data syncs.
- fontconfig-parser 0.5.8 with roxmltree moved 0.20.0 -> 0.21.1
  (usvg/fontique already pull 0.21; one parser linked).
  [patch.crates-io] path. Update: copy new release over, re-apply the
  one-line bump, run crate tests + workspace build. Upstream inactive
  since 2025-05-12.

## Test data (never ships)

- WPT submodule at third_party/wpt. IDL extracts snapshotted to
  crates/webidl-bindgen/idl/ via tools/webidl/import (manifest pins
  revision + sha256; tools/check enforces). Never edit extracts to
  match code. Runner: tools/wpt/cli.py; parser gate:
  html/syntax/parsing/html5lib_*.html.
- Blink inspector-protocol snapshot at third_party/blink-cdp.
  Revision in REVISION file. Update: set REVISION, run
  tools/cdp-tests/update. Runner: tools/cdp-tests/run.
