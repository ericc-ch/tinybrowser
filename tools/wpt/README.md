# WPT harness

tools/wpt/cli.py is the whole harness: `python3 tools/wpt/cli.py --help`
lists its subcommands (`run`, `score`, `fast`, `fast-harness`, and the rest).
It needs a dev-shell Python, so call it through `nix develop --command`.
Builds the wpt-profile binary via cargo each run; --binary PATH uses a
prebuilt binary as-is.

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
Budgets match the WebDriver lane's: 5 s, or 7 s for
`<meta name=timeout content=long>`; both lanes read the policy in
`tools/wpt/timeouts.py`, and cli.py passes it to the runner as
`--normal-budget`/`--long-budget`.

The selection comes from the checkout's `MANIFEST.json`, the same manifest
the WebDriver lane uses, so "a test" means what upstream says it means:
testharness and crashtest items, with the URL and `timeout` the manifest
records. Multi-global tests run through wptserve's `.any.html` and
`.window.html` wrappers, rendered from the source's `// META:` lines.
Testdriver items are selected but skipped (`testdriver.js` needs a
WebDriver session); reftests, worker/service-worker variants, and support
pages are not selected, because this lane has no executor for them.

    nix develop --command python3 tools/wpt/cli.py fast-harness dom/nodes --jobs 8

The lane has no result cache: every run executes the selection. It is fast
because the engine is in-process and the clock is paused, not because an
earlier verdict is reused. On dom/nodes/ (422 manifest tests) a run is ~11 s
at 12 jobs. A truncated file is never mistaken for a verdict: testharness
reports `interrupted` when the engine cuts a script short, and the runner
gives each file one drain with its whole budget instead of a short slice.

`--shard-size N` (in cli.py) runs the sorted selection in N-file processes.
Each shard is a separate `--offset`/`--limit` slice, so an engine abort costs
one shard instead of the run; a shard that exits non-zero is bisected until
the single crashing file is found, and that file is recorded as ERROR. The
driver owns the window: a `--offset`/`--limit` on the command line slices the
selection once and every shard's slice stays inside it, and the other runner
flags (`--jobs`) pass through with their values. Use it for directories where
layout or paint can panic (html/ currently does): a panic in-process bypasses
the harness's per-file error framing, and unwinding a half-torn engine can
abort outright. dom/ needs no sharding and runs 788 manifest tests in ~26 s
at 16 jobs, with 109 testdriver skips. html/ selects 7,895 manifest tests, so
it is sharded. The parity counts from the older tree-walking selector are
gone with it: that selector counted support pages and references (always
PASS) and could not see `.any.js`/`.window.js` sources at all, so its
"matched" numbers were over an intersection rather than the suite.

Known gap: an async listener that leaves a host object borrowed makes
rquickjs panic with `AlreadyBorrowed` in `Class::borrow_mut`; rquickjs
converts that panic into a script exception, so the run still finishes and
writes its report. It is an engine re-entrancy bug to fix, not a harness one.

The host serves files the way wptserve does: the content-type table comes from
`wptserve.constants` (cli.py hands it over), headers are `wptserve`'s
`load_headers` order (`__dir__` then the file, `.sub.headers` before
`.headers`, templates expanded), and `.any.html`/`.window.html` documents are
rendered from their sources. `document.cookie` goes through the browser's own
jar (`crates/cookies`). Still unimplemented: wptserve's query-param `pipe`
handlers, `.sub.` body substitution (a `.sub.html` is served literally), and
`.py` handlers (a network error rather than the handler's source). Popups,
workers, and WebDriver are out of scope: `window.open` returns null and
`testdriver.js` files are skipped.

**The lane runs the real fetch core.** `FsServices` is a `NetworkHost` over
`net::Agent::with_transport`: response tainting, CORS and preflight, the
no-CORS request guards, redirect policy, `Sec-Fetch-*`, referrer selection, and
cookies all run in this process, and only the one HTTP exchange is an in-memory
transport that maps `web-platform.test`, `www1.web-platform.test`, `:8443`
and the other wptserve hosts to the checkout. A file that asserts a
cross-origin `fetch` fails with `TypeError`, or that a `no-cors` response is
`opaque`, now measures the same policy the native browser does, so those files
belong in the parity numbers. Opaque Response Blocking and the browser-side
HTTP cache still live in `crates/browser` and are not exercised here.

A conformance change is a failure-list diff: failures prints the
canonical list of a --score log, baseline runs both sides in a detached
worktree (exit 1 on new failures). Metadata-expected TIMEOUTs:
--skip-timeout in dev only; score/overnight refuse it. overnight needs
explicit approval. Update the docs/progress.md total after scoring.
Timeout policy (DEFAULT/LONG/EXTRA seconds) lives in tools/wpt/timeouts.py.

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
