#!/usr/bin/env python3
"""Summarize a WPT run per directory.

Runs the test set through `tools/wpt/run` and prints one row per directory
with pass, expected-fail, and unexpected buckets plus subtest counts and test
time. This is the conformance-grind instrument: a directory's row is the unit
of work.

    tools/wpt/score dom/nodes/ | head
    tools/wpt/score dom/nodes/ -- --processes 4
    tools/wpt/score --report report.json     # summarize an existing wptreport

Runner arguments follow a literal `--`; everything before it is a test path
(or `--report FILE`). A nonzero exit means the runner failed, not that tests
failed: the run always passes `--no-fail-on-unexpected`.
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


def run(paths: list[str], extra: list[str]) -> tuple[Path, int]:
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


def split_args(argv: list[str]) -> tuple[Path | None, list[str], list[str], str | None]:
    """Split our arguments from the runner's.

    Paths are the leading positionals; `--` starts the runner extras;
    `--report FILE` is ours.
    """
    report: Path | None = None
    paths: list[str] = []
    for index, arg in enumerate(argv):
        if arg == "--":
            return report, paths, list(argv[index + 1 :]), None
        if arg == "--report":
            if index + 1 >= len(argv):
                return report, paths, [], "--report needs a file"
            report = Path(argv[index + 1])
            return report, paths, list(argv[index + 2 :]), None
        if arg.startswith("--report="):
            report = Path(arg.split("=", 1)[1])
            return report, paths, list(argv[index + 1 :]), None
        if arg.startswith("-"):
            return (
                report,
                paths,
                list(argv[index:]),
                "runner options must follow the test paths (or a literal `--`)",
            )
        paths.append(arg)
    return report, paths, [], None


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


def main() -> int:
    if sys.argv[1:] == ["--selftest"]:
        _selftest()
        return 0
    report, paths, extra, error = split_args(sys.argv[1:])
    if error:
        print(f"score: {error}", file=sys.stderr)
        return 2
    if report is not None:
        if paths or extra:
            print(
                "score: --report summarizes an existing report; give no test paths",
                file=sys.stderr,
            )
            return 2
        return summarize(report)
    if not paths:
        print("score: give test paths or --report FILE", file=sys.stderr)
        return 2

    started = time.monotonic()
    try:
        report_path, runner_exit = run(paths, extra)
    except OSError as error:
        print(f"score: could not start the runner: {error}", file=sys.stderr)
        return 1
    keep_report = runner_exit != 0
    try:
        if keep_report:
            print(
                f"runner failed with exit {runner_exit}; report kept at {report_path}",
                file=sys.stderr,
            )
            summarize(report_path)
            return runner_exit
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
        # `score ... | head` closes stdout early; exit quietly.
        devnull = os.open(os.devnull, os.O_WRONLY)
        os.dup2(devnull, sys.stdout.fileno())
        raise SystemExit(0)
