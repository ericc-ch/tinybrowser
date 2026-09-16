# WPT harness

`tools/wpt/run` builds the debug binary, installs the wptrunner product into
WPT's venv, and runs `wpt run` against tinybrowser. It defaults to
`--test-types testharness crashtest`, enables HTTPS with the wptserve CA, and
passes `--resolve` maps instead of editing `/etc/hosts`.

```sh
nix develop --command ./tools/wpt/run dom/events/ --exclude=worker
nix develop --command ./tools/wpt/score dom/nodes/ --processes 4
```

`tools/wpt/score` prints one row per directory with pass/unexpected buckets,
subtest counts, and wall time, then lists the unexpected results. It is the
grind instrument; `--report FILE` summarizes an existing `--log-wptreport`.

## Enabled

| Capability | Notes |
|---|---|
| testharness | Windows and same-site frames. |
| crashtest | Page must load and settle without killing the renderer. |
| HTTPS / WSS | `--ssl-type=openssl`; the generated CA is passed as `--tls-ca`. |
| testdriver | `supports_testdriver = True`; click, send keys, cookies, window rect. |
| Parallel processes | `--processes N` (each process gets its own browser and ports). |

## Blocked on engine capabilities

These are visible in runs and fail honestly; they are not harness restrictions.

| Capability | Blocked tests |
|---|---|
| Perform Actions / pointer + key input | ~660 testdriver files |
| `Worker` | ~1,000 `.worker.js` files |
| User activation (`test_driver.bless`) | ~340 files |
| Permissions, BiDi, Web Bluetooth | ~350 files |
| Reftests / print-reftest | Need layout and rendering |
| wdspec | Needs pytest plus a wider WebDriver command surface |

## Conventions

- Metadata baselines live in `tools/wpt/metadata`. An unexpected run should
  add or update an `expected: FAIL` entry only when the failure is understood;
  an unexplained failure is a bug to fix, not a baseline.
- Cargo tests cover tinybrowser-specific behavior only. Web-platform behavior
  belongs to WPT (`AGENTS.md`).
