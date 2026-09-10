# tinybrowser wptrunner product. Install into the WPT venv:
#   pip install -e tools/wpt
# then:
#   third_party/wpt/wpt run --binary /path/to/tinybrowser --ssl-type none tinybrowser [tests]
# or: ./tools/wpt/run [tests]
# ./tools/wpt/run skips the /etc/hosts check and this product passes --resolve.
# Each WebDriver endpoint gets a fresh temporary XDG profile.

from __future__ import annotations

import os
import shutil
import sys
import tempfile

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
        self._profile_root = None
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
        self._ensure_profile()
        return [
            self.webdriver_binary,
            f"--webdriver={self.port}",
            "--resolve=nonexistent.*.test=fail",
            "--resolve=*.test=127.0.0.1",
            "--resolve=*.test.=127.0.0.1",
        ] + self.webdriver_args

    def stop(self, force=False):
        try:
            return super().stop(force=force)
        finally:
            self._remove_profile()

    def cleanup(self):
        try:
            super().cleanup()
        finally:
            self._remove_profile()

    def _ensure_profile(self):
        if self._profile_root is not None:
            return
        self._profile_root = tempfile.mkdtemp(prefix="tinybrowser-wpt-")
        runtime = os.path.join(self._profile_root, "run")
        data = os.path.join(self._profile_root, "data")
        os.makedirs(runtime)
        os.makedirs(data)
        self.env["XDG_RUNTIME_DIR"] = runtime
        self.env["XDG_DATA_HOME"] = data

    def _remove_profile(self):
        root = self._profile_root
        self._profile_root = None
        if root is not None:
            shutil.rmtree(root, ignore_errors=True)


class TinyBrowserProtocol(WebDriverProtocol):
    enable_bidi = False


class TinyBrowserTestharnessExecutor(WebDriverTestharnessExecutor):
    supports_testdriver = False
    protocol_cls = TinyBrowserProtocol

    def create_test_window(self, protocol):
        # WebDriverTestharnessExecutor.create_test_window clicks the root
        # element to move focus to the new window. Input is not modeled yet
        # (Perform Actions is unsupported), so skip that click and keep the
        # window's initial about:blank.
        test_window = protocol.base.create_window()
        protocol.base.set_window(test_window)
        protocol.base.execute_script(self.window_loaded_script, asynchronous=True)
        return test_window
