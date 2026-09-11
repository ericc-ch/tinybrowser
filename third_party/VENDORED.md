# Vendored third-party data

Everything under `third_party/` belongs to someone else. Nothing here ships in
the binary; it exists so tests are reproducible and offline-capable.

## web-platform-tests

- **What**: the full WPT tree, including `resources/testharness.js` and
  the maintained html5lib parser corpus and wrappers under
  `html/syntax/parsing/`.
- **Upstream**: <https://github.com/web-platform-tests/wpt>, git submodule
  (`third_party/wpt`), full file tree at the pin below.
- **Pinned revision**: `92054a74d0c6a1ed2e9024d71ebf2880f2af02e2`
- **License**: each test's own license; see WPT `LICENSE.md`.
- **Fresh clones**: `git submodule update --init --recursive`.
- **Driver**: classic WebDriver on `tinybrowser --webdriver=PORT` over
  `BrowserHandle`, with a fresh temporary XDG profile per endpoint
  ([ADR 0008](../docs/adrs/0008-wpt-via-webdriver.md)).
- **Runner**: `./tools/wpt/run [tests]` builds the debug binary, installs `tools/wpt` into the WPT venv, skips the `/etc/hosts` check, and passes `--ssl-type none` plus `--resolve`
  ([ADR 0008](../docs/adrs/0008-wpt-via-webdriver.md)). Do not require a
  machine hosts file.
- **Parser gate**: run
  `./tools/wpt/run 'html/syntax/parsing/html5lib_*.html'`.
  The official URL, `document.write`, and single-character `document.write`
  wrappers cover full-document parsing; fragment cases run through
  `innerHTML` in the URL wrapper ([ADR 0005](../docs/adrs/0005-html5lib-tree-construction-suite.md)).
  Known tinybrowser results are baselined outside the submodule in
  `tools/wpt/metadata`.
