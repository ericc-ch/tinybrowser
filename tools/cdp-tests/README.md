# Blink CDP corpus

Runs Chromium's vendored inspector-protocol tests against a tinybrowser
daemon. Scripts drive the browser over CDP; they never run in the page.

    ./tools/cdp-tests/run 'plain/sessions/runtime-evaluate.js'
    ./tools/cdp-tests/run --all

Each test gets a Node worker, fresh profile, and daemon; hangs/crashes
are results. Per-test --timeout (default 3000ms). Results:
PASS, UNSUPPORTED_METHOD, PROTOCOL_FAILURE, MISSING_FIXTURE,
HARNESS_UNSUPPORTED, TIMEOUT, CRASH, HARNESS_FAILURE.
Written to target/cdp-results.json.

Absolute http/ws destinations are MISSING_FIXTURE before execution
(no Chromium hostnames, certs, or PHP handlers; no fixed-port calls).
Snapshot in third_party/blink-cdp: change REVISION, run
./tools/cdp-tests/update. Never edit vendored tests or expected output.
