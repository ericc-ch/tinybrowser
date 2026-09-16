#!/usr/bin/env python3
"""Summarize a WPT run per directory.

Runs the test set through `tools/wpt/run` and prints one row per directory
with pass/unexpected buckets, subtests, and wall time. This is the
conformance-grind instrument: a directory's row is the unit of work.

    tools/wpt/score dom/nodes/ | head
    tools/wpt/score --report report.json     # summarize an existing wptreport
    tools/wpt/score --processes 4 dom/       # parallel run

Any extra arguments after the paths are passed to `tools/wpt/run`.
"""

from __future__ import annotations

import argparse
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

STATUSES = ("PASS", "FAIL", "TIMEOUT", "CRASH", "ERROR", "SKIP")


def group_of(test: str) -> str:
    """The first two path segments of a WPT test URL."""
    parts = [part for part in test.split("/") if part]
    if len(parts) <= 1:
        return "(root)"
    return "/".join(parts[:2])


def run(paths: list[str], extra: list[str]) -> Path:
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
    result = subprocess.run(command, cwd=ROOT, check=False, stdout=sys.stderr)
    if result.returncode != 0:
        print(
            f"runner exited {result.returncode}; summarizing what it wrote",
            file=sys.stderr,
        )
    return report


def summarize(report: Path) -> int:
    data = json.loads(report.read_text())
    results = data.get("results", [])
    if not results:
        print("no results in report", file=sys.stderr)
        return 1

    buckets: dict[str, dict[str, int]] = defaultdict(
        lambda: dict.fromkeys(STATUSES, 0)
    )
    subtests: dict[str, dict[str, int]] = defaultdict(lambda: {"PASS": 0, "not PASS": 0})
    unexpected: list[tuple[str, str, str]] = []
    durations: dict[str, int] = defaultdict(int)

    for result in results:
        test = result.get("test", "?")
        status = result.get("status", "ERROR")
        directory = group_of(test)
        buckets[directory][status if status in STATUSES else "ERROR"] += 1
        durations[directory] += result.get("duration") or 0

        for subtest in result.get("subtests") or []:
            sub_status = subtest.get("status", "ERROR")
            key = "PASS" if sub_status == "PASS" else "not PASS"
            subtests[directory][key] += 1
            if key != "PASS":
                unexpected.append((test, subtest.get("name", "?"), sub_status))
        if status != "PASS":
            message = result.get("message") or ""
            unexpected.append((test, "", f"{status} {message}".strip()))

    width = max(len(name) for name in buckets)
    header = (
        f"{'directory'.ljust(width)}  tests  pass  fail  timeout  crash  error  skip"
        f"  subtests  subfail  time"
    )
    print(header)
    print("-" * len(header))
    totals = dict.fromkeys(STATUSES, 0)
    total_subtests = {"PASS": 0, "not PASS": 0}
    total_ms = 0
    for directory in sorted(buckets):
        row = buckets[directory]
        for status in STATUSES:
            totals[status] += row[status]
        subtotal = subtests[directory]
        total_subtests["PASS"] += subtotal["PASS"]
        total_subtests["not PASS"] += subtotal["not PASS"]
        total_ms += durations[directory]
        tests = sum(row.values())
        print(
            f"{directory.ljust(width)}  "
            f"{tests:5}  {row['PASS']:4}  {row['FAIL']:4}  {row['TIMEOUT']:7}  "
            f"{row['CRASH']:5}  {row['ERROR']:5}  {row['SKIP']:4}  "
            f"{subtotal['PASS'] + subtotal['not PASS']:8}  {subtotal['not PASS']:7}  "
            f"{durations[directory] / 1000:5.1f}s"
        )
    print("-" * len(header))
    tests = sum(totals.values())
    print(
        f"{'TOTAL'.ljust(width)}  {tests:5}  {totals['PASS']:4}  {totals['FAIL']:4}  "
        f"{totals['TIMEOUT']:7}  {totals['CRASH']:5}  {totals['ERROR']:5}  {totals['SKIP']:4}  "
        f"{total_subtests['PASS'] + total_subtests['not PASS']:8}  "
        f"{total_subtests['not PASS']:7}  {total_ms / 1000:5.1f}s"
    )

    print()
    print(f"unexpected results ({len(unexpected)}):")
    for test, subtest, status in unexpected[:40]:
        label = subtest or "(test)"
        print(f"  {status:8} {test} :: {label}")
    if len(unexpected) > 40:
        print(f"  ... and {len(unexpected) - 40} more")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("paths", nargs="*", help="WPT test paths to run")
    parser.add_argument(
        "--report",
        type=Path,
        help="summarize an existing wptreport instead of running",
    )
    args, extra = parser.parse_known_args()

    if args.report:
        return summarize(args.report)
    if not args.paths:
        parser.error("give test paths or --report")

    started = time.monotonic()
    report = run(args.paths, extra)
    code = summarize(report)
    try:
        os.unlink(report)
    except OSError:
        pass
    print(f"wall time: {time.monotonic() - started:.1f}s", file=sys.stderr)
    return code


if __name__ == "__main__":
    raise SystemExit(main())
