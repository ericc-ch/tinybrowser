# WPT harness

`tools/wpt/run` runs `wpt run` against tinybrowser. All logic lives in the
single Python CLI `tools/wpt/cli.py` (`run` | `score` | `rerun` |
`overnight` | `selftest`); `tools/wpt/run` and `tools/wpt/rerun` are thin
shims that find a Python before the venv exists (`tools/wpt/retest` is a
compat alias for `rerun`). `tools/wpt/failures` and `tools/wpt/baseline` are
standalone shell tools for failure-list diffs. The CLI builds the
`wpt`-profile binary through cargo on
every run (cargo's fingerprint no-ops when fresh): dev's assertions with
`opt-level = 2`, so file timeouts measure the engine instead of a `-O0`
QuickJS spending the whole budget. Long-timeout files (`<meta name=timeout
content=long>`) get 30 s instead of upstream's 60 s: every passing long file
here finishes in single seconds, so a hang costs half a minute. It installs the
wptrunner product into WPT's venv only when the venv is missing or its stamp
changed: the stamp covers the `wpt run` requirements files (manifest,
wptrunner, serve), `tools/wpt/pyproject.toml`, the python version, and the
worktree root (one shared venv serves every worktree; the adapter install is
editable, so adapter edits need no reinstall). The install adds
`setuptools<71` because uv seeds only pip on Python 3.12 and WPT's own
per-run requirements check needs `pkg_resources`; newer setuptools imports the
top-level `packaging`, which WPT's sys.path resolves to its vendored copy, and
fails there. Without a working `pkg_resources` WPT shells out to
`pip install` three times every run. The manifest walk is skipped with
`--no-manifest-update` when the checkout state and manifest bytes match the
last walked run. It defaults `--processes` to the CPU count (unless
`--processes`, `--fully-parallel`, or `-f` is set), `--test-types
testharness crashtest`, enables HTTPS with the wptserve CA, and passes
`--resolve` maps instead of editing `/etc/hosts`. `TINYBROWSER_BINARY` uses
a prebuilt binary as-is with no freshness check (it says so on stderr).
Browser start-up polls the WebDriver port every 10 ms (upstream polls every
500 ms; the binary listens in under 0.1 s).

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

## Baseline comparison

A conformance change is a failure-list diff, not a pass count: run the same
slice at the base revision and compare. `tools/wpt/failures` extracts the
canonical list from a `--score` log (`STATUS PATH :: SUBTEST`, sorted and
deduplicated; a top-level result prints as `STATUS PATH :: [top-level]`).
`tools/wpt/baseline` runs both sides and prints the diff:

```sh
# exits 1 when the working tree has failures the revision does not
./tools/wpt/baseline d78b30e dom/nodes/ -- --processes 8
# one side already ran: extract and diff by hand
./tools/wpt/failures /tmp/run.log > /tmp/run.failures
comm -13 /tmp/base.failures /tmp/run.failures   # failures only in the run
```

The baseline binary is built in a detached worktree under the temp directory
(this checkout's build submodules are copied in; the WPT submodule and venv
stay shared), copied out, and the worktree is removed on exit. Both sides run
this checkout's WPT, so only the browser differs. The script enters the dev
shell itself; it prints the temp directory with the two logs.

A test262 report needs the test type repeated, because the default run
selects testharness and crashtest only:

```sh
nix develop --command ./tools/wpt/rerun /tmp/test262.json -- --test-types test262
```

Tests baselined `expected: TIMEOUT` in `tools/wpt/metadata` wait on a
capability the engine does not have (for example `transitionstart` or
`animationstart` with no CSS transitions or animations, or Perform
Actions), so each one costs its full timeout every run. In a dev loop,
skip them with upstream's `--skip-timeout`; they are reported as SKIP:

```sh
nix develop --command ./tools/wpt/run dom/nodes/ -- --skip-timeout
```

`score` and `overnight` refuse `--skip-timeout`, so scored runs always pay
for and report every expected TIMEOUT. Baseline a TIMEOUT only when the
missing capability is understood, like any other expectation.

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
| Longest-first split | The default split gives each process one fixed queue with no work stealing; upstream fills it by test-id hash, so one process can draw several long tests. With known durations the harness assigns tests longest-first to the least-loaded process instead (`wpt: longest-first split, N tests with known durations`); tests never timed cost the median. Every run records its per-test durations in `~/.cache/tinybrowser/wpt-durations.json`, so the second run of a slice is balanced. `TINYBROWSER_WPT_DURATIONS=REPORT.json` reads durations from any `--log-wptreport` output instead; `TINYBROWSER_WPT_DURATIONS=` (empty) keeps the hash split. `--run-by-dir`, `--test-groups`, and `-f` keep their upstream grouping. |
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
