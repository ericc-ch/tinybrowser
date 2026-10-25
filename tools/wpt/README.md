# WPT harness

tools/wpt/cli.py is the whole harness (run | score | rerun | merge | profile |
failures | baseline | manifest | overnight | selftest). It needs a dev-shell
Python, so call it through `nix develop --command`. Builds the wpt-profile
binary via cargo each run; --binary PATH uses a prebuilt binary as-is.

    nix develop --command python3 tools/wpt/cli.py run dom/events/ --exclude=worker
    nix develop --command python3 tools/wpt/cli.py run --score dom/nodes/ -- --processes 4

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
