#!/usr/bin/env python3
"""Single entry point for the tinybrowser WPT harness.

Upstream WPT (`third_party/wpt`, pristine submodule) owns serving,
scheduling, driving, and reporting. This file owns the phases around it:
* `run`    — build the binary, ready the shared venv + manifest, exec wpt.
* `score`  — run a set, then print one row per directory + attention list.
* `rerun`  — re-run only the attention set from a saved wptreport.
* `merge`  — fold the chunk reports of a CI run into one score.
* `profile` — run tests (or read one report) and rank them by wall time.
* `failures` — print the canonical failure list from a `--score` log.
* `baseline` — run a slice at two revisions, diff the failure lists.
* `manifest` — one manifest walk for a chunked CI run (build job).
* `overnight` — full-suite run + score (needs explicit approval per AGENTS.md).

`tinybrowser_wpt.py` stays separate: upstream imports it as the
`wptrunner.products` entry-point, so it must remain a tiny importable module.
The run orchestration, in-venv patching, scoring, and rerun selection that
used to be split across shell and Python files live here, sharing one
argument grammar and one stamp/lock helper set.

Grammar (all subcommands): test paths and our flags come first; a literal
`--` starts verbatim upstream flags. For convenience an upstream flag before
`--` implicitly starts the verbatim region, so
`run dom/nodes/ --exclude=worker` works as well as
`run dom/nodes/ -- --exclude=worker`. The harness flags listed by `--help`
are recognized in any subcommand before `--`, and a command re-passes them
to whatever process it spawns, so a run has one configuration and no state
in the environment.
"""

from __future__ import annotations

import contextlib
import fcntl
import hashlib
import heapq
import io
import json
import os
import re
import shlex
import shutil
import signal
import socket
import statistics
import subprocess
import sys
import tempfile
import time
from collections import defaultdict, deque
from collections.abc import Iterable
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any, TypeVar

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
# `.any.worker.html` variants generated from `.any.js`, the security-features
# generator's `worker-classic.http.html` / `sharedworker-module.https.html`
# subresource files, and the canvas generator's `.w.html` variant in
# `html/canvas/` (the same OffscreenCanvas test run in a Worker).
WORKER_VARIANT = re.compile(
    r"(?:^|[/.])(any\.)?(sharedworker|serviceworker|worker)(-(classic|module))?"
    r"(\.[a-z0-9]+)*\.html$"
    r"|\.w\.html$"
)

# File-level statuses with their own bucket; anything else not-a-pass is
# classified by the report's `expected` field.
HARD_STATUSES = ("TIMEOUT", "CRASH", "ERROR")
TIMEOUT_STATUSES = ("TIMEOUT", "EXTERNAL-TIMEOUT")
MAX_LISTED = 40

# `wpt run`'s requirements (tools/wpt/commands.json), which WPT re-checks on
# every run, and a setuptools whose pkg_resources works for that check: WPT's
# tools/localpaths puts its vendored packaging 21 (needs pyparsing) ahead of
# site-packages, and setuptools >= 71 imports top-level packaging, so only
# releases that load their own vendored copy import cleanly there.
WPT_RUN_REQUIREMENTS = (
    "tools/manifest/requirements.txt",
    "tools/wptrunner/requirements.txt",
    "tools/serve/requirements.txt",
)
SETUPTOOLS = "setuptools<71"


def eprint(*parts: object) -> None:
    print(*parts, file=sys.stderr)


def die(prefix: str, message: str, code: int = 1) -> int:
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
            ["git", *args],
            cwd=str(cwd or ROOT),
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            check=False,
        )
    except OSError:
        return None
    if out.returncode != 0:
        return None
    return out.stdout.decode(errors="replace").strip()


# --- worker-variant filter (pure; selftested) ---


def exclude_skips_worker_variants(exclude: Iterable[str] | None) -> bool:
    return bool(exclude and any(item.strip("/") == "worker" for item in exclude))


def is_worker_variant_url(url: str) -> bool:
    return bool(WORKER_VARIANT.search(url.split("?", 1)[0]))


# --- option matching (mirrors wpt's allow_abbrev) ---


def has_option(option: str, args: list[str]) -> bool:
    for arg in args:
        if arg == option or arg.startswith(option + "="):
            return True
        if (
            option.startswith("--")
            and arg.startswith("--")
            and "=" not in arg
            and len(arg) >= 4
            and option.startswith(arg)
        ):
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


# --- harness options (every knob is a flag; no harness setting comes from the environment) ---


@dataclass(frozen=True)
class Options:
    """Harness configuration, resolved from argv once at the entry point.

    A subcommand and any process it spawns agree by construction: the parent
    re-creates the configuration as flags on the child's argv, so no value has
    to travel through the environment.
    """

    binary: str | None = None
    wpt_root: str | None = None
    venv: str | None = None
    durations: str | None = None
    log_level: str | None = None
    log_file: str | None = None
    allow_pin_mismatch: bool = False
    no_lock: bool = False
    lock_timeout: int | None = None

    def flags(self) -> list[str]:
        """This configuration as the flags that reproduce it."""
        out: list[str] = []
        for name, value in (
            ("--binary", self.binary),
            ("--wpt-root", self.wpt_root),
            ("--venv", self.venv),
            ("--log-level", self.log_level),
            ("--log-file", self.log_file),
        ):
            if value is not None:
                out += [name, value]
        if self.durations is not None:
            out.append(f"--durations={self.durations}")
        if self.allow_pin_mismatch:
            out.append("--allow-pin-mismatch")
        if self.no_lock:
            out.append("--no-lock")
        if self.lock_timeout is not None:
            out += ["--lock-timeout", str(self.lock_timeout)]
        return out


# flag -> (Options field, kind). `value` takes the next argv item and rejects an
# empty one, `value_or_empty` also accepts the bare `--flag=` form (the empty
# `--durations=` disables the longest-first split), `int` a non-negative whole
# number, `switch` is bare.
HARNESS_FLAGS: dict[str, tuple[str, str]] = {
    "--binary": ("binary", "value"),
    "--wpt-root": ("wpt_root", "value"),
    "--venv": ("venv", "value"),
    "--durations": ("durations", "value_or_empty"),
    "--log-level": ("log_level", "value"),
    "--log-file": ("log_file", "value"),
    "--lock-timeout": ("lock_timeout", "int"),
    "--allow-pin-mismatch": ("allow_pin_mismatch", "switch"),
    "--no-lock": ("no_lock", "switch"),
}


def extract_options(argv: list[str]) -> tuple[Options, list[str], str | None]:
    """Splits the harness flags out of argv, leaving the subcommand grammar.

    Stops at a literal `--` so upstream flags (`run -- --binary x`) are never
    mistaken for ours.
    """
    values: dict[str, Any] = {}
    rest: list[str] = []
    index = 0
    while index < len(argv):
        arg = argv[index]
        if arg == "--":
            rest.extend(argv[index:])
            break
        name = arg.split("=", 1)[0]
        entry = HARNESS_FLAGS.get(name)
        if entry is None:
            rest.append(arg)
            index += 1
            continue
        field, kind = entry
        if "=" in arg:
            value = arg.split("=", 1)[1]
        elif kind == "switch":
            value = ""
        else:
            # A harness value never starts with `-`, so refusing one keeps
            # `--binary --log-level=trace` from eating the next flag.
            if index + 1 >= len(argv) or argv[index + 1].startswith("-"):
                return Options(), [], f"{name} needs a value"
            value = argv[index + 1]
            index += 1
        if kind == "switch":
            if value:
                return Options(), [], f"{name} takes no value"
            values[field] = True
        elif kind == "int":
            try:
                number = int(value)
            except ValueError:
                return Options(), [], f"{name} needs a whole number"
            if number < 0:
                return Options(), [], f"{name} needs a non-negative number"
            values[field] = number
        elif kind == "value" and not value:
            return Options(), [], f"{name} needs a value"
        else:
            values[field] = value
        index += 1
    return Options(**values), rest, None


# --- WPT checkout ---


def resolve_wpt_root(options: Options) -> Path:
    override = options.wpt_root
    if override:
        return Path(override)
    local = ROOT / "third_party" / "wpt"
    if (local / "wpt").is_file():
        return local
    worktrees = git(["worktree", "list", "--porcelain"]) or ""
    primary = None
    for line in worktrees.splitlines():
        if line.startswith("worktree "):
            primary = line.split(" ", 1)[1]
            break
    base = Path(primary) if primary else ROOT
    return base / "third_party" / "wpt"


def check_pin(wpt: Path, options: Options) -> int:
    pinned = git(["rev-parse", "HEAD:third_party/wpt"])
    actual = git(["rev-parse", "HEAD"], cwd=wpt)
    allow = options.allow_pin_mismatch
    if pinned and not allow:
        if not actual:
            eprint(f"cannot read the WPT revision at {wpt}; it is not a git checkout")
            eprint("Pass --allow-pin-mismatch to run it anyway.")
            return 1
        if pinned != actual:
            eprint(f"WPT checkout at {wpt} is {actual}; this worktree pins {pinned}")
            eprint("Update the checkout, or pass --allow-pin-mismatch.")
            return 1
    elif pinned and pinned != (actual or ""):
        eprint(
            f"WPT pin mismatch ignored: running {actual or 'an unknown revision'}, worktree pins {pinned}"
        )
    return 0


# --- locks (shared venv + shared manifest) ---

# Seconds to wait for a shared venv/manifest lock when `--lock-timeout` is absent.
LOCK_TIMEOUT_S = 1800


def acquire_lock(name: str, options: Options, what: str | None = None):
    if options.no_lock:
        return None
    path = cache_dir() / name
    handle = open(path, "a+b")
    try:
        fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        return handle
    except BlockingIOError:
        pass
    timeout_s = options.lock_timeout
    if timeout_s is None:
        timeout_s = LOCK_TIMEOUT_S
    eprint(f"another WPT run holds {path}; waiting")
    deadline = time.monotonic() + timeout_s
    while True:
        try:
            fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            return handle
        except BlockingIOError:
            if time.monotonic() >= deadline:
                eprint(f"timed out waiting for {what or path}")
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
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
                check=False,
            )
            lines.append(out.stdout.decode(errors="replace").strip() or "no-python")
        except OSError:
            lines.append("no-python")
    else:
        lines.append("no-python")
    for rel in WPT_RUN_REQUIREMENTS:
        lines.append(sha_file(wpt / rel))
    lines.append(SETUPTOOLS)
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


def install_venv(wpt: Path, venv: Path, options: Options) -> None:
    if shutil.which("uv") is None:
        eprint("uv is required to bootstrap the WPT virtualenv")
        raise SystemExit(1)
    exe = venv_python(venv)
    if not (exe.is_file() and os.access(exe, os.X_OK)):
        if options.venv:
            eprint(f"WPT virtualenv not found at {venv}; --venv must name one")
            raise SystemExit(1)
        subprocess.run(
            ["uv", "venv", "--seed", str(venv), "--python", "3.12"], check=True
        )
    # uv seeds only pip on Python 3.12; WPT's per-run requirements check
    # needs pkg_resources, or it shells out to pip for every file.
    requirements = [
        arg for rel in WPT_RUN_REQUIREMENTS for arg in ("-r", str(wpt / rel))
    ]
    subprocess.run(
        ["uv", "pip", "install", "--python", str(exe), "-q", SETUPTOOLS, *requirements],
        check=True,
    )
    subprocess.run(
        [
            "uv",
            "pip",
            "install",
            "--python",
            str(exe),
            "-q",
            "-e",
            str(ROOT / "tools" / "wpt"),
        ],
        check=True,
    )
    (venv / ".tinybrowser-wpt-stamp").write_text(venv_stamp_payload(wpt, venv) + "\n")


def ensure_venv(wpt: Path, venv: Path, options: Options) -> None:
    if venv_is_current(wpt, venv):
        eprint("wpt: venv is current, skipping install")
        return
    lock = acquire_lock(f"wpt-{sha16(str(venv.resolve()))}.lock", options)
    try:
        if venv_is_current(wpt, venv):
            eprint("wpt: venv is current, skipping install")
        else:
            eprint("wpt: installing WPT virtualenv")
            install_venv(wpt, venv, options)
    finally:
        release_lock(lock)


# --- manifest (whole-tree inventory; skip the walk when unchanged) ---


def tree_state(wpt: Path) -> list[str]:
    """The checkout's content: HEAD plus hashes of every changed or
    untracked file. Taken before a run so an edit made while tests run is
    not stamped as walked."""
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
            ["git", "diff", "HEAD", "--name-only", "-z"],
            cwd=str(wpt),
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            check=False,
        )
        names = [p for p in proc.stdout.decode(errors="replace").split("\0") if p]
        parts.extend(hashed(names))
    except OSError:
        pass
    try:
        proc = subprocess.run(
            ["git", "ls-files", "--others", "--exclude-standard", "-z"],
            cwd=str(wpt),
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            check=False,
        )
        names = [p for p in proc.stdout.decode(errors="replace").split("\0") if p]
        parts.extend(hashed(names))
    except OSError:
        pass
    return parts


def manifest_token(tree: list[str], manifest: Path) -> str:
    parts = list(tree)
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
        return manifest_token(tree_state(wpt), manifest) == stamp.read_text().strip()
    except OSError:
        return False


def write_manifest_stamp(wpt: Path, manifest: Path, tree: list[str]) -> None:
    if git(["rev-parse", "HEAD"], cwd=wpt) is None:
        return
    stamp = manifest_stamp_file(manifest)
    try:
        stamp.write_text(manifest_token(tree, manifest) + "\n")
    except OSError:
        pass


def manifest_parses(exe: Path, manifest: Path) -> bool:
    try:
        out = subprocess.run(
            [
                str(exe),
                "-c",
                "import json,sys; json.load(open(sys.argv[1]))",
                str(manifest),
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
        return out.returncode == 0
    except OSError:
        return False


# --- argument grammars ---


def split_run_argv(
    argv: list[str],
) -> tuple[list[str], bool, bool, list[str], str | None]:
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
            extras.extend(argv[i + 1 :])
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
                if (
                    j < len(argv)
                    and not argv[j].startswith("-")
                    and "/" not in argv[j]
                    and "." not in argv[j]
                ):
                    return (
                        [],
                        False,
                        False,
                        [],
                        (
                            f"unknown --test-types value '{argv[j]}'; expected one of: {KNOWN_TEST_TYPES}"
                        ),
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


def hoist_paths_after_test_types(
    paths: list[str], extras: list[str]
) -> tuple[list[str], list[str], str | None]:
    """Rescue positionals `--test-types` (nargs='*') would swallow.

    Upstream argparse greedily consumes every positional following
    `--test-types` as another type, so `run --processes 4 --test-types
    reftest dom/nodes/` would silently cover nothing. Anything bare that
    directly trails the type values and looks like a path (or `-`) moves
    back to paths; a typeless bare word is a typo and fails loudly.
    Option values elsewhere are untouched: only the tail of
    `--test-types` is hazardous, since argparse assigns free positionals
    correctly everywhere else.
    """
    out: list[str] = []
    i = 0
    while i < len(extras):
        arg = extras[i]
        out.append(arg)
        i += 1
        if arg != "--test-types" and not arg.startswith("--test-types="):
            continue
        while i < len(extras) and extras[i] in KNOWN_TEST_TYPE_SET:
            out.append(extras[i])
            i += 1
        while i < len(extras) and not extras[i].startswith("-"):
            token = extras[i]
            i += 1
            if token == "-" or "/" in token or "." in token:
                paths.append(token)
            else:
                return (
                    paths,
                    [],
                    (
                        f"unknown --test-types value '{token}'; expected one of: {KNOWN_TEST_TYPES}"
                    ),
                )
    return paths, out, None


def split_score_argv(
    argv: list[str],
) -> tuple[Path | None, Path | None, list[str], list[str], bool, str | None]:
    report: Path | None = None
    save_report: Path | None = None
    paths: list[str] = []
    extra: list[str] = []
    dry_run = False
    i = 0
    while i < len(argv):
        arg = argv[i]
        if arg == "--":
            extra = list(argv[i + 1 :])
            break
        if arg in ("--report", "--save-report") or arg.startswith(
            ("--report=", "--save-report=")
        ):
            option, value = arg.split("=", 1) if "=" in arg else (arg, None)
            if value is None:
                if i + 1 >= len(argv):
                    return (
                        report,
                        save_report,
                        paths,
                        extra,
                        dry_run,
                        f"{option} needs a file",
                    )
                value = argv[i + 1]
                i += 1
            if not value:
                return (
                    report,
                    save_report,
                    paths,
                    extra,
                    dry_run,
                    f"{option} needs a file",
                )
            if option == "--report":
                report = Path(value)
            else:
                save_report = Path(value)
        elif arg == "--dry-run":
            # Honored only here, before `--`: past the separator it is a
            # verbatim upstream flag like any other.
            dry_run = True
        elif arg.startswith("-"):
            extra.extend(argv[i:])
            break
        else:
            paths.append(arg)
        i += 1
    return report, save_report, paths, extra, dry_run, None


def parse_rerun_argv(
    argv: list[str],
) -> tuple[Path | None, bool, bool, Path | None, list[str], str | None]:
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
                return (
                    None,
                    include_timeout,
                    dry_run,
                    save_report,
                    list(argv[i + 1 :]),
                    usage,
                )
            return (
                report,
                include_timeout,
                dry_run,
                save_report,
                list(argv[i + 1 :]),
                None,
            )
        if arg == "--include-timeout":
            include_timeout = True
        elif arg == "--dry-run":
            dry_run = True
        elif arg == "--save-report" or arg.startswith("--save-report="):
            if arg == "--save-report":
                if i + 1 >= len(argv):
                    return (
                        report,
                        include_timeout,
                        dry_run,
                        None,
                        [],
                        "--save-report needs a file",
                    )
                save_report = Path(argv[i + 1])
                i += 1
            else:
                value = arg.split("=", 1)[1]
                if not value:
                    return (
                        report,
                        include_timeout,
                        dry_run,
                        None,
                        [],
                        "--save-report needs a file",
                    )
                save_report = Path(value)
        elif arg.startswith("-"):
            return (
                report,
                include_timeout,
                dry_run,
                save_report,
                list(argv[i:]),
                "runner options must follow a literal `--`",
            )
        elif report is None:
            report = Path(arg)
        else:
            return (
                report,
                include_timeout,
                dry_run,
                save_report,
                [],
                f"unexpected argument {arg}",
            )
        i += 1
    if report is None:
        return None, include_timeout, dry_run, save_report, [], usage
    return report, include_timeout, dry_run, save_report, [], None


def parse_merge_argv(
    argv: list[str],
) -> tuple[list[Path], Path | None, bool, str | None]:
    reports: list[Path] = []
    out: Path | None = None
    strict = False
    i = 0
    while i < len(argv):
        arg = argv[i]
        if arg == "--out" or arg.startswith("--out="):
            if arg == "--out":
                if i + 1 >= len(argv) or argv[i + 1].startswith("-"):
                    return reports, None, strict, "--out needs a file"
                out = Path(argv[i + 1])
                i += 1
            else:
                value = arg.split("=", 1)[1]
                if not value:
                    return reports, None, strict, "--out needs a file"
                out = Path(value)
        elif arg == "--strict-skips":
            strict = True
        elif arg.startswith("-"):
            return (
                reports,
                out,
                strict,
                f"merge takes report files, not {arg} (chunk flags belong to `run`)",
            )
        else:
            reports.append(Path(arg))
        i += 1
    if not reports:
        return reports, out, strict, "merge needs at least one report file"
    return reports, out, strict, None


# --- report primitives (score + rerun share these) ---


def group_of(test: str) -> str:
    parts = [p for p in test.split("/") if p]
    if len(parts) <= 1:
        return "(root)"
    if "." in parts[1]:
        # A test directly inside a top-level directory groups under that
        # directory, not under the file; the table header says "directory".
        return parts[0]
    return "/".join(parts[:2])


def is_expected(entry: dict) -> bool:
    if entry.get("expected") is None:
        return True
    return entry.get("status") in (entry.get("known_intermittent") or [])


def unexpected_subs(subtests: list) -> list[dict]:
    return [s for s in subtests if s.get("status") != "SKIP" and not is_expected(s)]


def classify_results(
    results: list[dict],
) -> tuple[dict[str, dict[str, int]], list[tuple[str, str, str]]]:
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
                attention.append(
                    (test, sub.get("name", "?"), sub.get("status", "ERROR"))
                )
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
        eprint(
            f"report {report} is not valid JSON ({error}); the runner probably failed before writing results"
        )
        return 1
    results = data.get("results", [])
    if not results:
        eprint(f"no results in report {report}")
        return 1
    return summarize_results(results)


def summarize_results(results: list[dict]) -> int:
    if not results:
        eprint("no results to summarize")
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
    return [
        r["test"]
        for r in data.get("results", [])
        if r.get("test") and needs_retest(r, include_timeout)
    ]


def rerun_status(remaining: int, runner_exit: int) -> tuple[int, bool]:
    if runner_exit != 0:
        return runner_exit, True
    if remaining:
        return 1, True
    return 0, False


# --- `merge` (chunk reports of a CI run -> one report, one score) ---


def load_report(report: Path) -> tuple[dict | None, str | None]:
    """The parsed chunk report, or why it cannot be used at all.

    wptreport only materializes its JSON at suite_end, so a chunk whose
    harness died early (job/step timeout, segfault, wptserve failing to
    bind) leaves an empty or truncated file. `merge` treats that as a
    skipped report: the score job's report-count check is what fails the
    run, and one bad file must not cost the table for the other chunks.
    """
    try:
        text = report.read_text()
    except OSError as error:
        return None, f"cannot read {report}: {error}"
    try:
        data = json.loads(text)
    except json.JSONDecodeError as error:
        return None, f"{report} is not valid JSON ({error})"
    if not isinstance(data, dict):
        return None, f"{report} is not a wptreport object ({type(data).__name__})"
    return data, None


def fold_reports(
    loaded: list[tuple[Path, dict]],
) -> tuple[dict | None, list[str], str | None]:
    """Fold per-chunk wptreports into one, preserving run metadata.

    Returns (merged, warnings, error). The only hard error is a duplicate
    `test`/`subsuite` pair: upstream dir_hash chunking puts every test in
    exactly one chunk, so an overlap means the chunking and the reports
    disagree and folding would double-count. run_info and subsuites come
    from the first report that has them (chunk jobs are separate runners;
    a disagreement is a warning), and the time bounds span every report.
    """
    results: list[dict] = []
    where: dict[tuple[str, str], int] = {}
    warnings: list[str] = []
    run_info: dict | None = None
    subsuites: dict = {}
    starts: list[float] = []
    ends: list[float] = []
    empty: list[Path] = []
    for index, (report, data) in enumerate(loaded):
        chunk = data.get("results")
        if chunk is None:
            chunk = []
        if not isinstance(chunk, list):
            warnings.append(f"{report}: results is not a list; skipped")
            continue
        if not chunk:
            # A chunk legitimately runs zero tests (narrow test-types, big
            # chunks value, everything excluded); warn, never fail.
            empty.append(report)
        for result in chunk:
            if not isinstance(result, dict) or not result.get("test"):
                return None, warnings, f"a result in {report} has no test id"
            key = (result.get("subsuite") or "", result["test"])
            if key in where:
                first = where[key]
                if first == index:
                    return None, warnings, f"{result['test']} appears twice in {report}"
                return (
                    None,
                    warnings,
                    f"{result['test']} appears in {loaded[first][0]} and {report}",
                )
            where[key] = index
            results.append(result)
        info = data.get("run_info")
        if isinstance(info, dict):
            if run_info is None:
                run_info = info
            elif info != run_info:
                warnings.append(f"{report}: run_info differs from the first report")
        elif info is not None:
            warnings.append(f"{report}: run_info is not an object; ignored")
        for key, value in (data.get("subsuites") or {}).items():
            if key not in subsuites:
                subsuites[key] = value
            elif subsuites[key] != value:
                warnings.append(f"{report}: subsuite {key!r} differs; first wins")
        for name, times in (("time_start", starts), ("time_end", ends)):
            value = data.get(name)
            if isinstance(value, (int, float)):
                times.append(value)
    if empty:
        warnings.append("no results in " + ", ".join(str(report) for report in empty))
    # subsuites entries carry their own verdicts upstream; the empty
    # subsuite is the only one today and says nothing beyond its key.
    merged: dict = {
        "results": sorted(
            results, key=lambda result: (result.get("subsuite") or "", result["test"])
        )
    }
    if run_info is not None:
        merged["run_info"] = run_info
    if subsuites:
        merged["subsuites"] = subsuites
    if starts:
        merged["time_start"] = min(starts)
    if ends:
        merged["time_end"] = max(ends)
    return merged, warnings, None


def merge_main(argv: list[str]) -> int:
    reports, out, strict, error = parse_merge_argv(argv)
    if error:
        return die("merge", error, 2)
    loaded: list[tuple[Path, dict]] = []
    skipped = 0
    for report in reports:
        data, problem = load_report(report)
        if data is None:
            eprint(f"wpt: merge: skipping {problem}")
            skipped += 1
            continue
        loaded.append((report, data))
    if not loaded:
        return die("merge", f"no usable report among {len(reports)}", 2)
    merged, warnings, error = fold_reports(loaded)
    for warning in warnings:
        eprint(f"wpt: merge: {warning}")
    if error:
        return die("merge", error, 2)
    assert merged is not None
    if out is not None:
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(json.dumps(merged))
        eprint(
            f"merged {len(loaded)} reports -> {out} ({len(merged['results'])} results)"
        )
    code = summarize_results(merged["results"])
    if strict and skipped:
        # The table is out and --out is written; the caller (CI score job)
        # turns an unusable chunk report into a red run without losing it.
        eprint(f"wpt: merge: {skipped} of {len(reports)} chunk reports were unusable")
        return code or 2
    return code


# --- `profile` (wall-time census over a report or a live run) ---


# A passing test above this many ms is close to the normal budget's cliff.
NEAR_BUDGET_MS = 2500

# Browser-timeline records shown per profiled run.
TIMELINE_LINES = 40


def profile_report(report: Path, top: int) -> int:
    """Ranks one wptreport by wall time: killed tests first, then the near."""
    try:
        data = json.loads(report.read_text())
    except OSError as error:
        eprint(f"cannot read report {report}: {error}")
        return 1
    except json.JSONDecodeError as error:
        eprint(
            f"report {report} is not valid JSON ({error}); the runner probably failed before writing results"
        )
        return 1
    results = [r for r in data.get("results", []) if r.get("test")]
    if not results:
        eprint(f"no results in report {report}")
        return 1
    ranked = sorted(results, key=lambda r: -(r.get("duration") or 0))
    killed = [r for r in results if r.get("status") in HARD_STATUSES]
    near = [
        r
        for r in results
        if r.get("status") not in HARD_STATUSES
        and (r.get("duration") or 0) >= NEAR_BUDGET_MS
    ]
    total_s = sum(r.get("duration") or 0 for r in results) / 1000
    print(
        f"{len(results)} results, {total_s:.1f}s total, {len(killed)} killed, "
        f"{len(near)} passing within {NEAR_BUDGET_MS}ms of the budget"
    )
    print()
    test_width = max((len(r["test"]) for r in ranked[:top]), default=0)
    print(f"{'wall'.rjust(9)}  {'status'.ljust(8)}  test")
    print("-" * (11 + 8 + test_width))
    for result in ranked[:top]:
        duration = result.get("duration") or 0
        status = str(result.get("status", "?"))
        print(f"{duration:7.0f}ms  {status[:8].ljust(8)}  {result['test']}")
    return 0


def print_timeline(log: Path) -> None:
    """Prints the browser's navigation and request records from one kept log."""
    try:
        lines = log.read_text(errors="replace").splitlines()
    except OSError as error:
        eprint(f"cannot read browser log {log}: {error}")
        return
    records = [line for line in lines if "browser::net" in line]
    print()
    print(f"timeline ({log}), last {TIMELINE_LINES} records:")
    for line in records[-TIMELINE_LINES:]:
        print(f"  {line}")


def profile_run(options: Options, paths: list[str], extra: list[str], top: int) -> int:
    """Runs tests at trace with the browser log at a path it picks, then ranks them.

    A run's own profile is thrown away at teardown, so `profile` gives the
    browser its own temp log instead; the caller just asks for a profile.
    """
    handle, name = tempfile.mkstemp(prefix="wpt-profile-", suffix=".json")
    os.close(handle)
    report = Path(name)
    handle, name = tempfile.mkstemp(prefix="wpt-profile-", suffix=".log")
    os.close(handle)
    log = Path(name)
    profiled = replace(options, log_level="trace", log_file=str(log))
    command = [
        sys.executable,
        str(CLI),
        "run",
        *paths,
        *profiled.flags(),
        "--log-wptreport",
        str(report),
        "--no-fail-on-unexpected",
        *extra,
    ]
    eprint(f"$ {shlex.join(command)}")
    proc = subprocess.run(
        command,
        cwd=str(ROOT),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    out = proc.stdout.decode(errors="replace")
    stderr = proc.stderr.decode(errors="replace")
    sys.stderr.write(out)
    sys.stderr.write(stderr)
    if proc.returncode != 0:
        eprint(f"runner exited {proc.returncode}")
    eprint(f"profile: browser log {log}")
    if log.is_file():
        print_timeline(log)
    code = profile_report(report, top)
    try:
        os.unlink(report)
    except OSError:
        pass
    return code


def profile_main(options: Options, argv: list[str]) -> int:
    usage = "profile PATH... [--top N] | profile --report FILE [--top N]"
    paths: list[str] = []
    extra: list[str] = []
    report: Path | None = None
    top = MAX_LISTED
    index = 0
    while index < len(argv):
        arg = argv[index]
        if arg == "--":
            extra = argv[index + 1 :]
            break
        if arg.startswith("--report=") or arg.startswith("--top="):
            flag, value = arg.split("=", 1)
        elif arg in ("--report", "--top"):
            if index + 1 >= len(argv):
                return die("profile", f"{arg} needs a value", 2)
            flag, value = arg, argv[index + 1]
            index += 1
        elif arg.startswith("-"):
            # A flag that is not ours opens the runner's verbatim region,
            # matching the `run` grammar.
            extra = argv[index:]
            break
        else:
            paths.append(arg)
            index += 1
            continue
        if flag == "--top":
            try:
                top = int(value)
            except ValueError:
                return die("profile", f"--top needs an integer, got {value!r}", 2)
            if top < 1:
                return die("profile", f"--top needs a positive integer, got {top}", 2)
        else:
            report = Path(value)
        index += 1
    if report is not None and paths:
        return die("profile", f"give paths or --report, not both; usage: {usage}", 2)
    if report is not None:
        return profile_report(report, top)
    if not paths:
        return die("profile", usage, 2)
    return profile_run(options, paths, extra, top)


# --- longest-first split (durations from earlier runs) ---


def durations_source(options: Options) -> Path | None:
    """The wptreport-shaped file whose per-test durations drive the split.

    `--durations` names one (any `--log-wptreport` output); `--durations=`
    (empty) disables the split. Absent, it is the cache every run refreshes,
    so the second run of a slice is already balanced.
    """
    override = options.durations
    if override is not None:
        return Path(override) if override else None
    return cache_dir() / "wpt-durations.json"


def read_durations(report: Path) -> dict[str, float]:
    try:
        data = json.loads(report.read_text())
    except (OSError, json.JSONDecodeError):
        return {}
    return {
        r["test"]: float(r["duration"])
        for r in data.get("results", [])
        if r.get("test")
        and isinstance(r.get("duration"), (int, float))
        and r["duration"] > 0
    }


def write_durations(cache: Path, durations: dict[str, float]) -> None:
    merged = read_durations(cache)
    merged.update(durations)
    results = [
        {"test": test, "duration": round(ms)} for test, ms in sorted(merged.items())
    ]
    try:
        with tempfile.NamedTemporaryFile(
            "w", dir=cache.parent, prefix=cache.name, delete=False
        ) as out:
            json.dump({"results": results}, out)
        os.replace(out.name, cache)
    except OSError as error:
        eprint(f"wpt: cannot record test durations in {cache}: {error}")


_K = TypeVar("_K")


def longest_first(costs: dict[_K, float], buckets: int) -> list[list[_K]]:
    """LPT: hand each test, longest first, to the least-loaded bucket.

    Bucket order is longest first too, so stragglers start early. Ties
    break on the test id, so the split is deterministic for a given input.
    """
    loads = [(0.0, i) for i in range(buckets)]
    out: list[list[_K]] = [[] for _ in range(buckets)]
    for test in sorted(costs, key=lambda t: (-costs[t], t)):
        load, i = heapq.heappop(loads)
        out[i].append(test)
        heapq.heappush(loads, (load + costs[test], i))
    return out


class DurationRecorder:
    """mozlog handler: wall time from `test_start` to `test_end`, in ms."""

    def __init__(self) -> None:
        self.started: dict[str, int] = {}
        self.durations: dict[str, float] = {}

    def __call__(self, data: dict) -> None:
        action = data.get("action")
        if action == "test_start":
            self.started[data["test"]] = data["time"]
        elif action == "test_end" and data["test"] in self.started:
            self.durations[data["test"]] = data["time"] - self.started.pop(data["test"])


# --- in-venv runner ---


def patch_wpt(
    wpt_root: Path, durations: dict[str, float], recorder: DurationRecorder
) -> None:
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

    wpt_run.check_environ = skip_tinybrowser_hosts  # ty: ignore[invalid-assignment] -- intentional narrowing patch; upstream is untyped, product is always str, verified by every run

    get_ports = ConfigBuilder._get_ports

    def skip_tls_ports_without_ssl(self, data) -> dict:
        ports = get_ports(self, data)
        ssl_type = data.get("ssl", {}).get("type", "none")
        if not sslutils.get_cls(ssl_type).ssl_enabled:
            for scheme in TLS_SCHEMES:
                ports.pop(scheme, None)
            for scheme in list(ports):
                if scheme not in HTTP_ONLY:
                    ports.pop(scheme, None)
        return ports

    ConfigBuilder._get_ports = skip_tls_ports_without_ssl  # ty: ignore[invalid-assignment] -- intentional runtime patch of untyped upstream; verified by every run

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

    testloader.TestFilter: type[testloader.TestFilter] = TinybrowserTestFilter

    from wptrunner.browsers import base as browser_base

    def wait_for_webdriver(
        logger: Any,
        host: str,
        port: int,
        timeout: int | float = 60,
        server_process: Any = None,
    ) -> bool:
        """`wait_for_service`, polling every 10 ms instead of 500 ms.

        The tinybrowser WebDriver listens ~75 ms after spawn, so upstream's
        interval turned every browser start into a 0.5 s wait.
        """
        end = time.time() + timeout
        while time.time() < end:
            if server_process is not None and server_process.poll() is not None:
                break
            with socket.socket() as probe:
                try:
                    probe.connect((host, port))
                except ConnectionRefusedError:
                    time.sleep(0.01)
                    continue
            logger.debug(f"Connected to {host}:{port}")
            return True
        raise OSError(f"Service is unavailable: {host}:{port}")

    browser_base.wait_for_service = wait_for_webdriver  # ty: ignore[invalid-assignment] -- intentional runtime patch; polling interval is the whole point, verified by every run

    from wptrunner import wptlogging

    enter_logger = wptlogging.LoggerManager.__enter__

    def enter_recording_logger(self):
        logger = enter_logger(self)
        if recorder not in logger.handlers:
            logger.add_handler(recorder)
        return logger

    wptlogging.LoggerManager.__enter__ = enter_recording_logger  # ty: ignore[invalid-assignment] -- intentional runtime patch; return type lives in venv-only mozlog, verified by every run

    class LongestFirstSource(testloader.SingleTestSource):
        """Default (no `--run-by-dir`/`-f`) split, balanced by past durations.

        Upstream hashes test ids into one fixed group per process and test
        type, so one unlucky process gets the 60 s tests. Here every test,
        whatever its type, goes longest first to the least-loaded process;
        each process's tests become one group per type, and the queue hands
        out the costliest groups first so the short tail groups land on
        whichever process frees up. Tests never timed cost the median.
        """

        def make_queue(self, tests_by_type):
            tests = {
                (key, t.id): t for key, group in tests_by_type.items() for t in group
            }
            known = [durations[test_id] for _, test_id in tests if test_id in durations]
            if not known:
                return super().make_queue(tests_by_type)
            eprint(f"wpt: longest-first split, {len(known)} tests with known durations")
            unknown_ms = statistics.median(known)
            costs = {item: durations.get(item[1], unknown_ms) for item in tests}
            groups = []
            for bucket in longest_first(costs, self.kwargs["processes"]):
                by_type = defaultdict(list)
                for key, test_id in bucket:
                    by_type[key].append((key, test_id))
                for (subsuite, test_type), items in by_type.items():
                    metadata = self.group_metadata({})
                    queue = deque()
                    for item in items:
                        tests[item].update_metadata(metadata.extra)
                        queue.append(tests[item])
                    cost = sum(costs[item] for item in items)
                    groups.append(
                        (
                            cost,
                            testloader.TestGroup(queue, subsuite, test_type, metadata),
                        )
                    )
            groups.sort(key=lambda entry: entry[0], reverse=True)
            test_queue = testloader.WriteQueue()
            for _, group in groups:
                test_queue.put(group)
            return test_queue.to_read(), self.process_count(
                self.kwargs["processes"], len(groups)
            )

    testloader.SingleTestSource: type[testloader.SingleTestSource] = LongestFirstSource


def inner_main(options: Options, argv: list[str]) -> int:
    """Runs under the venv python: patch upstream, then `wpt run`."""
    if argv[:1] == ["--"]:
        argv = argv[1:]
    wpt_root = resolve_wpt_root(options)
    source = durations_source(options)
    recorder = DurationRecorder()
    patch_wpt(wpt_root, read_durations(source) if source else {}, recorder)
    from tools.wpt import wpt

    try:
        code = wpt.main(prog=str(wpt_root / "wpt"), argv=["run", *argv])
    finally:
        if recorder.durations:
            write_durations(cache_dir() / "wpt-durations.json", recorder.durations)
    return code if isinstance(code, int) else 0


# --- `run` (setup, then exec or spawn the venv python) ---


def build_binary(options: Options) -> Path:
    override = options.binary
    if override:
        eprint(
            f"wpt: using the --binary override without a freshness check: {override}"
        )
        return Path(override)
    out = subprocess.run(
        [str(ROOT / "tools" / "binary-path"), "--profile", "wpt"],
        cwd=str(ROOT),
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    binary = Path(out.stdout.decode(errors="replace").strip())
    # The `wpt` profile (see the root Cargo.toml) keeps dev's assertions but
    # optimizes the code: file timeouts must measure the engine, not a `-O0`
    # build spending the budget inside QuickJS.
    eprint("wpt: building tinybrowser (cargo no-ops when fresh)")
    # cargo's fingerprint tracks every compiled input; a failure means the
    # binary would be stale, so refuse instead of running against it.
    ret = subprocess.run(
        ["cargo", "build", "-q", "--profile", "wpt", "--offline"], cwd=str(ROOT)
    ).returncode
    if ret != 0:
        ret = subprocess.run(
            ["cargo", "build", "-q", "--profile", "wpt"], cwd=str(ROOT)
        ).returncode
    if ret != 0:
        eprint("wpt: cargo build failed; refusing to run against a stale binary")
        raise SystemExit(1)
    return binary


def finalize_run_args(wpt: Path, extras: list[str]) -> list[str]:
    args = list(extras)
    if not has_option("--metadata", args):
        args = ["--metadata", str(ROOT / "tools" / "wpt" / "metadata"), *args]
    if not has_option("--manifest", args):
        # Upstream defaults the manifest into ${metadata_root}, which would
        # land the 40MB walk output in our baselines dir; pin it at the WPT
        # checkout so the stamp, lock, and mtime gate track the file wpt uses.
        args = ["--manifest", str(wpt / "MANIFEST.json"), *args]
    if not has_option("--pause-after-test", args) and not has_option(
        "--no-pause-after-test", args
    ):
        args = ["--no-pause-after-test", *args]
    if not has_option("--no-restart-on-unexpected", args) and not has_option(
        "--restart-on-unexpected", args
    ):
        args = ["--no-restart-on-unexpected", *args]
    if not has_option("--ssl-type", args):
        args = ["--ssl-type=openssl", *args]
    if (
        not has_option("--processes", args)
        and not has_option("--fully-parallel", args)
        and not has_option("-f", args)
    ):
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
            [
                str(exe),
                "-c",
                "import sys; sys.path.insert(0, 'tools/wptrunner'); "
                "from wptrunner import wpttest; print(' '.join(sorted(wpttest.enabled_tests)))",
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            check=False,
        )
        actual = out.stdout.decode(errors="replace").strip()
    except OSError:
        return
    if actual and actual != KNOWN_TEST_TYPES:
        eprint(
            f"wpt: KNOWN_TEST_TYPES drifts from wpttest.enabled_tests ({actual}); update tools/wpt/cli.py"
        )


def inner_flags(wpt: Path, options: Options) -> list[str]:
    """Flags for the `_inner` child: only what it cannot derive itself.

    The child re-resolves the default durations cache identically, so only a
    caller-supplied value has to travel. The checkout is always passed: the
    parent did the worktree lookup, and the child must see the same tree.
    """
    out = ["--wpt-root", str(wpt)]
    if options.durations is not None:
        out.append(f"--durations={options.durations}")
    return out


def run_wpt(
    wpt: Path,
    venv: Path,
    exe: Path,
    binary: Path,
    final_args: list[str],
    manifest: Path,
    record: int,
    options: Options,
) -> int:
    # wpt's `--binary-arg` is the only channel that carries an argument into
    # the product's command line, and the browser owns both of these.
    wpt_args = list(final_args)
    for flag, value in (
        ("log-level", options.log_level),
        ("log-file", options.log_file),
    ):
        if value:
            wpt_args.append(f"--binary-arg=--{flag}={value}")
    # Everything for the harness goes before `--`; wpt's own `--binary` and the
    # user's upstream flags stay after it, untouched.
    inner_argv = [
        *inner_flags(wpt, options),
        "--",
        "--binary",
        str(binary),
        "tinybrowser",
        *wpt_args,
    ]
    manifest_lock = None
    if not has_option_exact("--no-manifest-update", final_args):
        lock_name = f"wpt-manifest-{sha16(str(manifest.resolve()))}.lock"
        manifest_lock = acquire_lock(lock_name, options, what=f"manifest {manifest}")
    manifest_locked = manifest_lock is not None
    venv_lock = None
    if (
        has_option("--install-browser", final_args)
        or has_option("--install-webdriver", final_args)
        or has_option_exact("--manifest-download", final_args)
        or has_option("--install-fonts", final_args)
    ):
        venv_lock = acquire_lock(
            f"wpt-{sha16(str(venv.resolve()))}.lock",
            options,
            what=f"venv {venv}",
        )

    for i, arg in enumerate(final_args):
        if arg == "--processes" and i + 1 < len(final_args):
            eprint(f"wpt: processes {final_args[i + 1]}")
            break
        if arg.startswith("--processes="):
            eprint(f"wpt: processes {arg.split('=', 1)[1]}")
            break

    if record == 0 and not manifest_locked and venv_lock is None:
        # Fast path: nothing to record or serialize. exec restores direct
        # signal delivery to the runner instead of stranding browsers behind us.
        release_lock(manifest_lock)
        try:
            os.execv(str(exe), [str(exe), str(CLI), "_inner", *inner_argv])
        except OSError as error:
            return die("run", f"cannot exec {exe}: {error}")
        raise AssertionError("unreachable")

    # Slow path: a manifest walk and/or venv writes happen under us, so stay
    # parent and forward signals explicitly. Children must not inherit the
    # lock fds (the lock belongs to the open file description; one orphaned
    # browser would block every later run).
    try:
        manifest_mtime_before = manifest.stat().st_mtime_ns
    except OSError:
        manifest_mtime_before = 0
    # The walk sees the tree as it is when the runner starts; snapshot it now.
    tree_before = tree_state(wpt) if record == 1 and manifest_locked else []
    try:
        child = subprocess.Popen(
            [str(exe), str(CLI), "_inner", *inner_argv],
            cwd=str(wpt),
            close_fds=True,
        )
    except OSError as error:
        release_lock(venv_lock)
        release_lock(manifest_lock)
        return die("run", f"could not start the runner: {error}")
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
        # wait() retries through trapped signals internally (unlike bash
        # wait, it does not return early), so a single call suffices.
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
        not interrupted
        and record == 1
        and manifest_mtime_after != manifest_mtime_before
        and manifest_locked
        and manifest_parses(exe, manifest)
    ):
        write_manifest_stamp(wpt, manifest, tree_before)
    release_lock(venv_lock)
    release_lock(manifest_lock)
    return status


def run_main(options: Options, argv: list[str]) -> int:
    paths, score, dry_run, extras, error = split_run_argv(argv)
    if error:
        return die("run", error, 2)
    if score:
        if dry_run:
            extras = ["--dry-run", *extras]
        return score_main(options, [*paths, *extras])
    if dry_run:
        wpt = resolve_wpt_root(options)
        final = finalize_run_args(wpt, extras)
        paths, final, error = hoist_paths_after_test_types(paths, final)
        if error:
            return die("run", error, 2)
        print(
            f"$ {wpt}/wpt run --binary <tinybrowser> tinybrowser {shlex.join([*paths, *final])}"
        )
        return 0

    wpt = resolve_wpt_root(options)
    if not (wpt / "wpt").is_file():
        eprint(f"WPT checkout not found at {wpt}")
        eprint("Initialize it with: git submodule update --init third_party/wpt")
        eprint("or pass --wpt-root with an initialized checkout.")
        return 1
    wpt = Path(os.path.realpath(wpt))
    if check_pin(wpt, options) != 0:
        return 1

    os.chdir(ROOT)
    binary = build_binary(options)
    if not (binary.is_file() and os.access(binary, os.X_OK)):
        return die("run", f"tinybrowser binary not found at {binary}")
    binary = Path(os.path.realpath(binary))
    os.chdir(wpt)

    venv = Path(options.venv) if options.venv else wpt / "_venv3"
    eprint(f"wpt: {wpt}")
    eprint(f"venv: {venv}")
    eprint(f"binary: {binary}")
    ensure_venv(wpt, venv, options)
    exe = venv_python(venv)

    final = finalize_run_args(wpt, extras)
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
    paths, final, error = hoist_paths_after_test_types(paths, final)
    if error:
        return die("run", error, 2)
    return run_wpt(wpt, venv, exe, binary, [*paths, *final], manifest, record, options)


# --- `manifest` (one walk for a whole chunked CI run) ---


def manifest_main(options: Options, argv: list[str]) -> int:
    """`manifest` — build MANIFEST.json and its stamp once per CI run.

    A chunked CI run has one build job and N chunk jobs; without this,
    every chunk walks the whole test tree to rebuild the manifest. The
    build job runs this and uploads both files; each chunk drops them into
    place, cli.py's staleness check (same HEAD, same clean tree, same
    manifest) passes --no-manifest-update, and the walk happens once. The
    stamp is only an optimization: a stamp that does not match makes
    chunks walk exactly as they did before.
    """
    if argv:
        return die("manifest", "manifest takes no arguments", 2)
    wpt = resolve_wpt_root(options)
    if not (wpt / "wpt").is_file():
        eprint(f"WPT checkout not found at {wpt}")
        eprint("Initialize it with: git submodule update --init third_party/wpt")
        eprint("or pass --wpt-root with an initialized checkout.")
        return 1
    wpt = Path(os.path.realpath(wpt))
    if check_pin(wpt, options) != 0:
        return 1
    venv = Path(options.venv) if options.venv else wpt / "_venv3"
    eprint(f"wpt: {wpt}")
    eprint(f"venv: {venv}")
    ensure_venv(wpt, venv, options)
    exe = venv_python(venv)
    os.chdir(wpt)
    child = [*inner_flags(wpt, options), "--"]
    try:
        os.execv(str(exe), [str(exe), str(CLI), "_manifest", *child])
    except OSError as error:
        return die("manifest", f"cannot exec {exe}: {error}")
    raise AssertionError("unreachable")


def manifest_inner_main(options: Options, argv: list[str]) -> int:
    """The `manifest` body, under the venv python (manifest has its own
    requirements). The tree snapshot is taken before the walk: the stamp
    must describe the tree the walk saw, not what the run left behind."""
    if argv[:1] == ["--"]:
        argv = argv[1:]
    if argv:
        return die("manifest", "manifest takes no arguments", 2)
    wpt = resolve_wpt_root(options)
    manifest = wpt / "MANIFEST.json"
    tree_before = tree_state(wpt)
    try:
        sys.path.insert(0, str(wpt))
        # Upstream's bootstrap puts tools/third_party (atomicwrites, attrs,
        # ...) on sys.path; the manifest package imports from there.
        from tools import localpaths  # noqa: F401
        from tools.manifest import manifest as manifest_pkg
    except ImportError as error:
        return die("manifest", f"cannot import the manifest package: {error}", 1)

    manifest_pkg.load_and_update(str(wpt), str(manifest), "/")
    if not manifest_parses(Path(sys.executable), manifest):
        return die("manifest", f"{manifest} does not parse", 1)
    write_manifest_stamp(wpt, manifest, tree_before)
    eprint(f"wpt: manifest {manifest} ({manifest.stat().st_size} bytes)")
    return 0


# --- `score` ---


def score_run(
    paths: list[str], extra: list[str], save_report: Path | None, options: Options
) -> tuple[Path, int]:
    if save_report is not None:
        report = save_report
        report.parent.mkdir(parents=True, exist_ok=True)
    else:
        handle, name = tempfile.mkstemp(prefix="wpt-report-", suffix=".json")
        os.close(handle)
        report = Path(name)
    command = [
        sys.executable,
        str(CLI),
        "run",
        *paths,
        *options.flags(),
        "--log-wptreport",
        str(report),
        "--no-fail-on-unexpected",
        *extra,
    ]
    eprint(f"$ {shlex.join(command)}")
    exit_code = subprocess.run(command, cwd=str(ROOT), stdout=sys.stderr).returncode
    if exit_code != 0:
        eprint(f"runner exited {exit_code}")
    return report, exit_code


def finish_scored_run(
    report_path: Path, runner_exit: int, save_report: Path | None, started: float
) -> int:
    """Shared score/overnight tail: keep-or-delete, summarize, wall time."""
    keep_report = runner_exit != 0 or save_report is not None
    try:
        if runner_exit != 0:
            eprint(
                f"runner failed with exit {runner_exit}; report kept at {report_path}"
            )
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


def score_main(options: Options, argv: list[str]) -> int:
    report, save_report, paths, extra, dry_run, error = split_score_argv(argv)
    if error:
        return die("run --score", error, 2)
    if report is not None and save_report is not None:
        return die(
            "run --score",
            "--report summarizes an existing report; do not combine it with --save-report",
            2,
        )
    if has_option("--skip-timeout", extra):
        return die(
            "run --score",
            "--skip-timeout is a dev-loop flag for `run`; a score must run expected TIMEOUTs",
            2,
        )
    if report is not None:
        if paths or extra or dry_run:
            return die(
                "run --score",
                "--report summarizes an existing report; give no test paths",
                2,
            )
        return summarize(report)
    if dry_run:
        eprint(
            f"$ {CLI} run {shlex.join([*paths, '--log-wptreport', '<tmp>', '--no-fail-on-unexpected', *extra])}"
        )
        return 0
    if not paths:
        return die("run --score", "give test paths or --report FILE", 2)
    started = time.monotonic()
    try:
        report_path, runner_exit = score_run(paths, extra, save_report, options)
    except OSError as error:
        return die("run --score", f"could not start the runner: {error}")
    return finish_scored_run(report_path, runner_exit, save_report, started)


# --- `rerun` ---


def rerun_spawn(
    tests: list[str], extra: list[str], save_report: Path | None, options: Options
) -> tuple[Path, int]:
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
        sys.executable,
        str(CLI),
        "run",
        "--include-file",
        str(include_file),
        *options.flags(),
        "--log-wptreport",
        str(report),
        "--no-fail-on-unexpected",
        *extra,
    ]
    eprint(f"$ {shlex.join(command)}")
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


def rerun_main(options: Options, argv: list[str]) -> int:
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
        report_path, runner_exit = rerun_spawn(tests, extra, save_report, options)
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


def overnight_main(options: Options, argv: list[str]) -> int:
    save_report: Path | None = None
    extra: list[str] = []
    dry_run = False
    i = 0
    rest = list(argv)
    while i < len(rest):
        arg = rest[i]
        if arg == "--":
            extra = rest[i + 1 :]
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
    if has_option("--skip-timeout", extra):
        return die(
            "overnight",
            "--skip-timeout is a dev-loop flag for `run`; a score must run expected TIMEOUTs",
            2,
        )
    if dry_run:
        eprint(
            f"$ {CLI} run --log-wptreport <report> --no-fail-on-unexpected {shlex.join(extra)}"
        )
        return 0
    started = time.monotonic()
    try:
        report_path, runner_exit = score_run([], extra, save_report, options)
    except OSError as error:
        return die("overnight", f"could not start the runner: {error}")
    return finish_scored_run(report_path, runner_exit, save_report, started)


# --- `failures` / `baseline` (canonical lists, two-revision diff) ---

# The `▶` blocks the runner prints (mozlog's grouping formatter) are the
# canonical record: the score table truncates its attention list. Headers are
# "STATUS [expected ...] PATH" or "Unexpected (subtest )?result in PATH:".
_FAILURE_BLOCK = "  ▶ "
_FAILURE_UNEXPECTED = re.compile(r"^Unexpected (subtest )?result in ")
_FAILURE_EXPECTED = re.compile(r"^[A-Z][A-Z_-]* \[expected[^\]]*\] ")
_FAILURE_STATUS = re.compile(r"^[A-Z][A-Z_-]* ")
_FAILURE_FIRST_FIELD = re.compile(r" .*")
_FAILURE_SUBTEST = re.compile(r"^  [│└] [A-Z][A-Z_-]*( |$)")
_FAILURE_SUBTEST_LEAD = re.compile(r"^  [│└] +")
_FAILURE_TRAILING_COLON = re.compile(r":$")

FAILURES_USAGE = "failures [LOG...]   (or pipe one log on stdin)"

BASELINE_USAGE = """baseline REV PATH... [-- upstream-flags...]

Run the same WPT slice against REV and the working tree, then diff the two
canonical failure lists. Both runs use this checkout's WPT, venv, and
manifest; only the browser binary differs, so a fresh failure is the working
tree's. REV must pin the same third_party build submodules as this checkout.
The baseline binary is built in a detached worktree with its own target dir,
and that worktree is removed on exit; run logs stay in the temp dir printed.

Exit: 0 no new failures, 1 new failures, 2 usage, setup, or runner error."""


def failure_lines(lines: Iterable[str]) -> list[str]:
    """One canonical line per failing result: `STATUS PATH :: SUBTEST`.

    Top-level results (an ERROR or TIMEOUT with no failing subtests) print as
    `STATUS PATH :: [top-level]`; subtests print their own line. Sorted and
    deduplicated, so two such lists compare with set operations.
    """
    out: list[str] = []
    path = ""
    for line in lines:
        if line.startswith(_FAILURE_BLOCK):
            header = line[len(_FAILURE_BLOCK) :]
            if _FAILURE_UNEXPECTED.match(header):
                path = _FAILURE_UNEXPECTED.sub("", header)
            else:
                top = _FAILURE_FIRST_FIELD.sub("", header)
                path = header
                if _FAILURE_EXPECTED.match(path):
                    path = _FAILURE_EXPECTED.sub("", path)
                else:
                    path = _FAILURE_STATUS.sub("", path)
                out.append(f"{top} {path} :: [top-level]")
            path = _FAILURE_TRAILING_COLON.sub("", path)
        elif _FAILURE_SUBTEST.match(line):
            body = _FAILURE_SUBTEST_LEAD.sub("", line)
            status = _FAILURE_FIRST_FIELD.sub("", body)
            if _FAILURE_EXPECTED.match(body):
                body = _FAILURE_EXPECTED.sub("", body)
            else:
                body = _FAILURE_STATUS.sub("", body)
            if path and body:
                out.append(f"{status} {path} :: {body}")
    return sorted(set(out))


def read_log(path: Path) -> str | None:
    try:
        return path.read_text(errors="replace")
    except OSError as error:
        eprint(f"cannot read {path}: {error}")
        return None


def failures_main(argv: list[str]) -> int:
    if argv and argv[0] in ("-h", "--help"):
        print(FAILURES_USAGE)
        return 0
    if not argv:
        text = sys.stdin.read()
    else:
        parts: list[str] = []
        for name in argv:
            part = read_log(Path(name))
            if part is None:
                return 1
            parts.append(part)
        text = "\n".join(parts)
    for line in failure_lines(text.splitlines()):
        print(line)
    return 0


def _remove_worktree(worktree: Path) -> None:
    subprocess.run(
        ["git", "worktree", "remove", "--force", str(worktree)],
        cwd=str(ROOT),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    if worktree.exists():
        shutil.rmtree(worktree, ignore_errors=True)


def build_baseline_binary(worktree: Path, tmp: Path) -> Path | None:
    """Builds REV's browser in its worktree, returning the copied binary.

    The worktree build gets its own target dir: the machine shares one target
    dir across checkouts (~/.cargo/config.toml), and same-version path
    dependencies from another revision would otherwise poison this checkout's
    fingerprints (e.g. an old webidl-bindgen reused by the new build script).
    """
    env = {**os.environ, "CARGO_TARGET_DIR": str(tmp / "target")}
    cargo_toml = (worktree / "Cargo.toml").read_text(errors="replace")
    if any(line.startswith("[profile.wpt]") for line in cargo_toml.splitlines()):
        profile = ["--profile", "wpt"]
        build = ["cargo", "build", "-q", "--profile", "wpt"]
    else:
        # Older revisions have no `wpt` profile; the override keeps both sides
        # of the diff at the same opt level.
        profile = []
        env["CARGO_PROFILE_DEV_OPT_LEVEL"] = "2"
        build = ["cargo", "build", "-q"]
    if subprocess.run(build, cwd=str(worktree), env=env).returncode != 0:
        return None
    located = subprocess.run(
        [str(ROOT / "tools" / "binary-path"), *profile],
        cwd=str(worktree),
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    source = Path(located.stdout.decode(errors="replace").strip())
    if located.returncode != 0 or not source.is_file():
        return None
    out = tmp / "tinybrowser-baseline"
    shutil.copy2(source, out)
    return out


def baseline_score(
    paths: list[str],
    runner: list[str],
    flags: list[str],
    binary: Path | None,
    log: Path,
) -> int:
    """Runs one side of the diff, streaming everything it prints into `log`."""
    command = [sys.executable, str(CLI), "run", *paths, *flags, "--score"]
    if binary is not None:
        command += ["--binary", str(binary)]
    command += runner
    eprint(f"$ {shlex.join(command)} > {log}")
    with open(log, "w") as handle:
        return subprocess.run(
            command, cwd=str(ROOT), stdout=handle, stderr=subprocess.STDOUT
        ).returncode


def baseline_main(options: Options, argv: list[str]) -> int:
    if argv and argv[0] in ("-h", "--help"):
        print(BASELINE_USAGE)
        return 0
    if len(argv) < 2:
        return die("baseline", BASELINE_USAGE, 2)
    rev, rest = argv[0], argv[1:]
    cut = rest.index("--") if "--" in rest else len(rest)
    paths, runner = rest[:cut], rest[cut:]
    if not paths:
        return die("baseline", "no test paths given", 2)
    if git(["rev-parse", "--verify", "--quiet", f"{rev}^{{commit}}"]) is None:
        return die("baseline", f"{rev} is not a commit", 2)

    tmp = Path(
        tempfile.mkdtemp(
            prefix="tinybrowser-baseline.", dir=os.environ.get("TMPDIR") or "/tmp"
        )
    )
    worktree = tmp / "tree"
    keep_logs = False
    try:
        if git(["worktree", "add", "--detach", str(worktree), rev]) is None:
            return die("baseline", f"cannot create a worktree at {worktree}", 2)
        eprint(f"baseline: {rev} -> {worktree}")
        # Linked worktrees have no submodule checkouts and the network may be
        # unavailable, so copy ours in; WPT stays out (the runner falls back).
        for sub in (
            "third_party/rquickjs",
            "third_party/blitz",
            "third_party/html5ever",
        ):
            pinned = git(["rev-parse", f"{rev}:{sub}"])
            actual = git(["rev-parse", "HEAD"], cwd=ROOT / sub)
            if pinned is None or pinned != actual:
                eprint(
                    f"baseline: {rev} pins {sub} at {(pinned or 'nothing')[:12]}, "
                    f"this checkout has {(actual or 'nothing')[:12]}"
                )
                eprint("baseline: update that submodule to the pinned commit and retry")
                return 2
            dest = worktree / sub
            shutil.rmtree(dest, ignore_errors=True)
            shutil.copytree(ROOT / sub, dest, symlinks=True)
        eprint(f"baseline: building {rev}")
        baseline_bin = build_baseline_binary(worktree, tmp)
        if baseline_bin is None:
            return die("baseline", f"could not build {rev}", 2)

        flags = replace(options, binary=None).flags()
        baseline_log = tmp / "baseline.log"
        current_log = tmp / "current.log"
        eprint(f"baseline: running the slice at {rev}")
        base_status = baseline_score(paths, runner, flags, baseline_bin, baseline_log)
        eprint("baseline: running the slice at the working tree")
        current_status = baseline_score(paths, runner, flags, None, current_log)
        keep_logs = True
        if base_status != 0 or current_status != 0:
            eprint(
                f"baseline: runner failed (baseline {base_status}, "
                f"working tree {current_status}); logs: {tmp}"
            )
            return 2

        base_failures = failure_lines((read_log(baseline_log) or "").splitlines())
        current_failures = failure_lines((read_log(current_log) or "").splitlines())
        new = sorted(set(current_failures) - set(base_failures))
        fixed = sorted(set(base_failures) - set(current_failures))
        print(f"baseline {rev}: {len(base_failures)} failing results")
        print(f"working tree:   {len(current_failures)} failing results")
        if fixed:
            print()
            print("only in baseline (fixed):")
            for line in fixed:
                print(f"  {line}")
        if new:
            print()
            print("only in the working tree (new failures):")
            for line in new:
                print(f"  {line}")
            print()
            print(f"logs: {tmp}")
            return 1
        print()
        print(f"no new failures; logs: {tmp}")
        return 0
    finally:
        _remove_worktree(worktree)
        if not keep_logs:
            shutil.rmtree(tmp, ignore_errors=True)


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
    assert is_worker_variant_url(
        "/html/canvas/offscreen/layers/2d.layer.clearRect.full.w.html"
    )
    assert is_worker_variant_url(
        "/html/canvas/offscreen/layers/2d.layer.clearRect.full.w.html?foo=1"
    )
    assert not is_worker_variant_url(
        "/html/canvas/offscreen/layers/2d.layer.clearRect.full.html"
    )
    assert is_worker_variant_url(
        "/referrer-policy/gen/top.http-rp/unset/worker-classic.http.html"
    )
    assert is_worker_variant_url(
        "/referrer-policy/gen/top.http-rp/unset/sharedworker-module.https.sub.html"
    )
    assert not is_worker_variant_url("/workers/Worker-basic.html")
    assert not is_worker_variant_url("/dom/events/Event-isTrusted.any.html")
    assert not is_worker_variant_url("/dom/events/Event-isTrusted.any.html?foo=1")
    assert not is_worker_variant_url("/dom/events/Event-dispatch-click.tentative.html")

    assert has_option("--processes", ["--processes", "4"])
    assert has_option("--processes", ["--processes=4"])
    assert has_option("--processes", ["--proc", "4"])
    assert not has_option("--processes", ["--pause-after-test"])
    assert has_option_exact("--manifest", ["--manifest", "x"])
    assert not has_option_exact("--manifest", ["--manif", "x"])

    # Harness flags are ours before `--`, upstream's after it.
    opts, rest, err = extract_options(
        [
            "run",
            "dom/",
            "--binary",
            "/x",
            "--durations=",
            "--no-lock",
            "--",
            "--binary",
            "wpt-value",
        ]
    )
    assert err is None, err
    assert rest == ["run", "dom/", "--", "--binary", "wpt-value"]
    assert opts.binary == "/x" and opts.durations == "" and opts.no_lock
    assert extract_options(["--binary"])[2] is not None
    assert extract_options(["--binary="])[2] is not None
    # A value never starts with `-`, so a missing one cannot swallow a flag.
    assert extract_options(["--binary", "--log-level=trace"])[2] is not None
    assert extract_options(["--log-level="])[2] is not None
    assert extract_options(["--no-lock=1"])[2] is not None
    assert extract_options(["--lock-timeout", "soon"])[2] is not None
    assert extract_options(["--lock-timeout", "-1"])[2] is not None
    assert extract_options([])[0] == Options()
    # Round trip: the flags a parent hands its child rebuild the same options.
    chosen = Options(
        binary="/x",
        wpt_root="/w",
        venv="/v",
        durations="/d",
        log_level="trace",
        log_file="/log",
        allow_pin_mismatch=True,
        no_lock=True,
        lock_timeout=7,
    )
    assert extract_options(chosen.flags())[0] == chosen

    paths, score, dry, extras, err = split_run_argv(["dom/nodes/", "--exclude=worker"])
    assert (
        err is None
        and paths == ["dom/nodes/"]
        and extras == ["--exclude=worker"]
        and not score
        and not dry
    )
    paths, score, dry, extras, err = split_run_argv(
        ["--score", "dom/", "--", "--processes", "4"]
    )
    assert (
        err is None and score and paths == ["dom/"] and extras == ["--processes", "4"]
    )
    paths, score, dry, extras, err = split_run_argv(
        ["--test-types", "reftest", "css/", "--", "--processes", "2"]
    )
    assert (
        err is None
        and paths == ["css/"]
        and extras == ["--test-types", "reftest", "--processes", "2"]
    )
    _, _, _, _, err = split_run_argv(["--test-types", "testharnes", "dom/"])
    assert err is not None

    # A positional trailing --test-types values in the verbatim region is
    # rescued, not swallowed (nargs='*'); a typeless word fails loudly.
    paths, extras, err = hoist_paths_after_test_types(
        [], ["--processes", "4", "--test-types", "reftest", "dom/nodes/"]
    )
    assert (
        err is None
        and paths == ["dom/nodes/"]
        and extras == ["--processes", "4", "--test-types", "reftest"]
    )
    _, _, err = hoist_paths_after_test_types([], ["--test-types", "testharnes", "dom/"])
    assert err is not None  # typo fails loudly instead of running the wrong set
    _, _, err = hoist_paths_after_test_types([], ["--test-types", "testharnes"])
    assert err is not None

    assert group_of("/dom/nodes/Node-childNodes.html") == "dom/nodes"
    assert group_of("/FileAPI/Blob.html") == "FileAPI"
    assert group_of("/foo.html") == "(root)"

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
    assert rows["html"]["unexpected"] == 1 and attention == [
        ("/html/foo.html", "", "PASS")
    ]
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
    assert rows["dom"]["unexpected"] == 1 and attention == [
        ("/dom/bar.html", "b", "PASS")
    ]
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
    assert rows["dom"]["pass"] == 1 and attention == []
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
    assert rows["dom"]["expected_fail"] == 1 and attention == []

    ok = {"test": "/dom/ok.html", "status": "PASS"}
    skip = {
        "test": "/wasm/x.any.js",
        "status": "SKIP",
        "message": "does not support jsshell",
    }
    timeout = {"test": "/WebCryptoAPI/x.html", "status": "TIMEOUT"}
    error = {"test": "/html/y.html", "status": "ERROR", "expected": "OK"}
    failed = {"test": "/FileAPI/z.html", "status": "FAIL", "expected": "PASS"}
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
    assert rerun_status(0, 0) == (0, False)
    assert rerun_status(0, 64) == (64, True)
    assert rerun_status(2, 0) == (1, True)

    assert longest_first({"a": 60, "b": 10, "c": 10, "d": 10, "e": 30, "f": 30}, 2) == [
        ["a", "b", "d"],
        ["e", "f", "c"],
    ]
    assert longest_first({"a": 1}, 3) == [["a"], [], []]
    recorder = DurationRecorder()
    recorder({"action": "test_start", "test": "/a.html", "time": 1000})
    recorder({"action": "test_end", "test": "/a.html", "time": 1250})
    recorder({"action": "test_end", "test": "/b.html", "time": 1300})
    assert recorder.durations == {"/a.html": 250}

    report, save, paths, extra, dry, err = split_score_argv(
        ["FileAPI/", "--save-report", "/tmp/x.json", "--", "--exclude=worker"]
    )
    assert (
        report is None
        and save == Path("/tmp/x.json")
        and paths == ["FileAPI/"]
        and extra == ["--exclude=worker"]
        and not dry
        and err is None
    )
    report, save, paths, extra, dry, err = split_score_argv(["--dry-run", "dom/"])
    assert dry and paths == ["dom/"] and err is None
    r, _, _, _, _, err = parse_rerun_argv(
        ["r.json", "--include-timeout", "--", "--processes", "8"]
    )
    assert r == Path("r.json") and err is None
    assert parse_rerun_argv([])[5] is not None

    reports, out, strict, err = parse_merge_argv(
        ["a.json", "b.json", "--out", "m.json", "--strict-skips"]
    )
    assert (
        reports == [Path("a.json"), Path("b.json")]
        and out == Path("m.json")
        and strict
        and err is None
    )
    assert parse_merge_argv(["a.json", "b.json"])[1] is None
    assert parse_merge_argv(["a.json"])[3] is None  # one report is a valid merge
    assert parse_merge_argv([])[3] is not None
    assert parse_merge_argv(["a.json", "b.json", "--out"])[3] is not None
    assert parse_merge_argv(["a.json", "b.json", "--total-chunks", "2"])[3] is not None

    _selftest_profile()
    _selftest_failures()
    _selftest_baseline()
    _selftest_merge()


def _selftest_profile() -> None:
    """profile: ranks a report by wall time, and reads the browser log it asked for."""
    with tempfile.TemporaryDirectory() as tmp:
        report = Path(tmp) / "r.json"
        report.write_text(
            json.dumps(
                {
                    "results": [
                        {"test": "/a.html", "status": "PASS", "duration": 100},
                        {"test": "/b.html", "status": "TIMEOUT", "duration": 7192},
                        {"test": "/c.html", "status": "PASS", "duration": 2600},
                    ]
                }
            )
        )
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            assert profile_main(Options(), ["--report", str(report), "--top=2"]) == 0
        text = stdout.getvalue()
        assert text.index("/b.html") < text.index("/c.html")  # longest first
        assert "/a.html" not in text  # --top=2 drops it
        assert "1 killed" in text and "1 passing within 2500ms" in text

        # A kept log prints only its browser::net records.
        log = Path(tmp) / "default.log"
        log.write_text(
            "2026-10-17T08:36:48.751Z TRACE browser::net nav start http://x/\n"
            "2026-10-17T08:36:48.752Z INFO renderer ready\n"
            "2026-10-17T08:36:48.772Z TRACE browser::net nav head http://x/ -> 200\n"
        )
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            print_timeline(log)
        text = stdout.getvalue()
        assert "nav start" in text and "nav head" in text
        assert "renderer ready" not in text

        # Malformed invocations fail; paths and --report are exclusive.
        for bad in (
            ["--top", "x", "--report", str(report)],
            ["--top=0", "--report", str(report)],
            ["--report"],
            [],
            ["--report", str(report), "dom/"],
            ["--bogus"],
        ):
            with (
                contextlib.redirect_stderr(io.StringIO()),
                contextlib.redirect_stdout(io.StringIO()),
            ):
                assert profile_main(Options(), bad) == 2, bad


def _selftest_failures() -> None:
    """failures: the canonical list is a pure fold over the runner's blocks."""
    log = (
        "  ▶ ERROR [expected OK] /a/top.html\n"
        "  ▶ TIMEOUT [expected OK] /b/timeout.html\n"
        "  ▶ NOTRUN /c/notrun.html\n"
        "  ▶ Unexpected subtest result in /d/sub.html:\n"
        "  │ FAIL [expected PASS] first subtest\n"
        "  │   → assert_equals: expected 1 but got 2\n"
        "  │ \n"
        "  │     at foo (eval:1:1)\n"
        "  └     at bar (eval:2:2)\n"
        "  ▶ Unexpected subtest result in /e/multi.html:\n"
        "  │ TIMEOUT subtest with status only\n"
        "  ▶ Unexpected subtest result in /d/sub.html:\n"
        "  │ FAIL [expected PASS] first subtest\n"
    )
    assert failure_lines(log.splitlines()) == [
        "ERROR /a/top.html :: [top-level]",
        "FAIL /d/sub.html :: first subtest",
        "NOTRUN /c/notrun.html :: [top-level]",
        "TIMEOUT /b/timeout.html :: [top-level]",
        "TIMEOUT /e/multi.html :: subtest with status only",
    ]
    # No block header yet: a stray subtest has no path to belong to.
    assert failure_lines(["noise", "  │ FAIL [expected PASS] x"]) == []
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "score.log"
        path.write_text(log)
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            assert failures_main([str(path)]) == 0
        assert "ERROR /a/top.html :: [top-level]" in stdout.getvalue()
        with contextlib.redirect_stdout(io.StringIO()):
            assert failures_main(["--help"]) == 0
        with (
            contextlib.redirect_stderr(io.StringIO()),
            contextlib.redirect_stdout(io.StringIO()),
        ):
            assert failures_main([str(Path(tmp) / "missing.log")]) == 1


def _selftest_baseline() -> None:
    """baseline: the argument grammar rejects before it touches the tree."""
    with contextlib.redirect_stdout(io.StringIO()):
        assert baseline_main(Options(), ["--help"]) == 0
    with (
        contextlib.redirect_stderr(io.StringIO()),
        contextlib.redirect_stdout(io.StringIO()),
    ):
        assert baseline_main(Options(), []) == 2
        assert baseline_main(Options(), ["d78b30e"]) == 2
        assert baseline_main(Options(), ["d78b30e", "--"]) == 2
        assert baseline_main(Options(), ["not-a-commit", "dom/"]) == 2


def _selftest_merge() -> None:
    """Fabricated chunk reports: merge survives the failure modes CI has."""
    info = {"product": "tinybrowser", "os": "linux"}

    def chunk(results, **top):
        data = {"results": results}
        data.update(top)
        return json.dumps(data)

    def result(test, subsuite=None):
        out = {"test": test, "status": "PASS", "subtests": []}
        if subsuite is not None:
            out["subsuite"] = subsuite
        return out

    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        a, b = tmp / "a.json", tmp / "b.json"
        corrupt, empty = tmp / "corrupt.json", tmp / "empty.json"
        a.write_text(
            chunk(
                [result("/b.html"), result("/a.html")],
                run_info=info,
                time_start=10,
                time_end=20,
            )
        )
        b.write_text(
            chunk([result("/c.html")], run_info=info, time_start=5, time_end=30)
        )
        corrupt.write_text('{"results": [')
        empty.write_text(chunk([]))

        merged_path = tmp / "merged.json"
        stdout, stderr = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            assert merge_main([str(a), str(b), "--out", str(merged_path)]) == 0
        merged = json.loads(merged_path.read_text())
        assert [r["test"] for r in merged["results"]] == [
            "/a.html",
            "/b.html",
            "/c.html",
        ]
        assert merged["run_info"] == info
        assert merged["time_start"] == 5 and merged["time_end"] == 30

        # A dead chunk is skipped, the rest still scores.
        stdout, stderr = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            assert merge_main([str(corrupt), str(empty), str(a)]) == 0
        assert "skipping" in stderr.getvalue()
        assert "no results in" in stderr.getvalue()
        assert "TOTAL" in stdout.getvalue()

        # --strict-skips: the table still prints, the merge still fails.
        stdout, stderr = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            assert merge_main(["--strict-skips", str(corrupt), str(a)]) == 2
        assert "TOTAL" in stdout.getvalue()
        with (
            contextlib.redirect_stdout(io.StringIO()),
            contextlib.redirect_stderr(io.StringIO()),
        ):
            assert merge_main(["--strict-skips", str(a)]) == 0

        # A duplicate (subsuite, test) is the only hard error; inside one
        # report it says "twice", across reports it names both.
        dup = tmp / "dup.json"
        dup.write_text(chunk([result("/a.html")]))
        stderr = io.StringIO()
        with (
            contextlib.redirect_stderr(stderr),
            contextlib.redirect_stdout(io.StringIO()),
        ):
            assert merge_main([str(a), str(dup)]) == 2
        assert f"/a.html appears in {a} and {dup}" in stderr.getvalue()
        twice = tmp / "twice.json"
        twice.write_text(chunk([result("/a.html"), result("/a.html")]))
        stderr = io.StringIO()
        with (
            contextlib.redirect_stderr(stderr),
            contextlib.redirect_stdout(io.StringIO()),
        ):
            assert merge_main([str(twice)]) == 2
        assert f"/a.html appears twice in {twice}" in stderr.getvalue()
        one, two = tmp / "one.json", tmp / "two.json"
        one.write_text(chunk([result("/a.html", subsuite="one")]))
        two.write_text(chunk([result("/a.html", subsuite="two")]))
        with contextlib.redirect_stdout(io.StringIO()):
            assert merge_main([str(one), str(two)]) == 0

        # Nothing usable is an error; a lone empty report has no table.
        with (
            contextlib.redirect_stderr(io.StringIO()),
            contextlib.redirect_stdout(io.StringIO()),
        ):
            assert merge_main([str(corrupt)]) == 2
            assert merge_main([str(empty)]) == 1


TOP_HELP = """tinybrowser WPT harness: run | score | rerun | merge | profile | failures | baseline | manifest | overnight | selftest

  run PATH... [--score] [--dry-run] [-- upstream-flags...]
    Run WPT. Upstream flags follow `--` (or start implicitly at the first
    `-flag`). --score runs then prints the per-directory table.
    --dry-run prints the wpt invocation without building or running.
  score PATH... [--report FILE | --save-report FILE] [-- upstream-flags...]
    Summarize an existing report (--report) or run then summarize.
  rerun REPORT [--include-timeout] [--dry-run] [--save-report FILE] [-- runner...]
    Re-run only the attention set from a wptreport.
  merge REPORT... [--out FILE] [--strict-skips]
    Fold the chunk reports of one CI run into a single score. Reports that
    are missing, corrupt, or empty are skipped with a warning; overlapping
    tests are refused. --out writes the merged wptreport, keeping run_info,
    subsuites, and the time bounds. --strict-skips exits 2 after printing
    the table when any report was unusable, so a dead chunk still fails CI.
  profile PATH... [--top N]
  profile --report FILE [--top N]
    Rank tests by wall time. With paths it runs them at trace with the
    browser log at a path it picks, then prints the navigation/request
    timeline. With --report it ranks one existing wptreport instead.
    The timeline separates wait-bound from compute-bound: requests that all
    end in milliseconds while the test still times out mean the renderer is
    spinning, not the network. Records sit at the browser's seams, never in
    the renderer crate (that crate is shared with wasm and carries no logger).
  failures [LOG...]
    Print the canonical failure list of a `--score` log, one
    `STATUS PATH :: SUBTEST` line per failing result, sorted and deduplicated.
    Reads stdin when given no logs.
  baseline REV PATH... [-- upstream-flags...]
    Run the slice at REV and in the working tree and diff the failure lists;
    exit 1 on a failure the working tree adds. The baseline browser is built
    in a detached worktree that is removed on exit.
  manifest
    Build MANIFEST.json plus its stamp once per chunked CI run, so chunk
    jobs can skip the tree walk. The stamp only ever makes runs faster.
  overnight [--save-report FILE] [--dry-run] [-- runner...]
    Full-suite run + score. Long; needs explicit approval per AGENTS.md.
  selftest
    Pure-function checks (worker filter, classify, arg grammars).

Harness flags (any subcommand, before `--`):
  --binary PATH          use a prebuilt browser binary as-is
  --wpt-root PATH        WPT checkout to use
  --venv PATH            WPT virtualenv to use
  --durations FILE       per-test durations for the longest-first split;
                         `--durations=` disables the split
  --log-level LEVEL      browser log level (e.g. trace)
  --log-file PATH        write the browser log here, not under the data home
  --allow-pin-mismatch   run even when the checkout drifts from the pin
  --no-lock              skip the shared venv/manifest locks
  --lock-timeout SECONDS how long to wait for a held lock

Examples:
  cli.py run dom/events/ --exclude=worker
  cli.py run --score dom/nodes/ -- --processes 4
  cli.py rerun /tmp/fileapi.json -- --processes 8 --fully-parallel
  cli.py merge reports/*.json --out merged.json
  cli.py profile referrer-policy/gen/req.rel-noref/no-referrer/a-tag.http.html
  cli.py baseline d78b30e dom/nodes/Document-adoptNode.html
"""


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    if argv == ["--selftest"]:
        argv = ["selftest"]
    if not argv or argv[0] in ("-h", "--help", "help"):
        print(TOP_HELP)
        return 0
    options, rest, error = extract_options(argv)
    if error:
        return die("wpt", error, 2)
    if not rest or rest[0] in ("-h", "--help", "help"):
        print(TOP_HELP)
        return 0
    command, args = rest[0], rest[1:]
    if command == "run":
        return run_main(options, args)
    if command == "score":
        return score_main(options, args)
    if command in ("rerun", "retest"):
        return rerun_main(options, args)
    if command == "merge":
        return merge_main(args)
    if command == "profile":
        return profile_main(options, args)
    if command == "failures":
        return failures_main(args)
    if command == "baseline":
        return baseline_main(options, args)
    if command == "manifest":
        return manifest_main(options, args)
    if command == "overnight":
        return overnight_main(options, args)
    if command == "_inner":
        return inner_main(options, args)
    if command == "_manifest":
        return manifest_inner_main(options, args)
    if command == "selftest":
        _selftest()
        eprint("selftest: ok")
        return 0
    return die(
        "wpt",
        f"unknown command {command}; expected run | score | rerun | merge | profile | failures | baseline | manifest | overnight | selftest",
        2,
    )


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except BrokenPipeError:
        devnull = os.open(os.devnull, os.O_WRONLY)
        os.dup2(devnull, sys.stdout.fileno())
        raise SystemExit(0)
