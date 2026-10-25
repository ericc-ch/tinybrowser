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

### referrer-policy/ (non-worker), 2026-10-24

Chrome 157 (WPT 9f868fd7) passes 892 of the 899 files that run here once
worker variants and their generated directories are set aside; tinybrowser
passes 872. Each remaining Chrome-passing file is blocked on one engine
feature, not on referrer plumbing:

- 8 `css-integration/image/`: Blitz does not fetch CSS-referenced images
  (`content: url()`, the `background` content attribute), so the request the
  test reports never happens.
- 2 SVG (`external-stylesheet`, `processing-instruction`): same gap for SVG
  paint servers (`fill`, `mask`, ...), plus `xml-stylesheet` PIs.
- 2 `css-integration/{font-face,child-css}/processing-instruction.html`:
  `xml-stylesheet` processing instructions do not load stylesheets.
- `generic/sandboxed-iframe-with-opaque-origin.html`: `iframe sandbox` is not
  implemented.
- `generic/iframe-upgrade-request-to-cross-origin.sub.html`: CSP
  `upgrade-insecure-requests` is not implemented.
- `generic/inheritance/{iframe-inheritance-history-about-blank,
  iframe-inheritance-javascript-child,popup-inheritance-about-blank}.html`:
  named-window navigation/history, `javascript:` child frames, and
  cross-window DOM access.
- `generic/meta-referrer-domparser.html`: `Document.parseHTMLUnsafe` is absent
  from the WebIDL generator (static operations).
- `generic/subresource-test/worker-messaging.html`: dedicated workers
  (out of the non-worker target).

`css-integration/svg/` is the one harness-policy exception: each file sleeps
800 ms per subtest (7.2 s total) before checking, above the 3 s default, so
those files get a 6 s budget (8 s with grace, still under testharness.js's
10 s clock). See `tools/wpt/README.md`.
