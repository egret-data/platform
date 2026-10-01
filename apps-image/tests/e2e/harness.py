"""Shared e2e helpers (kept out of conftest.py so tests can import them unambiguously)."""

from __future__ import annotations

import socket
import time
from contextlib import contextmanager
from pathlib import Path

import app_proxy

ROOT = Path(__file__).resolve().parents[2]
APPS = Path(__file__).resolve().parent / "apps"
EGRET_APPS = ROOT / ".venv" / "bin" / "egret-apps"
MARIMO_TOKEN = "marimo-secret"
POD_TOKEN = "pod-tok"
START_TIMEOUT_S = 60


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def wait_until(predicate, timeout: float, what: str, interval: float = 0.1):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(interval)
    raise AssertionError(f"timed out after {timeout}s waiting for {what}")


@contextmanager
def viewer_page(browser, base: str, viewer: str):
    """A browser context whose requests reach marimo as ``viewer``."""
    context = browser.new_context()
    context.set_default_timeout(30_000)
    context.add_cookies([{"name": app_proxy.COOKIE, "value": viewer, "url": base}])
    try:
        yield context.new_page()
    finally:
        context.close()
