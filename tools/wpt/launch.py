#!/usr/bin/env python3
"""Run `wpt run` for tinybrowser without /etc/hosts or a WPT submodule patch."""

from __future__ import annotations

import os
import re
import sys
from pathlib import Path

TLS_SCHEMES = ("https", "https-local", "https-public", "wss", "h2")
# HTTP testharness only: extra schemes bind extra loopbacks or start DNS.
HTTP_ONLY = ("http", "ws")
# wptrunner `--exclude` is a URL prefix, so `--exclude=worker` misses
# `.any.worker.html` / `.worker.html` variants generated from `.any.js`.
WORKER_VARIANT = re.compile(
    r"\.(any\.)?(sharedworker|serviceworker|worker)(-module)?\.html$"
)


def exclude_skips_worker_variants(exclude: object) -> bool:
    return bool(
        exclude and any(item.strip("/") == "worker" for item in exclude)
    )


def is_worker_variant_url(url: str) -> bool:
    return bool(WORKER_VARIANT.search(url.split("?", 1)[0]))


def selftest_worker_exclude() -> None:
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
    skip = exclude_skips_worker_variants(["worker"])
    kept = "/dom/events/Event-isTrusted.any.html"
    dropped = "/dom/events/Event-isTrusted.any.worker.html"
    assert skip and not is_worker_variant_url(kept)
    assert skip and is_worker_variant_url(dropped)


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
                    tests = {
                        test
                        for test in tests
                        if not is_worker_variant_url(test.url)
                    }
                    if not tests:
                        continue
                yield test_type, test_path, tests

    testloader.TestFilter = TinybrowserTestFilter


def main() -> object:
    root = Path(__file__).resolve().parents[2]
    wpt_root = Path(
        os.environ.get("TINYBROWSER_WPT_ROOT") or root / "third_party" / "wpt"
    )
    patch_wpt(wpt_root)
    from tools.wpt import wpt

    return wpt.main(prog=str(wpt_root / "wpt"), argv=["run", *sys.argv[1:]])


if __name__ == "__main__":
    if sys.argv[1:] == ["--selftest"]:
        selftest_worker_exclude()
        raise SystemExit(0)
    raise SystemExit(main())
