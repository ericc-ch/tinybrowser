## Binary size

11,492,008 bytes (2026-10-18)

## WPT

CI 2026-10-23 run 39980734258 (wpt.yml, 20 chunks), --exclude=worker
testharness+crashtest+reftest+test262

pass 66803
expfail 29
unexp 43362
timeout 5325
crash 1
error 3404
skip 78
tests 119002

per type (pass / tests)
testharness 8344 / 34819 = 24.0%
crashtest 1885 / 2031 = 92.8%
reftest 13448 / 28492 = 47.2%
test262 43126 / 53660 = 80.4%

Total 56.2% = pass / (tests - skip)

### referrer-policy/ (non-worker), 2026-10-25

Chrome 157 (WPT 9f868fd7) passes 892 of the 899 files that run here once
worker variants and their generated directories are set aside; tinybrowser
passes 887. Every remaining non-worker file Chrome passes needs a feature this
engine does not have:

- 2 SVG (`css-integration/svg/{external-stylesheet,processing-instruction}`):
  paint servers (`fill: url()`, `stroke: url()`) are not fetched, and this
  Servo build does not generate `filter: url()`, `marker-*`, or a computed
  `clip-path: url()`, so 7 of the 9 paint properties in each file cannot reach
  a request. The other four `css-integration/svg/` files fail the same seven
  subtests and are Chrome failures too. This is the ceiling: 891 of the 899
  files is the most this engine can pass without a forked style engine.
- `generic/inheritance/{iframe-inheritance-history-about-blank,
  popup-inheritance-about-blank}.html`: `window.open()` needs a second
  top-level browsing context whose `document`, `opener`, and storage are
  scriptable from the opener, and the history file needs
  `window.open(url, "named-iframe")` targeting with a child `history.back()`.
  A renderer hosts exactly one top-level traversable today.
- `generic/subresource-test/worker-messaging.html`: dedicated workers
  (out of the non-worker target).

The rest of the directory's Chrome failures (`iframe-inheritance-history-
about-srcdoc.html`, `popup-inheritance-form-submission.html`, `workers.html`,
the four SVG files above) fail here too, so they cost nothing against the
Chrome score. `iframe-upgrade-request-to-cross-origin` passes here and fails
in Chrome.

`css-integration/svg/` is the one harness-policy exception: each file sleeps
800 ms per subtest (7.2 s total) before checking, above the 3 s default, so
those files get a 6 s budget (8 s with grace, still under testharness.js's
10 s clock). See `tools/wpt/README.md`.
