# Vendored Blink CDP corpus

Chromium's two inspector-protocol web-test trees are vendored in full and run
through a tiny Node host that drives tinybrowser over its browser CDP
WebSocket. This is a CDP behavior gate, separate from both WPT and the
Playwright consumer smoke test.

Status: accepted.

## Source and update model

The corpus lives inside Chromium's monorepo rather than in a standalone Git
repository. A submodule cannot point at two directories within that repository,
and cloning Chromium violates the source-reading and repository-size rules.

`third_party/blink-cdp` is therefore a checked-in snapshot of:

- `third_party/blink/web_tests/inspector-protocol/`
- `third_party/blink/web_tests/http/tests/inspector-protocol/`

`third_party/blink-cdp/REVISION` pins the Chromium commit. `./tools/cdp/update`
downloads both Gitiles directory archives at that exact commit and refreshes
the snapshot. Normal test runs perform no network access.

Vendored tests and `-expected.txt` files remain unmodified. Harness policy and
tinybrowser-specific status live under `tools/cdp`.

## Runner

`./tools/cdp/run` runs tests promoted in `tools/cdp/passing.txt`. These are a
required exact-output gate.

`./tools/cdp/run --all` attempts every vendored JavaScript test that has a
matching expected text file, terminates each case in a category, and writes the
complete report to `target/cdp-results.json`. This exploratory mode may fail
and is not a required CI gate until its results have a maintained baseline.

Every test runs in its own Node worker process. The parent owns the deadline,
kills the worker's process group on timeout or interruption, and keeps
scheduling after a worker crashes, so one test cannot stall or end the run.
Every test also receives a fresh temporary Profile and daemon inside that
worker. The Node host owns the control script, implements the upstream
`TestRunner`, `Page`, and `Session` shape, serves vendored fixtures at
Chromium's `/inspector-protocol/` URL space, and forwards protocol messages
over the browser WebSocket. Test control code does not run in the document
under test, and no test-only API is added to the binary.

Cases terminate as `PASS`, `UNSUPPORTED_METHOD`, `PROTOCOL_FAILURE`,
`MISSING_FIXTURE`, `HARNESS_UNSUPPORTED`, `TIMEOUT`, `CRASH`, or
`HARNESS_FAILURE`. Tests whose control script or referenced HTML fixtures
mention absolute `http`/`ws` destinations are classified `MISSING_FIXTURE`
before execution instead of contacting Chromium's fixed-port test servers or
any unrelated local service. Control-script `fetch` is confined to the
per-test fixture origin.

The fixture server starts as a static server. Chromium PHP handlers, shared
`web_tests` resources outside the imported trees, and `content_shell`-only host
operations are reported as harness gaps rather than silently emulated.
`content_shell`'s `DevToolsAPI` capture hooks are not emulated; tests that
patch them are reported as `HARNESS_UNSUPPORTED`.

`node --test tools/cdp/runner.test.mjs` (also run by `./tools/cdp/run`) proves
the parent deadline, crash continuation, and signal cleanup with a fake
worker, so the runner contract is tested without asserting browser behavior.

## Boundaries with other gates

- Cargo tests cover tinybrowser-owned protocol transport, process lifecycle,
  resource budgets, and deterministic invariants.
- The Blink corpus checks CDP response and event behavior against Chromium's
  expected output.
- `./tools/playwright/run` proves that a real Playwright release can attach and
  drive the supported slice. It may tolerate missing CDP methods and is not a
  conformance replacement.
- `./tools/wpt/run` remains the web-platform conformance gate over WebDriver
  ([ADR 0008](0008-wpt-via-webdriver.md)).

## Options considered

- **Chromium submodule:** would bring the monorepo rather than just the two test
  trees. Rejected.
- **Download tests during every run:** makes results depend on the network and
  moving upstream state. Rejected.
- **Import only a passing allowlist:** makes unsupported tests invisible and
  complicates updates. Rejected in favor of a full snapshot plus two run modes.
- **Run control scripts inside tinybrowser:** conflates the test host with the
  runtime under test and requires fake `content_shell` globals. Rejected.
- **Replace Playwright with the corpus:** loses proof that an actual external
  client accepts tinybrowser's discovery, attachment, and lifecycle behavior.
  Rejected.
