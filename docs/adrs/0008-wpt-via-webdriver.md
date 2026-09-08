# WPT via WebDriver

Web-visible behavior is verified by the full web-platform-tests tree, driven the way browsers drive it: `./wpt run tinybrowser` over **WebDriver**. Host objects are Rust; remaining APIs may be JS. CDP is not the test runner.

Status: accepted

wptrunner’s testharness executor navigates a real HTML document and reads results from `testharness.js` / `testharnessreport.js`. That path is WebDriver. CDP stays the later agent-facing crate ([ADR 0007](0007-engine-charter.md)); using it here would implement the wrong protocol for WPT.

The WPT pin is the **full** tree (submodule), not a sparse subset. The first green bar is testharness completing through WebDriver, not a chosen directory of tests.

WebIDL: verify against vendored IDL ([webidl.md](../researches/webidl.md)); do not codegen bindings. Interfaces with branding, tree mutation, or a host resource are Rust host objects around `NodeId` (or page-owned handles). Other APIs may be implemented in JS to keep binary size down.

Parser html5lib `.dat` tests move to WPT (`html/syntax/parsing/`) once testharness runs; the frozen `html5lib-tests` pin is then dropped ([ADR 0005](0005-html5lib-tree-construction-suite.md)). Browser-crate tests of web-visible JS/DOM are not kept in parallel. WPT covers fetch, XHR, `document.cookie`, and WebSocket **as the page sees them**. It does not import `net::Agent`; that crate’s unit tests are transport tests, not a second web suite.

## Options considered

- **CDP as the WPT driver:** matches a future agent product, not wptrunner’s testharness executor. Rejected for this gate.
- **In-process `Page` loader instead of WebDriver:** same HTML files, not the canonical runner. Rejected.
- **Sparse WPT checkout:** smaller clone; cannot run an arbitrary test when adding an API. Rejected.
- **JS polyfills for Node/Document/Event:** fails WebIDL branding and WPT. Rejected.
