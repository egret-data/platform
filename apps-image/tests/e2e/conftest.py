"""Fixtures for the end-to-end tests: fake egret, the launcher subprocess, the proxy, Chromium."""

from __future__ import annotations

import os
import signal
import subprocess
import threading

import httpx
import pytest
import uvicorn

import app_proxy
import fake_egret
from harness import EGRET_APPS, MARIMO_TOKEN, POD_TOKEN, ROOT, START_TIMEOUT_S, free_port, wait_until


@pytest.fixture
def egret():
    fake_egret.STATE.reset()
    server, port = fake_egret.serve()
    fake_egret.STATE.base_url = f"http://127.0.0.1:{port}"
    fake_egret.STATE.pod_tokens[POD_TOKEN] = {}
    yield fake_egret.STATE
    server.should_exit = True


class Launched:
    def __init__(self, proc: subprocess.Popen, port: int, log: Path) -> None:
        self.proc, self.port, self.log = proc, port, log

    def output(self) -> str:
        return self.log.read_text(errors="replace")


@pytest.fixture
def launch(tmp_path, egret):
    """Start ``egret-apps <mode>``; the whole process group is killed at teardown."""
    started: list[Launched] = []

    def _launch(mode: str, base_url: str, **extra: str) -> Launched:
        port = free_port()
        home = tmp_path / f"home-{mode}"
        home.mkdir(exist_ok=True)
        env = {
            k: v for k, v in os.environ.items()
            if not k.startswith(("PYTEST_", "EGRET_", "MARIMO_", "XDG_")) and k != "APPS_WORKDIR"
        }
        env.update(
            HOME=str(home),
            EGRET_URL=egret.base_url,
            EGRET_POD_TOKEN=POD_TOKEN,
            EGRET_MARIMO_TOKEN=MARIMO_TOKEN,
            EGRET_BASE_URL=base_url,
            APPS_WORKDIR=str(tmp_path / f"work-{mode}"),
            EGRET_APPS_CONFIG=str(ROOT / "config"),
            EGRET_VENV_SHIM=str(ROOT / "venv-shim"),
            EGRET_MARIMO_PORT=str(port),
            EGRET_TEST_INCLUDE_PID="1",  # the gateway records which kernel sent each query
            EGRET_TEST_KEEP_DUMPABLE="1",  # `ps eww` must be able to read the server (see launcher.keep_dumpable)
            PYTEST_CURRENT_TEST="e2e",  # enables the test-only overrides and EGRET_TEST_INCLUDE_PID
        )
        env.update(extra)
        log = tmp_path / f"{mode}.log"
        proc = subprocess.Popen(
            [str(EGRET_APPS), mode], env=env, stdout=log.open("w"), stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        launched = Launched(proc, port, log)
        started.append(launched)

        def ready():
            if proc.poll() is not None:
                raise AssertionError(f"egret-apps {mode} exited {proc.returncode}:\n{launched.output()}")
            try:
                r = httpx.get(
                    f"http://127.0.0.1:{port}{base_url}/health",
                    headers={"authorization": f"Bearer {MARIMO_TOKEN}"}, timeout=2,
                )
            except httpx.TransportError:
                return False
            return r.status_code == 200

        wait_until(ready, START_TIMEOUT_S, f"marimo {mode} to serve")
        return launched

    yield _launch
    for launched in started:
        with_group(launched.proc, signal.SIGTERM)
        try:
            launched.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            with_group(launched.proc, signal.SIGKILL)
            launched.proc.wait(timeout=10)
        with_group(launched.proc, signal.SIGKILL)  # kernels left in the group


def with_group(proc: subprocess.Popen, sig: int) -> None:
    try:
        os.killpg(proc.pid, sig)
    except (ProcessLookupError, PermissionError):
        pass


@pytest.fixture
def proxy():
    """``proxy(upstream_port, {viewer: token}) -> base URL``; stopped at teardown."""
    servers: list[uvicorn.Server] = []

    def _proxy(upstream_port: int, tokens: dict[str, str]) -> str:
        port = free_port()
        server = uvicorn.Server(
            uvicorn.Config(
                app_proxy.make_app(upstream_port, tokens, MARIMO_TOKEN),
                host="127.0.0.1", port=port, log_level="warning", ws="websockets",
            )
        )
        threading.Thread(target=server.run, daemon=True).start()
        servers.append(server)
        wait_until(lambda: server.started, 10, "proxy to start", 0.02)
        return f"http://127.0.0.1:{port}"

    yield _proxy
    for server in servers:
        server.should_exit = True


@pytest.fixture(scope="session")
def browser():
    from playwright.sync_api import sync_playwright

    with sync_playwright() as p:
        b = p.chromium.launch()
        yield b
        b.close()
