# WPT via WebDriver

Web-visible behavior is verified by the full web-platform-tests tree, driven the way browsers drive it: `./tools/wpt/run` over **WebDriver**. Host objects are Rust; remaining APIs may be JS. CDP is not the test runner.

Status: accepted for WPT-via-WebDriver. The WebDriver adapter and “does not own Browser” law live in [ADR 0009](0009-named-profile-daemon.md). Temporary-profile isolation lives in `tools/wpt/tinybrowser_wpt.py`.

wptrunner’s testharness executor navigates a real HTML document and reads results from `testharness.js` / `testharnessreport.js`. That path is WebDriver. CDP is the CLI and agent control plane ([ADR 0009](0009-named-profile-daemon.md)). Using CDP as the WPT driver would implement the wrong protocol for WPT.

The WPT pin is the **full** tree at SHA `92054a74d0c6a1ed2e9024d71ebf2880f2af02e2` (submodule `third_party/wpt`). The first green testharness bar is still the goal, not a claim of this landing.

## Isolation

WebDriver is a peer adapter over `BrowserHandle` ([ADR 0009](0009-named-profile-daemon.md)). `--webdriver=PORT` remains the WPT host.

The runner gives each WebDriver endpoint a fresh temporary profile. `TinyBrowser` sets `XDG_RUNTIME_DIR` and `XDG_DATA_HOME` to a new temp tree before launch, and deletes that tree on `stop` / `cleanup`. It must not select a dirty persistent profile.

Product `DELETE /session` detaches automation and preserves persistent tabs and data. Explicit close-window closes the selected tab, including the last tab, with spec-correct session behavior. Those product rules are not WPT isolation.

The exact launch flag may stay `--webdriver=PORT` or change. That flag is open. The isolation grain is not.

## What shipped

- Classic WebDriver on `tinybrowser --webdriver=PORT` as a `BrowserHandle` adapter (the `webdriver` crate; root `tinybrowser` depends on it). This is the WPT host ([ADR 0009](0009-named-profile-daemon.md)).
- Out-of-tree wptrunner product (`tools/wpt`) plus `./tools/wpt/run`. Each endpoint gets a fresh temporary XDG profile.
- HTTP-only first bar: `--ssl-type none` (HTTPS testharness files are excluded until cert trust exists). `./tools/wpt/run` also drops extra listen ports (`https-*`, `http-local`, `http-public`, `ws`, `dns`, …) so the runner does not bind extra loopbacks or start a DNS server.
- Hosts: `./tools/wpt/run` skips WPT’s `/etc/hosts` check and launches `tinybrowser --webdriver=PORT --resolve=*.test=127.0.0.1` (plus `nonexistent.*.test=fail` and `*.test.`). No machine hosts file. Do not patch vendored WPT.
- `./tools/wpt/run` passes `--no-pause-after-test` (wptrunner otherwise pauses after a single file, and testharness `output: 1` never finishes on our DOM) and `--no-restart-on-unexpected`.
- Testdriver user-input tests are skipped (`supports_testdriver = False`) until click/send_keys are real. The testharness executor still uses testdriver `run()` for result collection. Click and actions endpoints return `unsupported operation` until they change browser state.
- html5lib-tests stay the parser gate until testharness runs `html/syntax/parsing/`. Browser-crate JS/DOM cargo tests are stand-ins until the first testharness file is green; delete them then. They are not a second web suite.

Invocation: `pip install -e tools/wpt` into the WPT venv, then `./tools/wpt/run [tests]`.

WebIDL: host objects are hand-written around `NodeId` (or page-owned handles). Do not codegen bindings. Other APIs may be JS.

## Options considered

- **CDP as the WPT driver:** matches a future agent product, not wptrunner’s testharness executor. Rejected for this gate.
- **In-process `Page` loader instead of WebDriver:** same HTML files, not the canonical runner. Rejected.
- **Sparse WPT checkout:** smaller clone; cannot run an arbitrary test when adding an API. Rejected.
- **JS polyfills for Node/Document/Event:** fails WebIDL branding and WPT. Rejected.
- **Build-time IDL codegen:** fights hand-written classes. wasm-bindgen-style warn-and-skip hides drift. Rejected.
- **weedle flatten/diff against webref:** not in tree. First evidence of WebIDL shape is testharness.
- **Machine `/etc/hosts` for WPT names:** not portable; rejected in favor of `--resolve` on `AgentBuilder` plus a skip in `./tools/wpt/run`.
- **Reuse a dirty persistent profile for WPT:** leaks cookies and tabs across testharness sessions. Rejected.
