# tinybrowser wptrunner product. Install into the WPT venv:
#   pip install -e tools/wpt
# then:
#   third_party/wpt/wpt run --binary /path/to/tinybrowser --ssl-type none tinybrowser [tests]
# or: ./tools/wpt/run [tests]

from __future__ import annotations

import sys

from wptrunner.browsers.base import WebDriverBrowser, get_timeout_multiplier, require_arg
from wptrunner.executors import executor_kwargs as base_executor_kwargs
from wptrunner.executors.executorwebdriver import (
    WebDriverProtocol,
    WebDriverTestharnessExecutor,
)
from wptrunner.products import Product

__wptrunner__ = {
    "product": "tinybrowser",
    "check_args": "check_args",
    "browser": "TinyBrowser",
    "executor": {
        "testharness": "TinyBrowserTestharnessExecutor",
    },
    "browser_kwargs": "browser_kwargs",
    "executor_kwargs": "executor_kwargs",
    "env_extras": "env_extras",
    "env_options": "env_options",
    "timeout_multiplier": "get_timeout_multiplier",
}


def load():
    return Product._from_dunder_wptrunner(sys.modules[__name__])


def check_args(**kwargs):
    require_arg(kwargs, "binary")


def browser_kwargs(logger, test_type, run_info_data, config, subsuite, **kwargs):
    return {
        "binary": kwargs["binary"],
        "binary_args": kwargs.get("binary_args") or [],
        "webdriver_host": "127.0.0.1",
    }


def executor_kwargs(logger, test_type, test_environment, run_info_data, **kwargs):
    rv = base_executor_kwargs(test_type, test_environment, run_info_data, **kwargs)
    rv["capabilities"] = {}
    return rv


def env_extras(**kwargs):
    return []


def env_options():
    return {"server_host": "127.0.0.1", "bind_address": True}


class TinyBrowser(WebDriverBrowser):
    def __init__(self, logger, binary, webdriver_host="127.0.0.1", binary_args=None, **kwargs):
        args = list(binary_args or [])
        super().__init__(
            logger,
            binary=binary,
            webdriver_binary=binary,
            webdriver_args=args,
            host=webdriver_host,
            supports_pac=False,
            **kwargs,
        )

    def make_command(self):
        return [self.webdriver_binary, f"--webdriver={self.port}"] + self.webdriver_args


class TinyBrowserProtocol(WebDriverProtocol):
    enable_bidi = False


class TinyBrowserTestharnessExecutor(WebDriverTestharnessExecutor):
    supports_testdriver = False
    protocol_cls = TinyBrowserProtocol
