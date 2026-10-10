#!/usr/bin/env python3
"""Single entry point for the tinybrowser WPT harness.

Upstream WPT (`third_party/wpt`, pristine submodule) owns serving,
scheduling, driving, and reporting. This file owns three phases around it:

* `run`    — build the binary, ready the shared venv + manifest, exec wpt.
* `score`  — run a set, then print one row per directory + attention list.
* `rerun`  — re-run only the attention set from a saved wptreport.
* `overnight` — full-suite run + score (needs explicit approval per AGENTS.md).

`tinybrowser_wpt.py` stays separate: upstream imports it as the
`wptrunner.products` entry-point, so it must remain a tiny importable module.
Everything else that was `run` (bash), `launch.py`, `score.py`, `retest.py`
lives here, sharing one argument grammar and one stamp/lock helper set.

Grammar (all subcommands): test paths and our flags come first; a literal
`--` starts verbatim upstream flags. For convenience an upstream flag before
`--` implicitly starts the verbatim region, so
`run dom/nodes/ --exclude=worker` works as well as
`run dom/nodes/ -- --exclude=worker`.
"""

from __future__ import annotations

import argparse
import fcntl
import hashlib
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CLI = Path(__file__).resolve()

# Upstream test types, `wpttest.enabled_tests` sort order. Checked against
# wptrunner at runtime; a mismatch warns instead of breaking the run.
KNOWN_TEST_TYPES = "aamtest crashtest print-reftest reftest test262 testharness wdspec"
KNOWN_TEST_TYPE_SET = set(KNOWN_TEST_TYPES.split())

# HTTP testharness only: extra schemes bind extra loopbacks or start DNS.
TLS_SCHEMES = ("https", "https-local", "https-public", "wss", "h2")
HTTP_ONLY = ("http", "ws")

# wptrunner `--exclude` is a URL prefix, so `--exclude=worker` misses
# `.any.worker.html` / `.worker.html` variants generated from `.any.js`.
WORKER_VARIANT = re.compile(r"\.(any\.)?(sharedworker|serviceworker|worker)(-module)?\.html$")

# File-level statuses with their own bucket; anything else not-a-pass is
# classified by the report's `expected` field.
HARD_STATUSES = ("TIMEOUT", "CRASH", "ERROR")
TIMEOUT_STATUSES = ("TIMEOUT", "EXTERNAL-TIMEOUT")
MAX_LISTED = 40


def eprint(*parts: object) -> None:
    print(*parts, file=sys.stderr)


def die(prefix: str, message: str, code: int = 1) -> "int":
    eprint(f"{prefix}: {message}")
    return code


def sha16(text: str) -> str:
    return hashlib.sha256(text.encode()).hexdigest()[:16]


def sha_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def cache_dir() -> Path:
    base = Path(os.environ.get("XDG_CACHE_HOME") or Path.home() / ".cache")
    out = base / "tinybrowser"
    out.mkdir(parents=True, exist_ok=True)
    return out


def git(args: list[str], cwd: Path | None = None) -> str | None:
    try:
        out = subprocess.run(
            ["git", *args], cwd=str(cwd or ROOT),
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, check=False,
        )
    except OSError:
        return None
    if out.returncode != 0:
        return None
    return out.stdout.decode(errors="replace").strip()


# --- worker-variant filter (pure; selftested) ---

def exclude_skips_worker_variants(exclude: object) -> bool:
    return bool(exclude and any(item.strip("/") == "worker" for item in exclude))


def is_worker_variant_url(url: str) -> bool:
    return bool(WORKER_VARIANT.search(url.split("?", 1)[0]))


# --- option matching (mirrors wpt's allow_abbrev) ---

def has_option(option: str, args: list[str]) -> bool:
    for arg in args:
        if arg == option or arg.startswith(option + "="):
            return True
        if option.startswith("--") and arg.startswith("--") and "=" not in arg and len(arg) >= 4 and option.startswith(arg):
            return True
    return False


def has_option_exact(option: str, args: list[str]) -> bool:
    return any(arg == option or arg.startswith(option + "=") for arg in args)


def opt_value(args: list[str], option: str) -> str | None:
    for i, arg in enumerate(args):
        if arg.startswith(option + "="):
            return arg.split("=", 1)[1]
        if arg == option and i + 1 < len(args):
            return args[i + 1]
    return None


# --- WPT checkout ---

def resolve_wpt_root() -> tuple[Path | None, str | None]:
    override = os.environ.get("TINYBROWSER_WPT_ROOT")
    if override:
        return Path(override), None
    status = git(["submodule", "status", "--", "third_party/wpt"])
    if status is not None and status != "" and not status.startswith("-"):
        return ROOT / "third_party" / "wpt", None
    worktrees = git(["worktree", "list", "--porcelain"]) or ""
    primary = None
    for line in worktrees.splitlines():
        if line.startswith("worktree "):
            primary = line.split(" ", 1)[1]
            break
    base = Path(primary) if primary else ROOT
    return base / "third_party" / "wpt", None


def check_pin(wpt: Path) -> int:
    pinned = git(["rev-parse", "HEAD:third_party/wpt"])
    actual = git(["rev-parse", "HEAD"], cwd=wpt)
    allow = os.environ.get("TINYBROWSER_WPT_ALLOW_PIN_MISMATCH", "0") == "1"
    if pinned and not allow:
        if not actual:
            eprint(f"cannot read the WPT revision at {wpt}; it is not a git checkout")
            eprint("Set TINYBROWSER_WPT_ALLOW_PIN_MISMATCH=1 to run it anyway.")
            return 1
        if pinned != actual:
            eprint(f"WPT checkout at {wpt} is {actual}; this worktree pins {pinned}")
            eprint("Update the checkout, or set TINYBROWSER_WPT_ALLOW_PIN_MISMATCH=1.")
            return 1
    elif pinned and pinned != (actual or ""):
        eprint(f"WPT pin mismatch ignored: running {actual or 'an unknown revision'}, worktree pins {pinned}")
    return 0


# --- locks (shared venv + shared manifest) ---

def acquire_lock(name: str, timeout_s: int = 1800):
    if os.environ.get("TINYBROWSER_WPT_NO_LOCK", "0") == "1":
        return None
    path = cache_dir() / name
    handle = open(path, "a+b")
    try:
        fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        return handle
    except BlockingIOError:
        pass
    raw = os.environ.get("TINYBROWSER_WPT_LOCK_TIMEOUT", "")
    try:
        timeout_s = int(raw) if raw else timeout_s
    except ValueError:
        timeout_s = 1800
    eprint(f"another WPT install holds {path}; waiting")
    deadline = time.monotonic() + timeout_s
    while True:
        try:
            fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            return handle
        except BlockingIOError:
            if time.monotonic() >= deadline:
                eprint("timed out waiting for the WPT venv lock")
                handle.close()
                raise SystemExit(1)
            time.sleep(0.2)


def release_lock(handle) -> None:
    if handle is not None:
        try:
            handle.close()
        except OSError:
            pass


# --- venv (upstream requirements + editable adapter) ---

def venv_python(venv: Path) -> Path:
    return venv / "bin" / "python"


def venv_stamp_payload(wpt: Path, venv: Path) -> str:
    lines = [f"root {ROOT}"]
    exe = venv_python(venv)
    if exe.is_file() and os.access(exe, os.X_OK):
        try:
            out = subprocess.run(
                [str(exe), "-c", "import sys; print(sys.version)"],
                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, check=False,
            )
            lines.append(out.stdout.decode(errors="replace").strip() or "no-python")
        except OSError:
            lines.append("no-python")
    else:
        lines.append("no-python")
    for rel in ("tools/manifest/requirements.txt", "tools/wptrunner/requirements.txt"):
        lines.append(sha_file(wpt / rel))
    lines.append(sha_file(ROOT / "tools" / "wpt" / "pyproject.toml"))
    return hashlib.sha256("\n".join(lines).encode()).hexdigest()


def venv_is_current(wpt: Path, venv: Path) -> bool:
    stamp = venv / ".tinybrowser-wpt-stamp"
    exe = venv_python(venv)
    if not (exe.is_file() and os.access(exe, os.X_OK) and stamp.is_file()):
        return False
    try:
        return venv_stamp_payload(wpt, venv) == stamp.read_text().strip()
    except OSError:
        return False


def install_venv(wpt: Path, venv: Path) -> None:
    if shutil.which("uv") is None:
        eprint("uv is required to bootstrap the WPT virtualenv")
        raise SystemExit(1)
    exe = venv_python(venv)
    if not (exe.is_file() and os.access(exe, os.X_OK)):
        if os.environ.get("TINYBROWSER_WPT_VENV"):
            eprint(f"WPT virtualenv not found at {venv}; TINYBROWSER_WPT_VENV must name one")
            raise SystemExit(1)
        subprocess.run(["uv", "venv", "--seed", str(venv), "--python", "3.12"], check=True)
    # uv owns the venv; --seed keeps pip/setuptools so WPT's working-set
    # check finds everything satisfied and never falls back to its pip path.
    subprocess.run(
        ["uv", "pip", "install", "--python", str(exe), "-q",
         "-r", str(wpt / "tools/manifest/requirements.txt"),
         "-r", str(wpt / "tools/wptrunner/requirements.txt")],
        check=True,
    )
    subprocess.run(
        ["uv", "pip", "install", "--python", str(exe), "-q", "-e", str(ROOT / "tools" / "wpt")],
        check=True,
    )
    (venv / ".tinybrowser-wpt-stamp").write_text(venv_stamp_payload(wpt, venv) + "\n")


def ensure_venv(wpt: Path, venv: Path) -> None:
    if venv_is_current(wpt, venv):
        eprint("wpt: venv is current, skipping install")
        return
    lock = acquire_lock(f"wpt-{sha16(str(venv.resolve()))}.lock")
    try:
        if venv_is_current(wpt, venv):
            eprint("wpt: venv is current, skipping install")
        else:
            eprint("wpt: installing WPT virtualenv")
            install_venv(wpt, venv)
    finally:
        release_lock(lock)


# --- manifest (whole-tree inventory; skip the walk when unchanged) ---

def manifest_token(wpt: Path, manifest: Path) -> str:
    parts: list[str] = []
    rev = git(["rev-parse", "HEAD"], cwd=wpt) or "no-git"
    parts.append(f"rev {rev}")
    status = git(["status", "--porcelain=v1", "--untracked-files=all"], cwd=wpt)
    parts.append(status if status is not None else "no-git-status")

    def hashed(paths: list[str]) -> list[str]:
        out: list[str] = []
        for rel in paths:
            try:
                out.append(f"{sha_file(wpt / rel)}  {rel}")
            except OSError:
                pass
        return out

    try:
        proc = subprocess.run(
            ["git", "diff", "HEAD", "--name-only", "-z"], cwd=str(wpt),
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, check=False,
        )
        names = [p for p in proc.stdout.decode(errors="replace").split("\0") if p]
        parts.extend(hashed(names))
    except OSError:
        pass
    try:
        proc = subprocess.run(
            ["git", "ls-files", "--others", "--exclude-standard", "-z"], cwd=str(wpt),
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, check=False,
        )
        names = [p for p in proc.stdout.decode(errors="replace").split("\0") if p]
        parts.extend(hashed(names))
    except OSError:
        pass
    try:
        parts.append(f"{sha_file(manifest)}  {manifest}")
    except OSError:
        parts.append("missing")
    return hashlib.sha256("\n".join(parts).encode()).hexdigest()


def manifest_stamp_file(manifest: Path) -> Path:
    try:
        key = sha16(str(manifest.resolve()))
    except OSError:
        key = sha16(str(manifest))
    return cache_dir() / f"wpt-manifest-{key}.stamp"


def manifest_is_current(wpt: Path, manifest: Path) -> bool:
    if not manifest.is_file() or git(["rev-parse", "HEAD"], cwd=wpt) is None:
        return False
    stamp = manifest_stamp_file(manifest)
    if not stamp.is_file():
        return False
    try:
        return manifest_token(wpt, manifest) == stamp.read_text().strip()
    except OSError:
        return False


def write_manifest_stamp(wpt: Path, manifest: Path) -> None:
    if git(["rev-parse", "HEAD"], cwd=wpt) is None:
        return
    stamp = manifest_stamp_file(manifest)
    try:
        stamp.write_text(manifest_token(wpt, manifest) + "\n")
    except OSError:
        pass


def manifest_parses(exe: Path, manifest: Path) -> bool:
    try:
        out = subprocess.run(
            [str(exe), "-c", "import json,sys; json.load(open(sys.argv[1]))", str(manifest)],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False,
        )
        return out.returncode == 0
    except OSError:
        return False


# --- argument grammars ---

def split_run_argv(argv: list[str]) -> tuple[list[str], bool, bool, list[str], str | None]:
    """paths + our flags vs verbatim upstream extras.

    Our flags: --score, --dry-run. Anything else starting with `-` (or the
    literal `--`) starts the verbatim region so upstream argparse sees it
    untouched. `--test-types` values stay with it; a following positional
    stays a path, so the nargs='*' swallow can never trigger.
    """
    paths: list[str] = []
    extras: list[str] = []
    score = False
    dry_run = False
    i = 0
    while i < len(argv):
        arg = argv[i]
        if arg == "--":
            extras.extend(argv[i + 1:])
            break
        if arg == "--score":
            score = True
        elif arg == "--dry-run":
            dry_run = True
        elif arg == "--test-types" or arg.startswith("--test-types="):
            values = [arg]
            if arg == "--test-types":
                j = i + 1
                while j < len(argv) and argv[j] in KNOWN_TEST_TYPE_SET:
                    values.append(argv[j])
                    j += 1
                if j < len(argv) and not argv[j].startswith("-") and "/" not in argv[j] and "." not in argv[j]:
                    return [], False, False, [], (
                        f"unknown --test-types value '{argv[j]}'; expected one of: {KNOWN_TEST_TYPES}"
                    )
                i = j - 1
            extras.extend(values)
        elif arg.startswith("-") and arg != "-":
            extras.extend(argv[i:])
            break
        else:
            paths.append(arg)
        i += 1
    return paths, score, dry_run, extras, None


def split_score_argv(argv: list[str]) -> tuple[Path | None, Path | None, list[str], list[str], str | None]:
    report: Path | None = None
    save_report: Path | None = None
    paths: list[str] = []
    extra: list[str] = []
    i = 0
    while i < len(argv):
        arg = argv[i]
        if arg == "--":
            extra = list(argv[i + 1:])
            break
        if arg in ("--report", "--save-report") or arg.startswith(("--report=", "--save-report=")):
            option, value = arg.split("=", 1) if "=" in arg else (arg, None)
            if value is None:
                if i + 1 >= len(argv):
                    return report, save_report, paths, [], f"{option} needs a file"
                value = argv[i + 1]
                i += 1
            if not value:
                return report, save_report, paths, [], f"{option} needs a file"
            if option == "--report":
                report = Path(value)
            else:
                save_report = Path(value)
        elif arg == "--dry-run":
            extra.append(arg)
        elif arg.startswith("-"):
            extra.extend(argv[i:])
            break
        else:
            paths.append(arg)
        i += 1
    return report, save_report, paths, extra, None


def parse_rerun_argv(argv: list[str]) -> tuple[Path | None, bool, bool, Path | None, list[str], str | None]:
    usage = "rerun REPORT [--include-timeout] [--dry-run] [--save-report FILE] [-- runner args...]"
    report: Path | None = None
    include_timeout = False
    dry_run = False
    save_report: Path | None = None
    i = 0
    while i < len(argv):
        arg = argv[i]
        if arg == "--":
            if report is None:
                return None, include_timeout, dry_run, save_report, list(argv[i + 1:]), usage
            return report, include_timeout, dry_run, save_report, list(argv[i + 1:]), None
        if arg == "--include-timeout":
            include_timeout = True
        elif arg == "--dry-run":
            dry_run = True
        elif arg == "--save-report" or arg.startswith("--save-report="):
            if arg == "--save-report":
                if i + 1 >= len(argv):
                    return report, include_timeout, dry_run, None, [], "--save-report needs a file"
                save_report = Path(argv[i + 1])
                i += 1
            else:
                value = arg.split("=", 1)[1]
                if not value:
                    return report, include_timeout, dry_run, None, [], "--save-report needs a file"
                save_report = Path(value)
        elif arg.startswith("-"):
            return report, include_timeout, dry_run, save_report, list(argv[i:]), "runner options must follow a literal `--`"
        elif report is None:
            report = Path(arg)
        else:
            return report, include_timeout, dry_run, save_report, [], f"unexpected argument {arg}"
        i += 1
    if report is None:
        return None, include_timeout, dry_run, save_report, [], usage
    return report, include_timeout, dry_run, save_report, [], None


# --- report primitives (score + rerun share these) ---

def group_of(test: str) -> str:
    parts = [p for p in test.split("/") if p]
    if len(parts) <= 1:
        return "(root)"
    return "/".join(parts[:2])


def is_expected(entry: dict) -> bool:
    if entry.get("expected") is None:
        return True
    return entry.get("status") in (entry.get("known_intermittent") or [])


def unexpected_subs(subtests: list) -> list[dict]:
    return [s for s in subtests if s.get("status") != "SKIP" and not is_expected(s)]


def classify_results(results: list[dict]) -> tuple[dict[str, dict[str, int]], list[tuple[str, str, str]]]:
    rows: dict[str, dict[str, int]] = defaultdict(lambda: defaultdict(int))
    attention: list[tuple[str, str, str]] = []
    for result in results:
        test = result.get("test", "?")
        row = rows[group_of(test)]
        row["tests"] += 1
        row["time_ms"] += result.get("duration") or 0
        subtests = result.get("subtests") or []
        mismatched = unexpected_subs(subtests)
        sub_failed = [s for s in subtests if s.get("status") not in ("PASS", "SKIP")]
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
            for sub in mismatched:
                attention.append((test, sub.get("name", "?"), sub.get("status", "ERROR")))
        elif sub_failed:
            row["expected_fail"] += 1
        elif status in ("PASS", "OK"):
            row["pass"] += 1
        else:
            row["expected_fail"] += 1
    return rows, attention


def summarize(report: Path) -> int:
    try:
        data = json.loads(report.read_text())
    except OSError as error:
        eprint(f"cannot read report {report}: {error}")
        return 1
    except json.JSONDecodeError as error:
        eprint(f"report {report} is not valid JSON ({error}); the runner probably failed before writing results")
        return 1
    results = data.get("results", [])
    if not results:
        eprint(f"no results in report {report}")
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
        print(f"  {status:10} {test} :: {subtest or '(test)'}")
    if len(attention) > MAX_LISTED:
        print(f"  ... and {len(attention) - MAX_LISTED} more")
    return 0


def needs_retest(result: dict, include_timeout: bool) -> bool:
    status = result.get("status", "ERROR")
    if status == "SKIP":
        return False
    if status in TIMEOUT_STATUSES:
        return include_timeout
    if status in HARD_STATUSES:
        return True
    if not is_expected(result):
        return True
    return bool(unexpected_subs(result.get("subtests") or []))


def failing_tests(report: Path, include_timeout: bool) -> list[str]:
    data = json.loads(report.read_text())
    return [r["test"] for r in data.get("results", []) if r.get("test") and needs_retest(r, include_timeout)]


def rerun_status(remaining: int, runner_exit: int) -> tuple[int, bool]:
    if runner_exit != 0:
        return runner_exit, True
    if remaining:
        return 1, True
    return 0, False


# --- in-venv runner (was launch.py) ---

def patch_wpt(wpt_root: Path) -> None:
    sys.path.insert(0, str(wpt_root))
    from tools import localpaths  # noqa: F401
    from tools.wpt import run as wpt_run
    from wptserve import sslutils
    from wptserve.config import ConfigBuilder

    check_environ = wpt_run.check_environ

    def skip_tinybrowser_hosts(product: str) -> None:
        if product == "tinybrowser":
            return
        check_environ(product)

    wpt_run.check_environ = skip_tinybrowser_hosts

    get_ports = ConfigBuilder._get_ports

    def skip_tls_ports_without_ssl(self, data):
        ports = get_ports(self, data)
        ssl_type = data.get("ssl", {}).get("type", "none")
        if not sslutils.get_cls(ssl_type).ssl_enabled:
            for scheme in TLS_SCHEMES:
                ports.pop(scheme, None)
            for scheme in list(ports):
                if scheme not in HTTP_ONLY:
                    ports.pop(scheme, None)
        return ports

    ConfigBuilder._get_ports = skip_tls_ports_without_ssl

    from wptrunner import testloader

    original_filter = testloader.TestFilter

    class TinybrowserTestFilter(original_filter):
        def __init__(self, *args, **kwargs):
            exclude = kwargs.get("exclude")
            if exclude is None and len(args) >= 3:
                exclude = args[2]
            self._skip_worker_variants = exclude_skips_worker_variants(exclude)
            super().__init__(*args, **kwargs)

        def __call__(self, manifest_iter):
            for test_type, test_path, tests in super().__call__(manifest_iter):
                if self._skip_worker_variants:
                    tests = {t for t in tests if not is_worker_variant_url(t.url)}
                    if not tests:
                        continue
                yield test_type, test_path, tests

    testloader.TestFilter = TinybrowserTestFilter


def inner_main(argv: list[str]) -> int:
    """Runs under the venv python: patch upstream, then `wpt run`."""
    wpt_root = Path(os.environ.get("TINYBROWSER_WPT_ROOT") or ROOT / "third_party" / "wpt")
    patch_wpt(wpt_root)
    from tools.wpt import wpt

    code = wpt.main(prog=str(wpt_root / "wpt"), argv=["run", *argv])
    return code if isinstance(code, int) else 0


# --- `run` (setup, then exec or spawn the venv python) ---

def build_binary() -> Path:
    override = os.environ.get("TINYBROWSER_BINARY")
    if override:
        eprint(f"wpt: using prebuilt TINYBROWSER_BINARY without a freshness check: {override}")
        return Path(override)
    out = subprocess.run(
        [str(ROOT / "tools" / "binary-path")],
        cwd=str(ROOT), stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, check=False,
    )
    binary = Path(out.stdout.decode(errors="replace").strip())
    eprint("wpt: building tinybrowser (cargo no-ops when fresh)")
    # cargo's fingerprint tracks every compiled input; a failure means the
    # binary would be stale, so refuse instead of running against it.
    ret = subprocess.run(["cargo", "build", "-q", "--offline"], cwd=str(ROOT)).returncode
    if ret != 0:
        ret = subprocess.run(["cargo", "build", "-q"], cwd=str(ROOT)).returncode
    if ret != 0:
        eprint("wpt: cargo build failed; refusing to run against a stale binary")
        raise SystemExit(1)
    return binary


def finalize_run_args(extras: list[str]) -> list[str]:
    args = list(extras)
    if not has_option("--metadata", args):
        args = ["--metadata", str(ROOT / "tools" / "wpt" / "metadata"), *args]
    if not has_option("--pause-after-test", args) and not has_option("--no-pause-after-test", args):
        args = ["--no-pause-after-test", *args]
    if not has_option("--no-restart-on-unexpected", args) and not has_option("--restart-on-unexpected", args):
        args = ["--no-restart-on-unexpected", *args]
    if not has_option("--ssl-type", args):
        args = ["--ssl-type=openssl", *args]
    if not has_option("--processes", args) and not has_option("--fully-parallel", args) and not has_option("-f", args):
        # Uncapped: one browser per core. Explicit --processes still wins.
        procs = os.cpu_count() or 1
        args.extend(["--processes", str(procs)])
    if not has_option("--test-types", args):
        # Last: --test-types is nargs='*', so nothing positional may follow it.
        # Paths already lead, so appending here is safe.
        args.extend(["--test-types", "testharness", "crashtest"])
    return args


def check_type_drift(exe: Path) -> None:
    try:
        out = subprocess.run(
            [str(exe), "-c",
             "import sys; sys.path.insert(0, 'tools/wptrunner'); "
             "from wptrunner import wpttest; print(' '.join(sorted(wpttest.enabled_tests)))"],
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, check=False,
        )
        actual = out.stdout.decode(errors="replace").strip()
    except OSError:
        return
    if actual and actual != KNOWN_TEST_TYPES:
        eprint(f"wpt: KNOWN_TEST_TYPES drifts from wpttest.enabled_tests ({actual}); update tools/wpt/cli.py")


def run_wpt(wpt: Path, exe: Path, binary: Path, final_args: list[str], manifest: Path, record: int) -> int:
    inner_argv = ["--binary", str(binary), "tinybrowser", *final_args]
    manifest_lock = None
    if not has_option_exact("--no-manifest-update", final_args):
        lock_name = f"wpt-manifest-{sha16(str(manifest.resolve()))}.lock"
        manifest_lock = acquire_lock(lock_name)
    manifest_locked = manifest_lock is not None
    venv_lock = None
    if (
        has_option("--install-browser", final_args)
        or has_option("--install-webdriver", final_args)
        or has_option_exact("--manifest-download", final_args)
        or has_option("--install-fonts", final_args)
    ):
        venv_lock = acquire_lock(f"wpt-{sha16(str((wpt / '_venv3').resolve()))}.lock")

    for i, arg in enumerate(final_args):
        if arg == "--processes" and i + 1 < len(final_args):
            eprint(f"wpt: processes {final_args[i + 1]}")
            break
        if arg.startswith("--processes="):
            eprint(f"wpt: processes {arg.split('=', 1)[1]}")
            break

    if record == 0 and manifest_locked is False and venv_lock is None:
        # Fast path: nothing to record or serialize. exec restores direct
        # signal delivery to the runner instead of stranding browsers behind us.
        release_lock(manifest_lock)
        os.execv(str(exe), [str(exe), str(CLI), "_inner", *inner_argv])
        raise AssertionError("unreachable")

    # Slow path: a manifest walk and/or venv writes happen under us, so stay
    # parent and forward signals explicitly. Children must not inherit the
    # lock fds (the lock belongs to the open file description; one orphaned
    # browser would block every later run).
    try:
        manifest_mtime_before = manifest.stat().st_mtime_ns
    except OSError:
        manifest_mtime_before = 0
    child = subprocess.Popen(
        [str(exe), str(CLI), "_inner", *inner_argv], cwd=str(wpt), close_fds=True,
    )
    interrupted = False

    def forward(signum, _frame):
        nonlocal interrupted
        interrupted = True
        try:
            child.send_signal(signum)
        except (OSError, ValueError):
            pass

    old = {s: signal.signal(s, forward) for s in (signal.SIGTERM, signal.SIGHUP)}
    old_int = signal.signal(signal.SIGINT, forward)
    try:
        status = child.wait()
        while child.poll() is None:
            status = child.wait()
    finally:
        for s, handler in old.items():
            signal.signal(s, handler)
        signal.signal(signal.SIGINT, old_int)
    try:
        manifest_mtime_after = manifest.stat().st_mtime_ns
    except OSError:
        manifest_mtime_after = 0
    if (
        interrupted is False
        and record == 1
        and manifest_mtime_after != manifest_mtime_before
        and manifest_locked
        and manifest_parses(exe, manifest)
    ):
        write_manifest_stamp(wpt, manifest)
    if venv_lock is not None:
        release_lock(venv_lock)
    release_lock(manifest_lock)
    return status


def run_main(argv: list[str]) -> int:
    paths, score, dry_run, extras, error = split_run_argv(argv)
    if error:
        return die("run", error, 2)
    if score:
        return score_main([*paths, *extras])
    if dry_run:
        wpt, _err = resolve_wpt_root()
        if wpt is None:
            return die("run", "no WPT checkout", 2)
        final = finalize_run_args(extras)
        print(f"$ {wpt}/wpt run --binary <tinybrowser> tinybrowser {' '.join([*paths, *final])}")
        return 0

    wpt, _err = resolve_wpt_root()
    if wpt is None or not (wpt / "wpt").is_file():
        eprint(f"WPT checkout not found at {wpt}")
        eprint(f"Initialize it with: git submodule update --init third_party/wpt")
        eprint("or point TINYBROWSER_WPT_ROOT at an initialized checkout.")
        return 1
    wpt = Path(os.path.realpath(wpt))
    os.environ["TINYBROWSER_WPT_ROOT"] = str(wpt)
    if check_pin(wpt) != 0:
        return 1

    os.chdir(ROOT)
    binary = build_binary()
    if not (binary.is_file() and os.access(binary, os.X_OK)):
        return die("run", f"tinybrowser binary not found at {binary}")
    binary = Path(os.path.realpath(binary))
    os.chdir(wpt)

    venv = Path(os.environ.get("TINYBROWSER_WPT_VENV") or wpt / "_venv3")
    eprint(f"wpt: {wpt}")
    eprint(f"venv: {venv}")
    eprint(f"binary: {binary}")
    ensure_venv(wpt, venv)
    exe = venv_python(venv)

    final = finalize_run_args(extras)
    check_type_drift(exe)

    manifest = Path(opt_value(final, "--manifest") or wpt / "MANIFEST.json")
    if has_option_exact("--no-manifest-update", final):
        record = 0
    elif has_option_exact("--manifest-update", final):
        record = 1
    elif manifest_is_current(wpt, manifest):
        eprint("wpt: manifest is current, skipping update")
        final = ["--no-manifest-update", *final]
        record = 1
    else:
        record = 1
    return run_wpt(wpt, exe, binary, [*paths, *final], manifest, record)


# --- `score` ---

def score_run(paths: list[str], extra: list[str], save_report: Path | None) -> tuple[Path, int]:
    if save_report is not None:
        report = save_report
        report.parent.mkdir(parents=True, exist_ok=True)
    else:
        handle, name = tempfile.mkstemp(prefix="wpt-report-", suffix=".json")
        os.close(handle)
        report = Path(name)
    command = [sys.executable, str(CLI), "run", *paths, "--log-wptreport", str(report), "--no-fail-on-unexpected", *extra]
    eprint(f"$ {' '.join(command)}")
    exit_code = subprocess.run(command, cwd=str(ROOT), stdout=sys.stderr).returncode
    if exit_code != 0:
        eprint(f"runner exited {exit_code}")
    return report, exit_code


def score_main(argv: list[str]) -> int:
    report, save_report, paths, extra, error = split_score_argv(argv)
    if error:
        return die("run --score", error, 2)
    if report is not None and save_report is not None:
        return die("run --score", "--report summarizes an existing report; do not combine it with --save-report", 2)
    if report is not None:
        if paths or extra:
            return die("run --score", "--report summarizes an existing report; give no test paths", 2)
        return summarize(report)
    if "--dry-run" in extra:
        extra = [a for a in extra if a != "--dry-run"]
        eprint(f"$ {CLI} run {' '.join([*paths, '--log-wptreport', '<tmp>', '--no-fail-on-unexpected', *extra])}")
        return 0
    if not paths:
        return die("run --score", "give test paths or --report FILE", 2)
    started = time.monotonic()
    try:
        report_path, runner_exit = score_run(paths, extra, save_report)
    except OSError as error:
        return die("run --score", f"could not start the runner: {error}")
    keep_report = runner_exit != 0 or save_report is not None
    try:
        if runner_exit != 0:
            eprint(f"runner failed with exit {runner_exit}; report kept at {report_path}")
            summarize(report_path)
            return runner_exit
        if keep_report:
            eprint(f"report kept at {report_path}")
        return summarize(report_path)
    finally:
        if not keep_report:
            try:
                os.unlink(report_path)
            except OSError:
                pass
        eprint(f"wall time: {time.monotonic() - started:.1f}s")


# --- `rerun` (was retest) ---

def rerun_spawn(tests: list[str], extra: list[str], save_report: Path | None) -> tuple[Path, int]:
    handle, name = tempfile.mkstemp(prefix="wpt-rerun-", suffix=".txt")
    os.close(handle)
    include_file = Path(name)
    include_file.write_text("\n".join(tests) + "\n")
    if save_report is not None:
        report = save_report
        report.parent.mkdir(parents=True, exist_ok=True)
    else:
        handle, out_name = tempfile.mkstemp(prefix="wpt-rerun-report-", suffix=".json")
        os.close(handle)
        report = Path(out_name)
    command = [
        sys.executable, str(CLI), "run",
        "--include-file", str(include_file),
        "--log-wptreport", str(report),
        "--no-fail-on-unexpected", *extra,
    ]
    eprint(f"$ {' '.join(command)}")
    try:
        exit_code = subprocess.run(command, cwd=str(ROOT), stdout=sys.stderr).returncode
    finally:
        try:
            os.unlink(include_file)
        except OSError:
            pass
    if exit_code != 0:
        eprint(f"runner exited {exit_code}")
    return report, exit_code


def rerun_main(argv: list[str]) -> int:
    report, include_timeout, dry_run, save_report, extra, error = parse_rerun_argv(argv)
    if error:
        return die("rerun", error, 2)
    assert report is not None
    try:
        tests = failing_tests(report, include_timeout)
    except OSError as error:
        return die("rerun", f"cannot read report {report}: {error}", 2)
    except json.JSONDecodeError as error:
        return die("rerun", f"report {report} is not valid JSON ({error})", 2)
    if not tests:
        eprint(f"nothing to rerun in {report}")
        return 0
    eprint(f"{len(tests)} tests to rerun")
    if dry_run:
        for test in tests:
            print(test)
        return 0
    started = time.monotonic()
    try:
        report_path, runner_exit = rerun_spawn(tests, extra, save_report)
    except OSError as error:
        return die("rerun", f"could not start the runner: {error}", 2)
    summarize(report_path)
    try:
        # The report holds only the selected tests, so a TIMEOUT here is a
        # test the selection gate dropped that now hangs: new information.
        remaining = failing_tests(report_path, include_timeout=True)
    except OSError as error:
        eprint(f"rerun: no usable report at {report_path}: {error}")
        return runner_exit or 2
    except json.JSONDecodeError as error:
        eprint(f"rerun: report {report_path} is not valid JSON ({error})")
        return runner_exit or 2
    status, keep = rerun_status(len(remaining), runner_exit)
    eprint(f"wall time: {time.monotonic() - started:.1f}s")
    if runner_exit != 0:
        eprint(f"runner exited {runner_exit}; report kept at {report_path}")
    elif remaining:
        eprint(f"{len(remaining)} tests still need rerun; report at {report_path}")
    else:
        eprint("all selected tests pass")
    if not keep:
        try:
            os.unlink(report_path)
        except OSError:
            pass
    return status


# --- `overnight` (full-suite run + score; needs approval per AGENTS.md) ---

def overnight_main(argv: list[str]) -> int:
    save_report: Path | None = None
    extra: list[str] = []
    dry_run = False
    i = 0
    rest = list(argv)
    while i < len(rest):
        arg = rest[i]
        if arg == "--":
            extra = rest[i + 1:]
            break
        if arg == "--save-report" or arg.startswith("--save-report="):
            option, value = arg.split("=", 1) if "=" in arg else (arg, None)
            if value is None:
                if i + 1 >= len(rest):
                    return die("overnight", "--save-report needs a file", 2)
                value = rest[i + 1]
                i += 1
            if not value:
                return die("overnight", "--save-report needs a file", 2)
            save_report = Path(value)
        elif arg == "--dry-run":
            dry_run = True
        elif arg.startswith("-"):
            extra.extend(rest[i:])
            break
        else:
            return die("overnight", f"unexpected argument {arg}", 2)
        i += 1
    if dry_run:
        eprint(f"$ {CLI} run --log-wptreport <report> --no-fail-on-unexpected {' '.join(extra)}")
        return 0
    started = time.monotonic()
    try:
        report_path, runner_exit = score_run([], extra, save_report)
    except OSError as error:
        return die("overnight", f"could not start the runner: {error}")
    keep_report = runner_exit != 0 or save_report is not None
    try:
        if runner_exit != 0:
            eprint(f"runner failed with exit {runner_exit}; report kept at {report_path}")
            summarize(report_path)
            return runner_exit
        if keep_report:
            eprint(f"report kept at {report_path}")
        return summarize(report_path)
    finally:
        if not keep_report:
            try:
                os.unlink(report_path)
            except OSError:
                pass
        eprint(f"wall time: {time.monotonic() - started:.1f}s")


# --- selftest ---

def _selftest() -> None:
    assert exclude_skips_worker_variants(["worker"])
    assert exclude_skips_worker_variants(["/worker/"])
    assert not exclude_skips_worker_variants(["workers"])
    assert not exclude_skips_worker_variants(None)
    assert is_worker_variant_url("/dom/events/Event-isTrusted.any.worker.html")
    assert is_worker_variant_url("/dom/events/Event-isTrusted.any.worker.html?foo=1")
    assert is_worker_variant_url("/x.worker.html")
    assert is_worker_variant_url("/x.any.sharedworker.html")
    assert is_worker_variant_url("/x.any.serviceworker.html")
    assert is_worker_variant_url("/x.any.worker-module.html")
    assert not is_worker_variant_url("/dom/events/Event-isTrusted.any.html")
    assert not is_worker_variant_url("/dom/events/Event-isTrusted.any.html?foo=1")
    assert not is_worker_variant_url("/dom/events/Event-dispatch-click.tentative.html")

    assert has_option("--processes", ["--processes", "4"])
    assert has_option("--processes", ["--processes=4"])
    assert has_option("--processes", ["--proc", "4"])
    assert not has_option("--processes", ["--pause-after-test"])
    assert has_option_exact("--manifest", ["--manifest", "x"])
    assert not has_option_exact("--manifest", ["--manif", "x"])

    paths, score, dry, extras, err = split_run_argv(["dom/nodes/", "--exclude=worker"])
    assert err is None and paths == ["dom/nodes/"] and extras == ["--exclude=worker"] and not score and not dry
    paths, score, dry, extras, err = split_run_argv(["--score", "dom/", "--", "--processes", "4"])
    assert err is None and score and paths == ["dom/"] and extras == ["--processes", "4"]
    paths, score, dry, extras, err = split_run_argv(
        ["--test-types", "reftest", "css/", "--", "--processes", "2"]
    )
    assert err is None and paths == ["css/"] and extras == ["--test-types", "reftest", "--processes", "2"]
    _, _, _, _, err = split_run_argv(["--test-types", "testharnes", "dom/"])
    assert err is not None

    rows, attention = classify_results(
        [{"test": "/html/foo.html", "status": "PASS", "expected": "FAIL", "subtests": [], "duration": 0}]
    )
    assert rows["html/foo.html"]["unexpected"] == 1 and attention == [("/html/foo.html", "", "PASS")]
    rows, attention = classify_results(
        [{"test": "/dom/ok.html", "status": "OK",
          "subtests": [{"name": "a", "status": "PASS"}], "duration": 0}]
    )
    assert rows["dom/ok.html"]["pass"] == 1 and attention == []

    ok = {"test": "/dom/ok.html", "status": "PASS"}
    skip = {"test": "/wasm/x.any.js", "status": "SKIP"}
    timeout = {"test": "/WebCryptoAPI/x.html", "status": "TIMEOUT"}
    error = {"test": "/html/y.html", "status": "ERROR", "expected": "OK"}
    baselined = {"test": "/dom/baselined.html", "status": "FAIL"}
    assert not needs_retest(ok, include_timeout=False)
    assert not needs_retest(skip, include_timeout=False)
    assert not needs_retest(timeout, include_timeout=False)
    assert needs_retest(timeout, include_timeout=True)
    assert needs_retest(error, include_timeout=False)
    assert not needs_retest(baselined, include_timeout=False)
    assert rerun_status(0, 0) == (0, False)
    assert rerun_status(0, 64) == (64, True)
    assert rerun_status(2, 0) == (1, True)

    report, save, paths, extra, err = split_score_argv(["FileAPI/", "--save-report", "/tmp/x.json", "--", "--exclude=worker"])
    assert report is None and save == Path("/tmp/x.json") and paths == ["FileAPI/"] and extra == ["--exclude=worker"] and err is None
    r, _, _, _, _, err = parse_rerun_argv(["r.json", "--include-timeout", "--", "--processes", "8"])
    assert r == Path("r.json") and err is None
    assert parse_rerun_argv([])[5] is not None


TOP_HELP = """tinybrowser WPT harness: run | score | rerun | overnight | selftest

  run PATH... [--score] [--dry-run] [-- upstream-flags...]
    Run WPT. Upstream flags follow `--` (or start implicitly at the first
    `-flag`). --score runs then prints the per-directory table.
    --dry-run prints the wpt invocation without building or running.
  score PATH... [--report FILE | --save-report FILE] [-- upstream-flags...]
    Summarize an existing report (--report) or run then summarize.
  rerun REPORT [--include-timeout] [--dry-run] [--save-report FILE] [-- runner...]
    Re-run only the attention set from a wptreport.
  overnight [--save-report FILE] [--dry-run] [-- runner...]
    Full-suite run + score. Long; needs explicit approval per AGENTS.md.
  selftest
    Pure-function checks (worker filter, classify, arg grammars).

Examples:
  tools/wpt/run dom/events/ --exclude=worker
  tools/wpt/run --score dom/nodes/ -- --processes 4
  tools/wpt/rerun /tmp/fileapi.json -- --processes 8 --fully-parallel
"""


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    if argv == ["--selftest"]:
        argv = ["selftest"]
    if not argv or argv[0] in ("-h", "--help", "help"):
        print(TOP_HELP)
        return 0
    command, rest = argv[0], argv[1:]
    if command == "run":
        return run_main(rest)
    if command == "score":
        return score_main(rest)
    if command in ("rerun", "retest"):
        return rerun_main(rest)
    if command == "overnight":
        return overnight_main(rest)
    if command == "_inner":
        return inner_main(rest)
    if command == "selftest":
        _selftest()
        eprint("selftest: ok")
        return 0
    return die("wpt", f"unknown command {command}; expected run | score | rerun | overnight | selftest", 2)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except BrokenPipeError:
        devnull = os.open(os.devnull, os.O_WRONLY)
        os.dup2(devnull, sys.stdout.fileno())
        raise SystemExit(0)
