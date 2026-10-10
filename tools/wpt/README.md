# WPT harness

`tools/wpt/run` runs `wpt run` against tinybrowser. All logic lives in the
single Python CLI `tools/wpt/cli.py` (`run` | `score` | `rerun` |
`overnight` | `selftest`); `tools/wpt/run` and `tools/wpt/rerun` are thin
shims that find a Python before the venv exists (`tools/wpt/retest` is a
compat alias for `rerun`). The CLI builds the debug binary through cargo on
every run (cargo's fingerprint no-ops when fresh), and installs the wptrunner
product into WPT's venv only when the venv is missing or its stamp changed:
the stamp covers the requirements files, `tools/wpt/pyproject.toml`, the
python version, and the worktree root (one shared venv serves every
worktree; the adapter install is editable, so adapter edits need no
reinstall). The manifest walk is skipped with `--no-manifest-update` when the
checkout state and manifest bytes match the last walked run. It defaults
`--processes` to the CPU count (unless `--processes`, `--fully-parallel`, or
`-f` is set), `--test-types testharness crashtest`, enables HTTPS with the
wptserve CA, and passes `--resolve` maps instead of editing `/etc/hosts`.
`TINYBROWSER_BINARY` uses a prebuilt binary as-is with no freshness check
(it says so on stderr).

```sh
nix develop --command ./tools/wpt/run dom/events/ --exclude=worker
nix develop --command ./tools/wpt/run --score dom/nodes/ -- --processes 4
nix develop --command ./tools/wpt/run --score css/css-color/ -- --test-types reftest --processes 4
```

Test paths and our flags come first; a literal `--` starts verbatim upstream
flags, and an upstream flag before `--` implicitly starts that region, so
both `run dom/events/ --exclude=worker` and `run dom/events/ -- --exclude=worker`
work. `--exclude=worker` also skips Worker variants (`.any.worker.html`,
`.worker.html`), not only URL prefix `/worker`. Those tests are omitted, not
run to a fail. Prove the filter with `nix develop --command python3 tools/wpt/cli.py selftest`.

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
nix develop --command ./tools/wpt/rerun /tmp/fileapi.json -- --processes 8 --fully-parallel
```

`tools/wpt/rerun REPORT.json` feeds the tests that need attention back to
`run --include-file`, so passing tests are not re-run. TIMEOUTs are excluded
by default (`--include-timeout` adds them): they are usually blocked
capabilities, and re-running them only pays the timeout. A TIMEOUT that
appears in a rerun report is always reported and kept, even when the
selection did not include TIMEOUTs. `--dry-run` lists the selection.

A test262 report needs the test type repeated, because the default run
selects testharness and crashtest only:

```sh
nix develop --command ./tools/wpt/rerun /tmp/test262.json -- --test-types test262
```

`tools/wpt/cli.py overnight` runs the full suite (no path filter) and scores
it. It takes minutes to hours; it needs explicit approval per `AGENTS.md`
like any suite run.

In `docs/progress.md`, replace the latest total and scored groups only.

## Enabled

| Capability | Notes |
|---|---|
| testharness | Multiple windows and same-site frames. |
| crashtest | Page must load and settle without killing the renderer. |
| reftest | Screenshot comparison through the WebDriver screenshot route; the engine has no chrome, so the outer window equals the inner 800x600 viewport. |
| test262 | Served as generated `.test262.html` wrappers through the testharness executor. |
| wdspec, aamtest | Registered as red foundations (pytest executors). pytest is not vendored into the venv yet, so results are infra ERROR until then and until the engine grows the needed surface; end-to-end probe pending an approved run. |
| HTTPS | `--ssl-type=openssl`; the generated CA is passed as `--tls-ca`. The same connector carries WSS, but no WSS test has been run yet. |
| testdriver | `supports_testdriver = True`; click, send keys, cookies, window rect. Actions/bless/permissions are engine-red foundations; BiDi later. |
| Parallel processes | CPU count unless the command sets `--processes`, `--fully-parallel`, or `-f`. Each process gets its own browser and ports. `--fully-parallel`/`-f` is separate: every test is its own group, so the browser restarts per test. The count actually used is echoed as `wpt: processes N`. |
| Skips and locks | The venv install holds the shared-venv lock only around an install that is still needed, then releases it before tests; a run that skips the install takes no lock. A run that lets wpt rewrite the shared manifest holds a manifest lock for the whole invocation; `--no-manifest-update` runs stay parallel. Runs passing `--install-browser`, `--install-webdriver`, `--manifest-download`, or `--install-fonts` hold the venv lock throughout, since wpt itself then writes into the venv. |

## Blocked on engine capabilities

These are visible in runs and fail honestly; they are not harness restrictions.

| Capability | Blocked tests |
|---|---|
| Perform Actions / pointer + key input | ~660 testdriver files |
| User activation (`test_driver.bless`) | ~340 files |
| Permissions, BiDi, Web Bluetooth | ~350 files |

## Test types without an executor

`print-reftest` is not registered for this product, so `wpt run` reports an
unsupported test type and runs nothing for it.

## Conventions

- Metadata baselines live in `tools/wpt/metadata`. An unexpected run should
  add or update an `expected: FAIL` entry only when the failure is understood;
  an unexplained failure is a bug to fix, not a baseline.
- Cargo tests cover tinybrowser-specific behavior only. Web-platform behavior
  belongs to WPT (`AGENTS.md`).
