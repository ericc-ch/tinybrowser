# WPT harness

tools/wpt/run runs wpt run against tinybrowser. All logic in
tools/wpt/cli.py (run | score | rerun | merge | overnight | selftest).
Builds the wpt-profile binary via cargo each run; TINYBROWSER_BINARY
uses a prebuilt binary as-is.

    nix develop --command ./tools/wpt/run dom/events/ --exclude=worker
    nix develop --command ./tools/wpt/run --score dom/nodes/ -- --processes 4

Paths and our flags first; literal -- starts upstream flags.
--exclude=worker skips Worker variants too. Prove with cli.py selftest.

Tight loop: --score prints per-directory buckets; --save-report keeps
the wptreport; rerun feeds attention-tests back (TIMEOUTs excluded
unless --include-timeout); --dry-run lists the selection.

A conformance change is a failure-list diff: failures extracts the
canonical list, baseline runs both sides in a detached worktree (exit 1
on new failures). Metadata-expected TIMEOUTs: --skip-timeout in dev
only; score/overnight refuse it. overnight needs explicit approval.
Update the docs/progress.md total after scoring. Timeout policy
(DEFAULT/LONG/EXTRA seconds) lives at the top of tinybrowser_wpt.py.

Enabled: testharness (multi-window, same-site frames), crashtest,
reftest (screenshot compare, 800x600), test262 (generated wrappers),
HTTPS (--ssl-type=openssl; WSS unrun), testdriver (click/keys/cookies/
rect). wdspec/aamtest are red foundations (pytest not vendored).

Blocked on engine, not harness: Perform Actions (~660 files), user
activation bless (~340), Permissions/BiDi/Web Bluetooth (~350).
print-reftest is unregistered.

Conventions: metadata in tools/wpt/metadata; expected FAIL only when
understood. Cargo tests cover internals only.
