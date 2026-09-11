# HTML parser conformance through WPT

Hand-written parse fixtures miss foster parenting, the adoption agency,
templates, foreign content, and parser/script interaction. Browser-visible
parser conformance therefore runs through the official web-platform-tests
html5lib wrappers, not through Cargo.

Status: accepted and implemented

Run the parser gate with:

```sh
./tools/wpt/run 'html/syntax/parsing/html5lib_*.html'
```

The WPT pin carries the maintained `.dat` corpus in
`html/syntax/parsing/resources/` and three testharness wrappers:

- `html5lib_url.html` navigates an iframe to a Blob URL. It also owns every
  `#document-fragment` case, which parses through the context element's
  `innerHTML` setter.
- `html5lib_write.html` parses full documents through `document.write()`.
- `html5lib_write_single.html` repeats that path one input character per
  `document.write()` call.

At migration on WPT revision `92054a74d0c6a1ed2e9024d71ebf2880f2af02e2`,
all 173 wrapper variants complete under the expected-result metadata. The URL
mode ran 1,924 subtests; each write mode ran 1,725, for 5,374 total. Of those,
5,086 pass and 288 known parser divergences are recorded as tinybrowser-specific
expectations under `tools/wpt/metadata`. The gate is green at that baseline and
turns red for a new failure or an unexpected pass; no renderer-stop or timeout
is accepted as a baseline result.

The former `third_party/html5lib-tests` submodule and
`crates/renderer/tests/html5lib.rs` duplicated browser conformance in Cargo.
They were removed once the WPT URL path reproduced the old
`tests_innerHTML_1.dat` select-fragment divergence and the document wrappers
completed the full maintained corpus. The old suite's scripting-off mode was
an internal parser toggle, not observable behavior in a script-running WPT
realm; any disabled-scripting product mode must be tested through its browser
surface rather than by restoring a Cargo conformance harness.

## Options considered

- **Keep the frozen html5lib submodule beside WPT:** rejected. It stopped
  receiving upstream cases when the suite moved into WPT and answers the same
  web-platform question through a non-browser API.
- **Run only one WPT wrapper:** rejected. URL navigation, bulk
  `document.write`, incremental `document.write`, and fragment parsing exercise
  distinct browser paths.
- **Patch vendored WPT:** rejected. Runner prerequisites belong in the browser;
  upstream test data and wrapper behavior remain untouched.

## Consequences

Spec parser changes require a result from the governing html5lib WPT variant.
Cargo tests may cover tinybrowser-owned parser lifecycle or resource-budget
invariants, but must not duplicate `.dat` expectations or pin alternative DOM
trees.
