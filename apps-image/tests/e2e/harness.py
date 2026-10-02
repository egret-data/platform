"""Shared e2e helpers (kept out of conftest.py so tests can import them unambiguously)."""

from __future__ import annotations

import socket
import subprocess
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


def assert_env_has_no_secrets(pid: int, *secrets: str) -> str:
    """Read a live process's environment block as another same-uid process can.

    macOS has no /proc; `ps eww` prints the command line followed by the process's
    environment as the kernel holds it (KERN_PROCARGS2), i.e. the block set at exec,
    which os.environ.pop does not change. A known variable must be visible, so an
    empty read cannot pass as "no secrets".
    """
    out = subprocess.run(
        ["ps", "eww", "-o", "command=", "-p", str(pid)], capture_output=True, text=True, timeout=10, check=True
    ).stdout
    assert "EGRET_URL=http://127.0.0.1:" in out, f"ps could not read pid {pid}'s environment: {out!r}"
    for secret in secrets:
        assert secret not in out, f"secret visible in pid {pid}'s environment"
    return out


def assert_log_has_no_secrets(output: str, mode: str) -> None:
    """Neither token may reach the launcher's stdout/stderr, i.e. the pod log.

    marimo prints its startup URL with ``?access_token=<marimo token>`` unless it runs
    with ``-q``. The log must hold the launcher's own startup line, so an empty or
    missing log cannot pass as "no secrets".
    """
    assert f"egret-apps: marimo {mode} on port" in output, f"launcher log is missing its startup line: {output!r}"
    for name, secret in (("pod token", POD_TOKEN), ("marimo token", MARIMO_TOKEN)):
        assert secret not in output, f"the {name} is in the launcher's log:\n{output}"
    assert "access_token=" not in output, f"a tokenised URL is in the launcher's log:\n{output}"


def recording_shim(tmp: Path) -> tuple[Path, Path]:
    """A venv shim that logs each invocation, then runs the image's shim.

    marimo announces "Using configured venv" through its quiet-gated echo, which the
    launcher's ``-q`` silences, so the kernels' route through the shim is recorded here.
    """
    venv, log = tmp / "recording-shim", tmp / "shim-calls.log"
    (venv / "bin").mkdir(parents=True)
    python = venv / "bin" / "python"
    python.write_text(
        "#!/bin/sh\n"
        f'printf "%s\\n" "$*" >> "{log}"\n'
        f'exec "{ROOT / "venv-shim" / "bin" / "python"}" "$@"\n'
    )
    python.chmod(0o755)
    return venv, log
