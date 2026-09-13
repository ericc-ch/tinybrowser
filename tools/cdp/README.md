# Blink CDP corpus

The runner executes Chromium's vendored inspector-protocol tests against a
tinybrowser daemon. The test scripts drive the browser over the browser CDP
WebSocket; they do not execute inside the page being tested.

```bash
# Tests promoted to the required passing set.
./tools/cdp/run

# One test or glob.
./tools/cdp/run 'plain/sessions/runtime-evaluate.js'

# The complete exploratory scoreboard.
./tools/cdp/run --all
```

Each test runs in its own Node worker process with a fresh temporary profile
and daemon, so hangs and crashes become results instead of ending the run. The
parent enforces `--timeout` (per test, default 3000 ms) and reports every case
as one of:

- `PASS`
- `UNSUPPORTED_METHOD` — the first missing CDP method
- `PROTOCOL_FAILURE` — output differs from the vendored `-expected.txt`
- `MISSING_FIXTURE` — the test needs Chromium's fixed-port test servers or
  fixtures outside the two imported trees
- `HARNESS_UNSUPPORTED` — the control script needs a `content_shell` host
  feature such as `DevToolsAPI`
- `TIMEOUT`, `CRASH`, `HARNESS_FAILURE`

Results are written to `target/cdp-results.json`. `./tools/cdp/run` also runs
`node --test tools/cdp/runner.test.mjs`, which proves the parent deadline,
crash continuation, and cleanup with a fake worker. Those are tinybrowser-owned
runner invariants, not browser conformance.

Tests that reference absolute `http`/`ws` destinations are classified
`MISSING_FIXTURE` before execution: the static fixture server cannot serve
Chromium's `*.test` hostnames, HTTPS certificates, or PHP handlers, and the
runner must not contact unrelated local services on fixed ports. The fixture
server mounts the vendored tree at Chromium's `/inspector-protocol/` URL space,
so `startBlank` loads Chromium's dummy page and root-relative fixture paths
resolve the same way they do upstream.

The complete upstream snapshot is under `third_party/blink-cdp`. Change its
`REVISION`, update the pin in `third_party/VENDORED.md`, then run
`./tools/cdp/update` to update it. Never edit the vendored test files or
expected output.
