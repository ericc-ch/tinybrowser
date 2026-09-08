# Vendored third-party data

Everything under `third_party/` belongs to someone else. Nothing here ships in
the binary; it exists so tests are reproducible and offline-capable.

## html5lib-tests

- **What**: the canonical HTML parser conformance suite (`tree-construction/`,
  `tokenizer/`), consumed by `crates/browser/tests/html5lib.rs`.
- **Upstream**: <https://github.com/html5lib/html5lib-tests>, git submodule.
- **Pinned revision**: `9329e64694e7835d0dcff9811e22856ef6ad16f9`
  (2026-06-20, "Add test for AAA step 4.3").
- **Why this pin**: the very next upstream commit (`224991e`, June 2026)
  deletes `tree-construction/`; the tests moved into web-platform-tests. This
  is the final revision carrying the suite, so master cannot be tracked.
- **License**: MIT; see `LICENSE` inside the submodule.
- **Fresh clones** need `git submodule update --init` before
  `cargo test`; the harness fails loudly with that instruction otherwise.
- **Updating**: move the submodule pin, rerun the harness, and apply the
  fix-or-document rule from `wiki/researches/testing.md` to every new divergence.
- **Successor**: upstream maintenance moved to web-platform-tests:
  `wpt/html/syntax/parsing/resources/*.dat`, same format, README included.
  This pin is frozen and receives nothing new; the plan is to repoint the
  harness at WPT when the js layer lands (ADR 0005, "Upstream
  consolidation"). Until then this pin stays authoritative.

## web-platform-tests

- **What**: the full WPT tree, including `resources/testharness.js` and
  `html/syntax/parsing/resources/*.dat`.
- **Upstream**: <https://github.com/web-platform-tests/wpt>, git submodule
  (`third_party/wpt`), shallow clone of the full file tree (not a sparse
  checkout).
- **Pinned revision**: `92054a74d0c6a1ed2e9024d71ebf2880f2af02e2`
- **License**: each test's own license; see WPT `LICENSE.md`.
- **Fresh clones**: `git submodule update --init --recursive`.
- **Driver**: classic WebDriver in-process on `tinybrowser --webdriver=PORT`
  ([ADR 0008](../docs/adrs/0008-wpt-via-webdriver.md)).
- **Runner**: `pip install -e tools/wpt` into the WPT venv (or `tools/wpt/run`),
  then `./wpt run --binary /path/to/tinybrowser tinybrowser [tests]`.
  Install hosts once with `./wpt make-hosts-file` appended to `/etc/hosts`.


### Known exclusions

None for tree-construction: fragment cases run through `parse_html_fragment`.
html5ever's unimplemented `selectedcontent` option-clone (`webkit02.dat` #44–47)
and select-fragment `<input><option>` (`tests_innerHTML_1.dat` #75) are listed in
`KNOWN_UPSTREAM_DIVERGENCES` with pinned dumps under
`crates/browser/tests/html5lib/accepted/` (see ADR 0005).
