#!/usr/bin/env python3
"""Run `wpt run` for tinybrowser without /etc/hosts or a WPT submodule patch."""

from __future__ import annotations

import sys
from pathlib import Path

TLS_SCHEMES = ("https", "https-local", "https-public", "wss", "h2")
# HTTP testharness only: extra schemes bind extra loopbacks or start DNS.
HTTP_ONLY = ("http",)


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


def main() -> object:
    root = Path(__file__).resolve().parents[2]
    wpt_root = root / "third_party" / "wpt"
    patch_wpt(wpt_root)
    from tools.wpt import wpt

    return wpt.main(prog=str(wpt_root / "wpt"), argv=["run", *sys.argv[1:]])


if __name__ == "__main__":
    raise SystemExit(main())
