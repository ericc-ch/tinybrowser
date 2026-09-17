#!/usr/bin/env python3
"""Re-run only the failures from a previous wptreport.

Turns the tests that need attention in REPORT into an include file and runs
`tools/wpt/run --include-file` over just those. This is the tight loop for
fixing a directory: passing tests are not re-run, so a fix can be checked in
seconds instead of re-scoring the whole directory.

    tools/wpt/retest report.json                      # rerun unexpected failures
    tools/wpt/retest report.json --include-timeout    # also rerun TIMEOUTs
    tools/wpt/retest report.json --dry-run            # list them, run nothing
    tools/wpt/retest report.json -- --processes 8 -f  # runner options

The new wptreport is kept when failures remain (printed to stderr) so retest
can chain. Exit status: 0 when nothing needs retesting or everything selected
passes, 1 when failures remain, the runner's exit code when the run itself
fails, 2 on usage or report errors.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import score  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
RUNNER = ROOT / "tools" / "wpt" / "run"
USAGE = "retest REPORT [--include-timeout] [--dry-run] [--save-report FILE] [-- runner args...]"
TIMEOUT_STATUSES = ("TIMEOUT", "EXTERNAL-TIMEOUT")


def needs_retest(result: dict, include_timeout: bool) -> bool:
    """Whether a wptreport result is a failure worth re-running.

    Mirrors score.classify_results: hard statuses, unexplained statuses, and
    mismatched subtests need attention. SKIP never does. TIMEOUT and
    EXTERNAL-TIMEOUT only count when asked, so blocked hangs do not dominate
    the loop.
    """
    status = result.get("status", "ERROR")
    if status == "SKIP":
        return False
    if status in TIMEOUT_STATUSES:
        return include_timeout
    if status in score.HARD_STATUSES:
        return True
    if not score.is_expected(result):
        return True
    return bool(score.unexpected_subs(result.get("subtests") or []))


def failing_tests(report: Path, include_timeout: bool) -> list[str]:
    data = json.loads(report.read_text())
    tests = []
    for result in data.get("results", []):
        if needs_retest(result, include_timeout) and result.get("test"):
            tests.append(result["test"])
    return tests


def parse_args(argv: list[str]) -> tuple[Path | None, bool, bool, Path | None, list[str], str | None]:
    report: Path | None = None
    include_timeout = False
    dry_run = False
    save_report: Path | None = None
    index = 0
    while index < len(argv):
        arg = argv[index]
        if arg == "--":
            if report is None:
                return None, include_timeout, dry_run, save_report, list(argv[index + 1 :]), USAGE
            return report, include_timeout, dry_run, save_report, list(argv[index + 1 :]), None
        if arg == "--include-timeout":
            include_timeout = True
        elif arg == "--dry-run":
            dry_run = True
        elif arg == "--save-report" or arg.startswith("--save-report="):
            if arg == "--save-report":
                if index + 1 >= len(argv):
                    return report, include_timeout, dry_run, None, [], "--save-report needs a file"
                save_report = Path(argv[index + 1])
                index += 1
            else:
                value = arg.split("=", 1)[1]
                if not value:
                    return report, include_timeout, dry_run, None, [], "--save-report needs a file"
                save_report = Path(value)
        elif arg.startswith("-"):
            return (
                report,
                include_timeout,
                dry_run,
                save_report,
                list(argv[index:]),
                "runner options must follow a literal `--`",
            )
        elif report is None:
            report = Path(arg)
        else:
            return report, include_timeout, dry_run, save_report, [], f"unexpected argument {arg}"
        index += 1
    if report is None:
        return None, include_timeout, dry_run, save_report, [], USAGE
    return report, include_timeout, dry_run, save_report, [], None


def run(tests: list[str], extra: list[str], save_report: Path | None) -> tuple[Path, int]:
    handle, name = tempfile.mkstemp(prefix="wpt-retest-", suffix=".txt")
    os.close(handle)
    include_file = Path(name)
    include_file.write_text("\n".join(tests) + "\n")
    if save_report is not None:
        report = save_report
        report.parent.mkdir(parents=True, exist_ok=True)
    else:
        handle, out_name = tempfile.mkstemp(prefix="wpt-retest-report-", suffix=".json")
        os.close(handle)
        report = Path(out_name)
    command = [
        str(RUNNER),
        "--include-file",
        str(include_file),
        "--log-wptreport",
        str(report),
        "--no-fail-on-unexpected",
        *extra,
    ]
    print(f"$ {' '.join(command)}", file=sys.stderr)
    try:
        exit_code = subprocess.run(command, cwd=ROOT, check=False, stdout=sys.stderr).returncode
    finally:
        try:
            os.unlink(include_file)
        except OSError:
            pass
    if exit_code != 0:
        print(f"runner exited {exit_code}", file=sys.stderr)
    return report, exit_code


def retest_status(remaining: int, runner_exit: int) -> tuple[int, bool]:
    """Exit code and whether the new report must be kept.

    A runner failure keeps the report even when no selected test needs
    retest: wptrunner writes `No tests ran` as an empty report with a
    nonzero exit, and that has to stay visible. `remaining` is computed with
    TIMEOUTs included, so a test the selection gate dropped that now hangs
    is still reported.
    """
    if runner_exit != 0:
        return runner_exit, True
    if remaining:
        return 1, True
    return 0, False


def _selftest() -> None:
    ok = {"test": "/dom/ok.html", "status": "PASS"}
    skip = {"test": "/wasm/x.any.js", "status": "SKIP", "message": "does not support jsshell"}
    timeout = {"test": "/WebCryptoAPI/x.html", "status": "TIMEOUT"}
    error = {"test": "/html/y.html", "status": "ERROR", "expected": "OK"}
    failed = {"test": "/FileAPI/z.html", "status": "FAIL", "expected": "PASS"}
    # No `expected` means mozlog saw the expected status: a baselined failure.
    baselined = {"test": "/dom/baselined.html", "status": "FAIL"}
    unpinned = {"test": "/dom/fixed.html", "status": "PASS", "expected": "FAIL"}
    sub = {
        "test": "/dom/s.html",
        "status": "OK",
        "subtests": [{"name": "a", "status": "FAIL", "expected": "PASS"}],
    }
    sub_baselined = {
        "test": "/dom/sb.html",
        "status": "OK",
        "subtests": [{"name": "a", "status": "FAIL"}],
    }
    expected = {
        "test": "/dom/e.html",
        "status": "OK",
        "subtests": [{"name": "a", "status": "PASS", "expected": "FAIL"}],
    }
    assert not needs_retest(ok, include_timeout=False)
    assert not needs_retest(skip, include_timeout=False)
    assert not needs_retest(timeout, include_timeout=False)
    assert needs_retest(timeout, include_timeout=True)
    assert needs_retest(error, include_timeout=False)
    assert needs_retest(failed, include_timeout=False)
    assert not needs_retest(baselined, include_timeout=False)
    assert needs_retest(unpinned, include_timeout=False)
    assert needs_retest(sub, include_timeout=False)
    assert not needs_retest(sub_baselined, include_timeout=False)
    assert needs_retest(expected, include_timeout=False)

    report, include_timeout, dry_run, save_report, extra, err = parse_args(
        ["r.json", "--include-timeout", "--save-report", "out.json", "--", "--processes", "8"]
    )
    assert report == Path("r.json")
    assert include_timeout and not dry_run
    assert save_report == Path("out.json")
    assert extra == ["--processes", "8"]
    assert err is None
    assert parse_args([])[5] == USAGE
    assert parse_args(["r.json", "extra"])[5] is not None
    assert parse_args(["r.json", "--processes", "8"])[5] is not None
    assert parse_args(["--", "--processes", "8"])[5] == USAGE
    assert parse_args(["r.json", "--save-report="])[5] == "--save-report needs a file"

    assert retest_status(0, 0) == (0, False)
    assert retest_status(0, 64) == (64, True)
    assert retest_status(2, 0) == (1, True)
    # A test the selection gate dropped that now hangs still keeps the report.
    assert retest_status(1, 0)[1] is True


def main() -> int:
    if sys.argv[1:] == ["--selftest"]:
        _selftest()
        return 0
    report, include_timeout, dry_run, save_report, extra, error = parse_args(sys.argv[1:])
    if error:
        print(f"retest: {error}", file=sys.stderr)
        return 2
    if report is None:
        print(f"retest: {USAGE}", file=sys.stderr)
        return 2
    try:
        tests = failing_tests(report, include_timeout)
    except OSError as error:
        print(f"retest: cannot read report {report}: {error}", file=sys.stderr)
        return 2
    except json.JSONDecodeError as error:
        print(f"retest: report {report} is not valid JSON ({error})", file=sys.stderr)
        return 2
    if not tests:
        print(f"nothing to retest in {report}", file=sys.stderr)
        return 0
    print(f"{len(tests)} tests to retest", file=sys.stderr)
    if dry_run:
        for test in tests:
            print(test)
        return 0

    started = time.monotonic()
    try:
        report_path, runner_exit = run(tests, extra, save_report)
    except OSError as error:
        print(f"retest: could not start the runner: {error}", file=sys.stderr)
        return 2
    score.summarize(report_path)
    try:
        # The report holds only the selected tests, so a TIMEOUT here is a
        # test the selection gate dropped that now hangs: new information.
        remaining = failing_tests(report_path, include_timeout=True)
    except OSError as error:
        print(f"retest: no usable report at {report_path}: {error}", file=sys.stderr)
        return runner_exit or 2
    except json.JSONDecodeError as error:
        print(f"retest: report {report_path} is not valid JSON ({error})", file=sys.stderr)
        return runner_exit or 2
    status, keep = retest_status(len(remaining), runner_exit)
    print(f"wall time: {time.monotonic() - started:.1f}s", file=sys.stderr)
    if runner_exit != 0:
        print(f"runner exited {runner_exit}; report kept at {report_path}", file=sys.stderr)
    elif remaining:
        print(f"{len(remaining)} tests still need retest; report at {report_path}", file=sys.stderr)
    else:
        print("all selected tests pass", file=sys.stderr)
    if not keep:
        try:
            os.unlink(report_path)
        except OSError:
            pass
    return status


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except BrokenPipeError:
        # `retest ... | head` closes stdout early; exit quietly.
        devnull = os.open(os.devnull, os.O_WRONLY)
        os.dup2(devnull, sys.stdout.fileno())
        raise SystemExit(0)
