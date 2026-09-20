#!/usr/bin/env python3
"""Summarize a WPT run per directory.

Runs the test set through `tools/wpt/run` and prints one row per directory
with pass, expected-fail, and unexpected buckets plus subtest counts and test
time. This is the conformance-grind instrument: a directory's row is the unit
of work.

    tools/wpt/run --score dom/nodes/ | head
    tools/wpt/run --score dom/nodes/ -- --processes 4
    tools/wpt/run --score --report report.json     # summarize an existing wptreport
    tools/wpt/run --score FileAPI/ --save-report /tmp/fileapi.json   # keep the report

`--save-report FILE` keeps the wptreport a `tools/wpt/retest FILE` run needs.
Without it the report is temporary and removed after a successful run.

Runner arguments follow a literal `--`; everything before it is a test path
(or `--report FILE` / `--save-report FILE`). A nonzero exit means the runner
failed, not that tests failed: the run always passes `--no-fail-on-unexpected`.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import time
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
RUNNER = ROOT / "tools" / "wpt" / "run"

# File-level statuses that get their own bucket; anything else that is not a
# pass is classified by the report's `expected` field.
HARD_STATUSES = ("TIMEOUT", "CRASH", "ERROR")
MAX_LISTED = 40


def group_of(test: str) -> str:
    """The first two path segments of a WPT test URL."""
    parts = [part for part in test.split("/") if part]
    if len(parts) <= 1:
        return "(root)"
    return "/".join(parts[:2])


def is_expected(entry: dict) -> bool:
    """Whether a result or subtest matched its expectation.

    mozlog only writes `expected` when the actual status differs from the
    expected one, and `known_intermittent` lists statuses that are acceptable
    for the test.
    """
    if entry.get("expected") is None:
        return True
    return entry.get("status") in (entry.get("known_intermittent") or [])


def unexpected_subs(subtests: list) -> list[dict]:
    """Subtests whose status did not match the baseline, including PASS."""
    return [
        subtest
        for subtest in subtests
        if subtest.get("status") != "SKIP" and not is_expected(subtest)
    ]


def classify_results(
    results: list[dict],
) -> tuple[dict[str, dict[str, int]], list[tuple[str, str, str]]]:
    """Bucket each file and collect rows that need attention.

    A baselined FAIL that starts passing is unexpected, at file and subtest
    level: that is the grind signal that a baseline should come out.
    """
    rows: dict[str, dict[str, int]] = defaultdict(lambda: defaultdict(int))
    attention: list[tuple[str, str, str]] = []

    for result in results:
        test = result.get("test", "?")
        row = rows[group_of(test)]
        row["tests"] += 1
        row["time_ms"] += result.get("duration") or 0

        subtests = result.get("subtests") or []
        mismatched = unexpected_subs(subtests)
        sub_failed = [
            subtest for subtest in subtests if subtest.get("status") not in ("PASS", "SKIP")
        ]
        row["subtests"] += len(subtests)
        row["subfail"] += len(sub_failed)

        status = result.get("status", "ERROR")
        if status == "SKIP":
            row["skip"] += 1
        elif status in HARD_STATUSES:
            row[status.lower()] += 1
            message = (result.get("message") or "").strip()
            attention.append((test, "", f"{status} {message}".strip()))
        elif not is_expected(result) or mismatched:
            row["unexpected"] += 1
            if not is_expected(result):
                message = (result.get("message") or "").strip()
                attention.append((test, "", f"{status} {message}".strip()))
            for subtest in mismatched:
                attention.append(
                    (test, subtest.get("name", "?"), subtest.get("status", "ERROR"))
                )
        elif sub_failed:
            row["expected_fail"] += 1
        elif status in ("PASS", "OK"):
            row["pass"] += 1
        else:
            row["expected_fail"] += 1

    return rows, attention


def run(paths: list[str], extra: list[str], save_report: Path | None = None) -> tuple[Path, int]:
    if save_report is not None:
        report = save_report
        report.parent.mkdir(parents=True, exist_ok=True)
    else:
        handle, name = tempfile.mkstemp(prefix="wpt-report-", suffix=".json")
        os.close(handle)
        report = Path(name)
    command = [
        str(RUNNER),
        *paths,
        "--log-wptreport",
        str(report),
        "--no-fail-on-unexpected",
        *extra,
    ]
    print(f"$ {' '.join(command)}", file=sys.stderr)
    # The runner's progress goes to stderr; the table stays on stdout.
    exit_code = subprocess.run(command, cwd=ROOT, check=False, stdout=sys.stderr).returncode
    if exit_code != 0:
        print(f"runner exited {exit_code}", file=sys.stderr)
    return report, exit_code


def summarize(report: Path) -> int:
    try:
        data = json.loads(report.read_text())
    except OSError as error:
        print(f"cannot read report {report}: {error}", file=sys.stderr)
        return 1
    except json.JSONDecodeError as error:
        print(
            f"report {report} is not valid JSON ({error}); the runner probably "
            "failed before writing results",
            file=sys.stderr,
        )
        return 1

    results = data.get("results", [])
    if not results:
        print(f"no results in report {report}", file=sys.stderr)
        return 1

    rows, attention = classify_results(results)
    width = max(len(name) for name in rows)
    header = (
        f"{'directory'.ljust(width)}  tests  pass  expfail  unexp  timeout  crash  error"
        f"  skip  subtests  subfail  test time"
    )
    print(header)
    print("-" * len(header))

    def line(name: str, row: dict[str, int]) -> str:
        return (
            f"{name.ljust(width)}  {row['tests']:5}  {row['pass']:4}  {row['expected_fail']:7}  "
            f"{row['unexpected']:5}  {row['timeout']:7}  {row['crash']:5}  {row['error']:5}  "
            f"{row['skip']:4}  {row['subtests']:8}  {row['subfail']:7}  "
            f"{row['time_ms'] / 1000:8.1f}s"
        )

    totals: dict[str, int] = defaultdict(int)
    for directory in sorted(rows):
        for key, value in rows[directory].items():
            totals[key] += value
        print(line(directory, rows[directory]))
    print("-" * len(header))
    print(line("TOTAL", totals))

    print()
    print(f"needs attention ({len(attention)} entries):")
    for test, subtest, status in attention[:MAX_LISTED]:
        label = subtest or "(test)"
        print(f"  {status:10} {test} :: {label}")
    if len(attention) > MAX_LISTED:
        print(f"  ... and {len(attention) - MAX_LISTED} more")
    return 0


def split_args(
    argv: list[str],
) -> tuple[Path | None, Path | None, list[str], list[str], str | None]:
    """Split our arguments from the runner's.

    Paths are the leading positionals; `--` starts the runner extras;
    `--report FILE` is ours (summarize only); `--save-report FILE` runs and
    keeps the wptreport for `retest`.
    """
    report: Path | None = None
    save_report: Path | None = None
    paths: list[str] = []
    extra: list[str] = []
    index = 0
    while index < len(argv):
        arg = argv[index]
        if arg == "--":
            extra = list(argv[index + 1 :])
            break
        if arg in ("--report", "--save-report") or arg.startswith(("--report=", "--save-report=")):
            option, value = arg.split("=", 1) if "=" in arg else (arg, None)
            if value is None:
                if index + 1 >= len(argv):
                    return report, save_report, paths, [], f"{option} needs a file"
                value = argv[index + 1]
                index += 1
            if not value:
                return report, save_report, paths, [], f"{option} needs a file"
            if option == "--report":
                report = Path(value)
            else:
                save_report = Path(value)
        elif arg.startswith("-"):
            return (
                report,
                save_report,
                paths,
                list(argv[index:]),
                "runner options must follow the test paths (or a literal `--`)",
            )
        else:
            paths.append(arg)
        index += 1
    return report, save_report, paths, extra, None


def _selftest() -> None:
    rows, attention = classify_results(
        [
            {
                "test": "/html/foo.html",
                "status": "PASS",
                "expected": "FAIL",
                "subtests": [],
                "duration": 0,
            }
        ]
    )
    assert rows["html/foo.html"]["unexpected"] == 1
    assert rows["html/foo.html"]["pass"] == 0
    assert attention == [("/html/foo.html", "", "PASS")]

    rows, attention = classify_results(
        [
            {
                "test": "/dom/bar.html",
                "status": "OK",
                "subtests": [
                    {"name": "a", "status": "FAIL"},
                    {"name": "b", "status": "PASS", "expected": "FAIL"},
                ],
                "duration": 0,
            }
        ]
    )
    assert rows["dom/bar.html"]["unexpected"] == 1
    assert rows["dom/bar.html"]["pass"] == 0
    assert attention == [("/dom/bar.html", "b", "PASS")]

    rows, attention = classify_results(
        [
            {
                "test": "/dom/ok.html",
                "status": "OK",
                "subtests": [{"name": "a", "status": "PASS"}],
                "duration": 0,
            }
        ]
    )
    assert rows["dom/ok.html"]["pass"] == 1
    assert attention == []

    rows, attention = classify_results(
        [
            {
                "test": "/dom/fail.html",
                "status": "FAIL",
                "subtests": [{"name": "a", "status": "FAIL"}],
                "duration": 0,
            }
        ]
    )
    assert rows["dom/fail.html"]["expected_fail"] == 1
    assert attention == []

    report, save_report, paths, extra, error = split_args(
        ["FileAPI/", "--save-report", "/tmp/x.json", "--", "--exclude=worker"]
    )
    assert report is None
    assert save_report == Path("/tmp/x.json")
    assert paths == ["FileAPI/"]
    assert extra == ["--exclude=worker"]
    assert error is None

    report, save_report, paths, extra, error = split_args(["--report", "r.json"])
    assert report == Path("r.json")
    assert save_report is None
    assert not paths and not extra and error is None

    report, save_report, paths, extra, error = split_args(["--save-report=x.json", "dom/"])
    assert report is None
    assert save_report == Path("x.json")
    assert paths == ["dom/"]
    assert error is None

    assert split_args(["--save-report"])[4] == "--save-report needs a file"
    assert split_args(["--bogus"])[4] is not None

    report, save_report, paths, extra, error = split_args(
        ["--report", "a.json", "--save-report", "b.json"]
    )
    assert report == Path("a.json")
    assert save_report == Path("b.json")
    assert error is None
    assert split_args(["--report="])[4] == "--report needs a file"
    assert split_args(["--save-report="])[4] == "--save-report needs a file"


def main() -> int:
    if sys.argv[1:] == ["--selftest"]:
        _selftest()
        return 0
    report, save_report, paths, extra, error = split_args(sys.argv[1:])
    if error:
        print(f"run --score: {error}", file=sys.stderr)
        return 2
    if report is not None and save_report is not None:
        print(
            "run --score: --report summarizes an existing report; do not combine "
            "it with --save-report",
            file=sys.stderr,
        )
        return 2
    if report is not None:
        if paths or extra:
            print(
                "run --score: --report summarizes an existing report; give no test paths",
                file=sys.stderr,
            )
            return 2
        return summarize(report)
    if not paths:
        print("run --score: give test paths or --report FILE", file=sys.stderr)
        return 2

    started = time.monotonic()
    try:
        report_path, runner_exit = run(paths, extra, save_report)
    except OSError as error:
        print(f"run --score: could not start the runner: {error}", file=sys.stderr)
        return 1
    keep_report = runner_exit != 0 or save_report is not None
    try:
        if runner_exit != 0:
            print(
                f"runner failed with exit {runner_exit}; report kept at {report_path}",
                file=sys.stderr,
            )
            summarize(report_path)
            return runner_exit
        if keep_report:
            print(f"report kept at {report_path}", file=sys.stderr)
        return summarize(report_path)
    finally:
        if not keep_report:
            try:
                os.unlink(report_path)
            except OSError:
                pass
        print(f"wall time: {time.monotonic() - started:.1f}s", file=sys.stderr)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except BrokenPipeError:
        # `run --score ... | head` closes stdout early; exit quietly.
        devnull = os.open(os.devnull, os.O_WRONLY)
        os.dup2(devnull, sys.stdout.fileno())
        raise SystemExit(0)
