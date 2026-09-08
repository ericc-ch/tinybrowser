# WPT via WebDriver

Web-visible behavior is verified by the full web-platform-tests tree, driven the way browsers drive it: `./tools/wpt/run` over **WebDriver**. Host objects are Rust; remaining APIs may be JS. CDP is not the test runner.

Status: accepted

wptrunner’s testharness executor navigates a real HTML document and reads results from `testharness.js` / `testharnessreport.js`. That path is WebDriver. CDP stays the later agent-facing crate ([ADR 0007](0007-engine-charter.md)); using it here would implement the wrong protocol for WPT.

The WPT pin is the **full** tree at SHA `92054a74d0c6a1ed2e9024d71ebf2880f2af02e2` (submodule `third_party/wpt`). The first green testharness bar is still the goal, not a claim of this landing.

## What shipped

- In-process classic WebDriver on `tinybrowser --webdriver=PORT` (the `webdriver` crate; root `tinybrowser` depends on it).
- Out-of-tree wptrunner product (`tools/wpt`) plus `./tools/wpt/run`.
- HTTP-only first bar: `--ssl-type none` (HTTPS testharness files are excluded until cert trust exists).
- Hosts: `tinybrowser` is not on WPT’s skip list; install `./wpt make-hosts-file` into `/etc/hosts`.
- Testdriver user-input tests are skipped (`supports_testdriver = False`) until click/send_keys are real. The testharness executor still uses testdriver `run()` for result collection.
- html5lib-tests stay the parser gate until testharness runs `html/syntax/parsing/`. Browser-crate JS/DOM tests stay until that green bar exists.

Invocation: `pip install -e tools/wpt` into the WPT venv, then `./wpt run --binary /path/to/tinybrowser --ssl-type none tinybrowser [tests]`, or `./tools/wpt/run [tests]`.

WebIDL: verify against vendored IDL ([webidl.md](../researches/webidl.md)); do not codegen bindings. Interfaces with branding, tree mutation, or a host resource are Rust host objects around `NodeId` (or page-owned handles). Other APIs may be implemented in JS to keep binary size down.

## Options considered

- **CDP as the WPT driver:** matches a future agent product, not wptrunner’s testharness executor. Rejected for this gate.
- **In-process `Page` loader instead of WebDriver:** same HTML files, not the canonical runner. Rejected.
- **Sparse WPT checkout:** smaller clone; cannot run an arbitrary test when adding an API. Rejected.
- **JS polyfills for Node/Document/Event:** fails WebIDL branding and WPT. Rejected.
