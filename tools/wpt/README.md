# WPT harness

`tools/wpt/run` builds the debug binary, installs the wptrunner product into
WPT's venv, and runs `wpt run` against tinybrowser. It defaults to
`--test-types testharness crashtest`, enables HTTPS with the wptserve CA, and
passes `--resolve` maps instead of editing `/etc/hosts`.

```sh
nix develop --command ./tools/wpt/run dom/events/ --exclude=worker
nix develop --command ./tools/wpt/score dom/nodes/ -- --processes 4
```

`--exclude=worker` also skips Worker variants (`.any.worker.html`, `.worker.html`), not only URL prefix `/worker`. Those tests are omitted, not run to a fail. Prove the filter with `python3 tools/wpt/launch.py --selftest`.

`tools/wpt/score` prints one row per directory with pass, expected-fail, and
unexpected buckets, subtest counts, and test time, then lists what needs
attention. It is the grind instrument; `--report FILE` summarizes an existing
`--log-wptreport`. Runner options follow a literal `--`.

In `docs/progress.md`, replace the latest total and scored groups only.

## Enabled

| Capability | Notes |
|---|---|
| testharness | Multiple windows and same-site frames. |
| crashtest | Page must load and settle without killing the renderer. |
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

`reftest`, `print-reftest`, and `wdspec` are not registered for this product,
so `wpt run` reports an unsupported test type and runs nothing for them.
Reftests need layout and rendering; wdspec needs pytest plus a wider WebDriver
command surface (element properties, frames, actions, screenshots).

## Conventions

- Metadata baselines live in `tools/wpt/metadata`. An unexpected run should
  add or update an `expected: FAIL` entry only when the failure is understood;
  an unexplained failure is a bug to fix, not a baseline.
- Cargo tests cover tinybrowser-specific behavior only. Web-platform behavior
  belongs to WPT (`AGENTS.md`).
