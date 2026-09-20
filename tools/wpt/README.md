# WPT harness

`tools/wpt/run` builds the debug binary, installs the wptrunner product into
WPT's venv, and runs `wpt run` against tinybrowser. It defaults to
`--test-types testharness crashtest`, enables HTTPS with the wptserve CA, and
passes `--resolve` maps instead of editing `/etc/hosts`.

```sh
nix develop --command ./tools/wpt/run dom/events/ --exclude=worker
nix develop --command ./tools/wpt/run --score dom/nodes/ -- --processes 4
nix develop --command ./tools/wpt/run --score css/css-color/ -- --test-types reftest --processes 4
```

`--exclude=worker` also skips Worker variants (`.any.worker.html`, `.worker.html`), not only URL prefix `/worker`. Those tests are omitted, not run to a fail. Prove the filter with `nix develop --command python3 tools/wpt/launch.py --selftest`.

`tools/wpt/run --score` prints one row per directory with pass, expected-fail, and
unexpected buckets, subtest counts, and test time, then lists what needs
attention. It is the grind instrument; `--report FILE` summarizes an existing
`--log-wptreport`. Runner options follow a literal `--`.

`--save-report FILE` keeps the wptreport for the tight loop:

```sh
# score a directory and keep the report
nix develop --command ./tools/wpt/run --score FileAPI/ --save-report /tmp/fileapi.json -- \
  --exclude=worker --processes 8 --fully-parallel
# after a fix, re-run only the tests that needed attention
nix develop --command ./tools/wpt/retest /tmp/fileapi.json -- --processes 8 --fully-parallel
```

`tools/wpt/retest REPORT.json` feeds the tests that need attention back to
`run --include-file`, so passing tests are not re-run. TIMEOUTs are excluded
by default (`--include-timeout` adds them): they are usually blocked
capabilities, and re-running them only pays the timeout. A TIMEOUT that
appears in a retest report is always reported and kept, even when the
selection did not include TIMEOUTs. `--dry-run` lists the selection.

A test262 report needs the test type repeated, because the default run
selects testharness and crashtest only:

```sh
nix develop --command ./tools/wpt/retest /tmp/test262.json -- --test-types test262
```

In `docs/progress.md`, replace the latest total and scored groups only.

## Enabled

| Capability | Notes |
|---|---|
| testharness | Multiple windows and same-site frames. |
| crashtest | Page must load and settle without killing the renderer. |
| reftest | Screenshot comparison through the WebDriver screenshot route; the engine has no chrome, so the outer window equals the inner 800x600 viewport. |
| HTTPS | `--ssl-type=openssl`; the generated CA is passed as `--tls-ca`. The same connector carries WSS, but no WSS test has been run yet. |
| testdriver | `supports_testdriver = True`; click, send keys, cookies, window rect. |
| Parallel processes | `--processes N` (each process gets its own browser and ports). |

## Blocked on engine capabilities

These are visible in runs and fail honestly; they are not harness restrictions.

| Capability | Blocked tests |
|---|---|
| Perform Actions / pointer + key input | ~660 testdriver files |
| User activation (`test_driver.bless`) | ~340 files |
| Permissions, BiDi, Web Bluetooth | ~350 files |

## Test types without an executor

`aamtest`, `print-reftest`, and `wdspec` are not registered for this product,
so `wpt run` reports an unsupported test type and runs nothing for them.
`wdspec` needs pytest plus a wider WebDriver command surface (element
properties, frames, actions).

## Conventions

- Metadata baselines live in `tools/wpt/metadata`. An unexpected run should
  add or update an `expected: FAIL` entry only when the failure is understood;
  an unexplained failure is a bug to fix, not a baseline.
- Cargo tests cover tinybrowser-specific behavior only. Web-platform behavior
  belongs to WPT (`AGENTS.md`).
