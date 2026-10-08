# Vendored third-party code and data

Most of `third_party/` is test input that never ships in the binary. The
exception is the JS engine forks below, which do ship.

## Engine forks (ship in the binary)

- **What**: the maintained rquickjs and QuickJS-NG forks tinybrowser builds
  against.
- **Upstream**: <https://github.com/ericc-ch/rquickjs> at
  `third_party/rquickjs`, with the QuickJS-NG fork at
  `third_party/rquickjs/sys/quickjs` as its nested submodule.
- **Wiring**: `[patch.crates-io]` path entries in the workspace root
  `Cargo.toml`; the rquickjs tree is its own workspace excluded from the
  tinybrowser workspace.
- **Pinned revisions**: `third_party/rquickjs` at `50b403e`, nested
  `sys/quickjs` at `8feb526` (check `git submodule status`).
- **Fresh clones**: `git submodule update --init --recursive`.
- **Update**: pull upstream inside the submodule, push the fork branch there,
  then bump the submodule pointer here.

## Vendored crates (ship in the binary)

- **What**: the `weedle4` WebIDL parser tinybrowser's bindgen builds against.
- **Upstream**: vendored source at `third_party/weedle4` (see `UPSTREAM.md` there).
- **Wiring**: path dependency in `crates/webidl-bindgen/Cargo.toml`. Vendored,
  not a submodule, so correctly absent from `.gitmodules`.

- **What**: `fontconfig-parser` 0.5.8, so fontconfig XML and SVG share one
  `roxmltree`.
- **Upstream**: vendored source at `third_party/fontconfig-parser` (see
  `UPSTREAM.md` there). The only change from 0.5.8 is `roxmltree = "0.21.1"`.
- **Wiring**: `[patch.crates-io]` path entry in the workspace root
  `Cargo.toml`. Excluded from the workspace, same as `weedle4`.

## Test data (never ships)

## Web IDL extracts

- **What**: every non-tentative `interfaces/*.idl` extract from the pinned WPT
  checkout, the unchanged upstream contract the generator compiles. Also
  includes WPT's `LICENSE.md` and `manifest.json` recording the source
  repository, revision, and per-file sha256.
- **Upstream**: WPT's `interfaces/` directory at the `third_party/wpt` pin
  below; the same revision the test suite runs at.
- **Snapshot**: `crates/webidl-bindgen/idl/`, build input only, never shipped
  in the binary. A frozen copy keeps `cargo build` working without the WPT
  submodule checked out.
- **Pull**: bump `third_party/wpt`, then run `tools/webidl/import` and commit
  both; the manifest diff shows exactly which interfaces changed.
  `tools/check` runs `tools/webidl/import --check`, which fails if the snapshot
  differs from the pin or any file was edited.
- **Rule**: implementation follows these extracts; never edit them to match
  code (`AGENTS.md`).

## web-platform-tests

- **What**: the full WPT tree, including `resources/testharness.js` and
  the maintained html5lib parser corpus and wrappers under
  `html/syntax/parsing/`.
- **Upstream**: <https://github.com/web-platform-tests/wpt>, git submodule
  (`third_party/wpt`), full file tree at the pin below.
- **Pinned revision**: `92054a74d0c6a1ed2e9024d71ebf2880f2af02e2`
- **License**: each test's own license; see WPT `LICENSE.md`.
- **Fresh clones**: `git submodule update --init --recursive`.
- **Driver**: classic WebDriver on `tinybrowser webdriver --port=PORT` over
  `BrowserHandle`, with a fresh temporary XDG profile per endpoint.
- **Runner**: `./tools/wpt/run [tests]` builds the debug binary, installs `tools/wpt` into the WPT venv, skips the `/etc/hosts` check, and passes `--ssl-type=openssl` plus `--resolve`. Do not require a machine hosts file.
- **Parser gate**: run
  `./tools/wpt/run 'html/syntax/parsing/html5lib_*.html'`.
  The official URL, `document.write`, and single-character `document.write`
  wrappers cover full-document parsing; fragment cases run through
  `innerHTML` in the URL wrapper. Known tinybrowser results are baselined
  outside the submodule in `tools/wpt/metadata`.

## Blink inspector-protocol tests

- **What**: Chromium's complete `inspector-protocol` and HTTP
  `inspector-protocol` web-test trees, including expected text output and local
  resources.
- **Upstream**: <https://chromium.googlesource.com/chromium/src/>; direct
  snapshots from the two Gitiles archive endpoints recorded in
  `third_party/blink-cdp/README.md`.
- **Pinned revision**: `578830dbc33cea008a78bc6ff9825f85a83343a7`.
- **License**: Chromium's BSD-style license in `third_party/blink-cdp/LICENSE`.
- **Size**: about 20 MB across 3,609 files; test input only, never compiled
  into the binary.
- **Update**: change `third_party/blink-cdp/REVISION`, update the pin recorded
  here, then run `./tools/cdp-tests/update`. The updater stages both archives and
  refuses to swap in a tree that is empty or contains anything but regular
  files and directories.
- **Runner**: `./tools/cdp-tests/run` for promoted passing tests;
  `./tools/cdp-tests/run --all` for the complete exploratory scoreboard.
