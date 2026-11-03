# WPT harness

tools/wpt/cli.py is the whole harness (run | score | rerun | merge | profile |
failures | baseline | manifest | fast | overnight | selftest). It needs a
dev-shell Python, so call it through `nix develop --command`. Builds the
wpt-profile binary via cargo each run; --binary PATH uses a prebuilt binary
as-is.

    nix develop --command python3 tools/wpt/cli.py run dom/events/ --exclude=worker
    nix develop --command python3 tools/wpt/cli.py run --score dom/nodes/ -- --processes 4
    nix develop --command python3 tools/wpt/cli.py fast test/language --jobs 12

Paths and our flags first; literal -- starts upstream flags.
--exclude=worker skips Worker variants too. Prove with `... selftest`.

A linked worktree may leave third_party/wpt uninitialized: the harness
falls back to the canonical checkout (the first `git worktree list`
entry), and `check_pin` still compares that checkout's HEAD with the
worktree's gitlink. Use `--wpt-root` for a different pin;
`--allow-pin-mismatch` only when the difference is known to be harmless.

Tight loop: --score prints per-directory buckets; --save-report keeps
the wptreport; rerun feeds attention-tests back (TIMEOUTs excluded
unless --include-timeout); --dry-run lists the selection. profile
ranks tests by wall time with --report, or runs a path and prints the
browser's navigation/request timeline.

`fast` is the native test262 lane: tools/wpt-fast runs each `.js` file in a
fresh QuickJS context with the test262 harness, no HTML, testharness.js,
wptserve, or WebDriver. The suite is read-only; strict variants are a
`"use strict";` prefix added in memory. It emits a wptreport with
`--report FILE`; `--baseline FILE` diffs the next run against that report
and exits 1 on a regression. The report has no WPT metadata, so every
failure in it reads as unexpected to `cli.py`; it is a native baseline, not
a replacement for the full score. Module tests load through QuickJS's module
loader; import attributes (JSON/text) and `$262.agent` are not implemented
yet. On this machine the 53,640 test262 files take ~27 s wall across 12 jobs
(~2m25s CPU) versus 20,587 s in the harness.

`fast-harness` is the in-process testharness/crashtest lane: each file is
mounted in the real engine in the harness process, with no WebDriver,
wptserve, or renderer child. Requests are served from the checkout, the
only replaced resource is `/resources/testharnessreport.js` (a small
reporter the host polls), and a paused Tokio clock jumps over timer waits,
so a hung file costs its harness budget instead of a browser round trip.
Budgets match upstream: 5 s, or 7 s for `<meta name=timeout content=long>`.
Reftests and testdriver files are selected but skipped: `testdriver.js`
needs a WebDriver session, and screenshot comparison is not implemented.

    nix develop --command python3 tools/wpt/cli.py fast-harness dom/nodes --jobs 8

`--cache FILE` keeps verdicts keyed by the harness binary plus the SHA-256 of
every file a test read, so an unchanged file is reused without running it.
The digest of the `wpt` binary (hundreds of MB with debug info) is remembered
in `FILE.binary` and refreshed when the executable's path, size, or mtime
changes. On dom/nodes/ (100 files) a cold run is ~6 s at 8 jobs; a warm run is
~15 ms. A truncated file is never cached as a verdict: testharness reports
`interrupted` when the engine cuts a script short, and the runner gives each
file one drain with its whole budget instead of a short slice.

`--shard-size N` (in cli.py) runs the sorted selection in N-file processes.
Each shard is a separate `--offset`/`--limit` slice, so an engine abort costs
one shard instead of the run; a shard that exits non-zero is bisected until
the single crashing file is found, and that file is recorded as ERROR. The
driver owns the window: a `--offset`/`--limit` on the command line slices the
selection once and every shard's slice stays inside it, and the other runner
flags (`--cache`, `--jobs`, `--deps`/`--changed`) pass through with their
values. Use it for directories where layout or paint can panic (html/
currently does): a panic in-process bypasses the harness's per-file error
framing, and unwinding a half-torn engine can abort outright. dom/ needs no
sharding and runs 692 files in ~25 s at 16 jobs, with 557/557 file verdicts
matching the nightly report and 93 files skipped (testdriver/reftest). html/
(11,252 files, 58 shards) took ~322 s of shard wall with 5 crashing files
isolated, and 6,238/6,238 verdicts matched. fetch+xhr+url+encoding+FileAPI+
streams (638 files) is 45 s cold and 0.6 s warm, 325/325 matched. The nightly
report gives 8,140 s of per-test time for those three slices together.

A warm cache needs no sharding, so the driver tries one process first and
only splits when it dies: html/ warm is 1.2 s at 16 jobs (3 s through the
driver) against those 7,206 s. A file that aborts the engine cannot record
its own verdict, so the driver marks it with `--mark-crashed` and later runs
read it as an ordinary cached ERROR instead of bisecting again.

`--deps MAP --changed FILE` narrows a run to the tests that executed a
changed repository file. `--changed` is one path per line, relative to the
repository root; MAP is `{"version":1,"tests":{id:[source paths]}}` built by
`tools/wpt/deps.py build` from an instrumented run. The rule is conservative
in one place: a changed path the map never saw means the map cannot bound the
blast radius, so every selected test runs. A file's own bytes are still an
input through `--cache`, so editing a test reruns it even when the map is
stale.

Building a full map is a scheduled job, not a per-push step: `-C
instrument-coverage` makes process setup cost ~19 s here (fontconfig through
instrumented Rust) and each test ~0.2 s. `--batch N` pays that setup once per
N tests and gives every test in the batch the union of what the batch
executed, which over-selects and never under-selects. Measured: dom/nodes
(374 tests) at `--batch 50 --jobs 4` took 1 m 47 s and yields a 50-test
granularity floor; a change to `window_features.rs` selected 24 tests and ran
in 1.5 s against 10.9 s for the directory, while a core file such as `xml.rs`
selected all 374 and ran in 11.0 s. Larger `--batch` is cheaper to record and
coarser to select.

Known gap: an async listener that leaves a host object borrowed makes
rquickjs panic with `AlreadyBorrowed` in `Class::borrow_mut`; rquickjs
converts that panic into a script exception, so the run still finishes and
writes its report. It is an engine re-entrancy bug to fix, not a harness one.

The host serves files and their `.headers` siblings only, and `document.cookie`
goes through the browser's own jar (`crates/cookies`). wptserve's query-param
handlers, `.sub.` substitution files, and server-set `Set-Cookie` are not
implemented, so a test that depends on them cannot pass here even when it
passes in the nightly; that is why the parity numbers above are quoted per
directory. Popups, workers, and WebDriver are out of scope: `window.open`
returns null and `testdriver.js` files are skipped.

A conformance change is a failure-list diff: failures prints the
canonical list of a --score log, baseline runs both sides in a detached
worktree (exit 1 on new failures). Metadata-expected TIMEOUTs:
--skip-timeout in dev only; score/overnight refuse it. overnight needs
explicit approval. Update the docs/progress.md total after scoring.
Timeout policy (DEFAULT/LONG/EXTRA seconds) lives at the top of
tinybrowser_wpt.py.

A test file whose own timers outlast DEFAULT_TIMEOUT cannot be sped up
from the engine: `referrer-policy/css-integration/svg/` sleeps 800 ms
per subtest (nine serially) before it checks anything. SLOW_TEST_PREFIXES
in tinybrowser_wpt.py gives those directories SLOW_TEST_TIMEOUT, still
below testharness.js's 10 s clock so the runner kill lands first. That
list is only for waits written into test code, never for a slow engine
path; an entry whose reason expired falls back to the default budget and
its files fail visibly.

Enabled: testharness (multi-window, same-site frames), crashtest,
reftest (screenshot compare, 800x600), test262 (generated wrappers),
HTTPS (--ssl-type=openssl; WSS unrun), testdriver (click/keys/cookies/
rect). wdspec/aamtest are red foundations (pytest not vendored).

Blocked on engine, not harness: Perform Actions (~660 files), user
activation bless (~340), Permissions/BiDi/Web Bluetooth (~350).
print-reftest is unregistered.

Conventions: metadata in tools/wpt/metadata; expected FAIL only when
understood. Cargo tests cover internals only.
