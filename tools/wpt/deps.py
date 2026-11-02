#!/usr/bin/env python3
"""Build the test-to-source map that `wpt-harness --deps` selects from.

The harness runs with `-C instrument-coverage` and each test runs in its own
process with `LLVM_PROFILE_FILE` pointing at that test's profile. The profile
is turned into an lcov report, and every repository file with a non-zero line
count becomes one of the test's inputs.

    nix develop --command python3 tools/wpt/deps.py build \
        --wpt-root ../projects/tinybrowser/third_party/wpt \
        --binary /mnt/ssd/rust-targets/.../wpt-harness \
        --out tools/wpt/deps.json dom/nodes --jobs 12

Cost is the reason this is not wired into a per-push job yet: coverage
instrumentation makes a file that normally takes ~80 ms take ~23 s on this
engine, so a full map is many hours. The selection side is complete and
tested; see tools/wpt/README.md for the numbers.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
VERSION = 1


def rust_llvm_tools() -> Path:
    """The LLVM tools shipped with the pinned rustup toolchain."""
    toolchain = Path(
        subprocess.run(
            ["rustup", "show", "active-toolchain"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.split()[0]
    )
    home = Path(os.environ.get("RUSTUP_HOME", Path.home() / ".rustup"))
    target = subprocess.run(
        ["rustc", "-vV"], check=True, capture_output=True, text=True
    ).stdout
    host = next(
        line.split(":", 1)[1].strip()
        for line in target.splitlines()
        if line.startswith("host:")
    )
    return home / "toolchains" / toolchain / "lib" / "rustlib" / host / "bin"


def list_tests(binary: Path, wpt: Path, paths: list[str]) -> list[tuple[str, str]]:
    result = subprocess.run(
        [str(binary), "--wpt-root", str(wpt), *paths, "--list"],
        check=True,
        capture_output=True,
        text=True,
    )
    tests = []
    for line in result.stdout.splitlines():
        kind, _, relative = line.partition("\t")
        if kind in ("Testharness", "Crashtest"):
            tests.append((kind, relative))
    return tests


def profile_name(relative: str) -> str:
    """A unique file name for one test's raw profile."""
    safe = "".join(
        character if character.isalnum() or character in ".-" else "_"
        for character in relative
    )
    digest = 0xCBF2_9CE4_8422_2325
    for byte in relative.encode():
        digest ^= byte
        digest = (digest * 0x0000_0100_0000_01B3) & 0xFFFF_FFFF_FFFF_FFFF
    return f"{safe}-{digest:016x}.profraw"


def sources_for(
    binary: Path, profraw: Path, tools: Path, scratch: Path
) -> list[str]:
    """Repository files the test executed."""
    merged = scratch / (profraw.stem + ".profdata")
    merge = subprocess.run(
        [str(tools / "llvm-profdata"), "merge", "-sparse", str(profraw), "-o", str(merged)],
        capture_output=True,
        text=True,
    )
    if merge.returncode != 0:
        raise RuntimeError(merge.stderr.strip() or "llvm-profdata failed")
    export = subprocess.run(
        [
            str(tools / "llvm-cov"),
            "export",
            "-format=lcov",
            f"-instr-profile={merged}",
            f"-object={binary}",
        ],
        capture_output=True,
        text=True,
    )
    if export.returncode != 0:
        raise RuntimeError(export.stderr.strip() or "llvm-cov failed")
    files: list[str] = []
    current: str | None = None
    covered = False
    for line in export.stdout.splitlines():
        if line.startswith("SF:"):
            if current is not None and covered:
                files.append(current)
            current = line[3:]
            covered = False
        elif line.startswith("DA:") and current is not None:
            _, _, count = line[3:].partition(",")
            if count.strip().split(",")[0] not in ("", "0"):
                covered = True
    if current is not None and covered:
        files.append(current)
    kept = []
    for path in files:
        try:
            relative = str(Path(path).resolve().relative_to(ROOT))
        except ValueError:
            continue
        if relative.startswith("third_party/wpt/"):
            continue
        kept.append(relative)
    return sorted(set(kept))


def run_batch(
    binary: Path,
    wpt: Path,
    batch: list[tuple[str, str]],
    tools: Path,
    scratch: Path,
) -> dict[str, list[str]]:
    """Records one process's profile and attributes it to each test in it.

    A batch shares one profile, so every test gets the union: a superset of
    what it alone executed. That over-selects, never under-selects. The fixed
    cost of an instrumented process (~19 s here) is paid once per batch
    instead of once per test.
    """
    raw = scratch / profile_name(batch[0][1] + f"-b{len(batch)}")
    env = dict(os.environ, LLVM_PROFILE_FILE=str(raw))
    run = subprocess.run(
        [
            str(binary),
            "--wpt-root",
            str(wpt),
            *[relative for _, relative in batch],
            "--jobs",
            "1",
        ],
        env=env,
        capture_output=True,
        text=True,
    )
    if run.returncode == 0 and raw.exists():
        sources = sources_for(binary, raw, tools, scratch)
        # A raw profile is tens of megabytes; keep only a couple alive.
        raw.unlink(missing_ok=True)
        return {f"/{relative}": sources for _, relative in batch}
    if len(batch) == 1:
        print(f"deps: /{batch[0][1]} produced no profile", file=sys.stderr)
        return {f"/{batch[0][1]}": []}
    # A crash took the whole batch's profile with it; split and retry.
    half = len(batch) // 2
    merged = run_batch(binary, wpt, batch[:half], tools, scratch)
    merged.update(run_batch(binary, wpt, batch[half:], tools, scratch))
    return merged


def build(args: argparse.Namespace) -> int:
    binary = Path(args.binary).resolve()
    wpt = Path(args.wpt_root).resolve()
    tools = Path(args.llvm_tools) if args.llvm_tools else rust_llvm_tools()
    for tool in ("llvm-profdata", "llvm-cov"):
        if not (tools / tool).is_file():
            print(
                f"deps: {tools / tool} is missing; install llvm-tools-preview "
                "for the pinned toolchain",
                file=sys.stderr,
            )
            return 2
    tests = list_tests(binary, wpt, args.paths)
    if not tests:
        print("deps: no testharness or crashtest files selected", file=sys.stderr)
        return 2
    print(f"deps: {len(tests)} tests, {args.jobs} at a time", file=sys.stderr)
    batches = [
        tests[index : index + args.batch]
        for index in range(0, len(tests), args.batch)
    ]
    print(f"deps: {len(batches)} batches of up to {args.batch}", file=sys.stderr)
    with tempfile.TemporaryDirectory(prefix="wpt-deps-") as scratch_dir:
        scratch = Path(scratch_dir)
        tests_map: dict[str, list[str]] = {}
        with concurrent.futures.ThreadPoolExecutor(args.jobs) as pool:
            futures = [
                pool.submit(run_batch, binary, wpt, batch, tools, scratch)
                for batch in batches
            ]
            for index, future in enumerate(
                concurrent.futures.as_completed(futures), 1
            ):
                tests_map.update(future.result())
                if index % 25 == 0:
                    print(f"deps: {index}/{len(batches)} batches", file=sys.stderr)

    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps({"version": VERSION, "tests": tests_map}))
    known = {source for sources in tests_map.values() for source in sources}
    print(
        f"deps: wrote {out} ({len(tests_map)} tests, {len(known)} source files)",
        file=sys.stderr,
    )
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    build_parser = sub.add_parser("build", help="record a map from a coverage run")
    build_parser.add_argument("--wpt-root", required=True)
    build_parser.add_argument("--binary", required=True)
    build_parser.add_argument("--out", required=True)
    build_parser.add_argument("--llvm-tools", default=None)
    build_parser.add_argument("--jobs", type=int, default=os.cpu_count() or 1)
    build_parser.add_argument(
        "--batch",
        type=int,
        default=1,
        help="tests per instrumented process (default 1; larger is far cheaper "
        "and records a superset of each test's sources)",
    )
    build_parser.add_argument("paths", nargs="+")
    args = parser.parse_args()
    if args.command == "build":
        return build(args)
    return 2


if __name__ == "__main__":
    sys.exit(main())
