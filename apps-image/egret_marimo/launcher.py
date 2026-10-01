"""``egret-apps edit|run``: the pod's entrypoint.

Everything else comes from the environment (set by egret's launcher, P4):

| Variable             | Mode | Meaning                                     |
|----------------------|------|---------------------------------------------|
| ``EGRET_URL``        | both | egret's base URL                            |
| ``EGRET_POD_TOKEN``  | both | the pod token (pod API)                     |
| ``EGRET_MARIMO_TOKEN`` | both | marimo's ``--token-password``             |
| ``EGRET_BASE_URL``   | both | ``/e/<pod>`` or ``/r/<version>``            |
| ``APPS_WORKDIR``     | both | working directory (the image sets ``/work``) |
| ``EGRET_APP_ID``     | run  | the app                                     |
| ``EGRET_VERSION_ID`` | run  | the pinned version                          |

Image paths, overridable for tests: ``EGRET_APPS_CONFIG`` (the locked marimo
config, default ``/opt/egret-apps/config``), ``EGRET_VENV_SHIM`` (default
``/opt/egret-apps/venv-shim``) and ``EGRET_MARIMO_PORT`` (default 2718 for
edit, 2719 for run).

Exit codes: 64 bad usage or a missing/invalid variable; 65 the locked
configuration could be shadowed or is missing; 69 egret could not serve the
version source.
"""

from __future__ import annotations

import os
import re
import sys
from pathlib import Path

from .client import GatewayError
from .patches import install_run_mode_ipc_patch
from .podapi import PodApi
from .storage import install_storage_patch

EX_USAGE = 64
EX_DATAERR = 65
EX_UNAVAILABLE = 69

DEFAULT_CONFIG = "/opt/egret-apps/config"
DEFAULT_VENV_SHIM = "/opt/egret-apps/venv-shim"
PORTS = {"edit": "2718", "run": "2719"}

REQUIRED = {
    "edit": ["EGRET_URL", "EGRET_POD_TOKEN", "EGRET_MARIMO_TOKEN", "EGRET_BASE_URL", "APPS_WORKDIR"],
    "run": [
        "EGRET_URL", "EGRET_POD_TOKEN", "EGRET_MARIMO_TOKEN", "EGRET_BASE_URL", "APPS_WORKDIR",
        "EGRET_APP_ID", "EGRET_VERSION_ID",
    ],
}
# Secrets the kernels must not inherit: kernels copy os.environ, and they run app code.
SCRUBBED = ("EGRET_POD_TOKEN", "EGRET_MARIMO_TOKEN")

_UUID = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$", re.IGNORECASE)


class LaunchError(Exception):
    def __init__(self, code: int, message: str) -> None:
        super().__init__(message)
        self.code = code


def shadowing_config(workdir: Path) -> Path | None:
    """A ``.marimo.toml`` marimo would read instead of the locked config, if any.

    marimo takes the first ``.marimo.toml`` walking up from the cwd (the
    launcher chdirs to the workdir), then ``$HOME``, before ``XDG_CONFIG_HOME``.
    """
    for directory in (workdir, *workdir.parents, Path.home()):
        candidate = directory / ".marimo.toml"
        if candidate.exists():
            return candidate
    return None


def _check_env(mode: str) -> dict[str, str]:
    missing = [name for name in REQUIRED[mode] if not os.environ.get(name)]
    if missing:
        raise LaunchError(EX_USAGE, f"missing required environment variable(s): {', '.join(missing)}")
    env = {name: os.environ[name] for name in REQUIRED[mode]}
    if mode == "run" and not _UUID.match(env["EGRET_APP_ID"]):
        raise LaunchError(EX_USAGE, f"EGRET_APP_ID is not an app id: {env['EGRET_APP_ID']!r}")
    return env


def _lock_config(workdir: Path) -> None:
    shadow = shadowing_config(workdir)
    if shadow is not None:
        raise LaunchError(EX_DATAERR, f"refusing to start: {shadow} would override the locked marimo config")
    config = Path(os.environ.get("EGRET_APPS_CONFIG", DEFAULT_CONFIG))
    if not (config / "marimo" / "marimo.toml").is_file():
        raise LaunchError(EX_DATAERR, f"locked marimo config not found: {config / 'marimo' / 'marimo.toml'}")
    os.environ["XDG_CONFIG_HOME"] = str(config)
    os.environ["MARIMO_SKIP_UPDATE_CHECK"] = "1"


def _venv_shim() -> Path:
    shim = Path(os.environ.get("EGRET_VENV_SHIM", DEFAULT_VENV_SHIM))
    if not os.access(shim / "bin" / "python", os.X_OK):
        raise LaunchError(EX_DATAERR, f"venv shim not executable: {shim / 'bin' / 'python'}")
    # The shim answers marimo's install probe with this version and execs this interpreter.
    import marimo

    os.environ["EGRET_REAL_PYTHON"] = sys.executable
    os.environ["EGRET_MARIMO_VERSION"] = marimo.__version__
    return shim


def build(mode: str) -> list[str]:
    """Validate, lock the config, install this mode's patch, and return marimo's argv."""
    env = _check_env(mode)
    workdir = Path(env["APPS_WORKDIR"]).resolve()
    workdir.mkdir(parents=True, exist_ok=True)
    _lock_config(workdir)
    os.chdir(workdir)
    api = PodApi(env["EGRET_URL"], env["EGRET_POD_TOKEN"])

    if mode == "edit":
        install_storage_patch(workdir, api)
        target = str(workdir)
    else:
        shim = _venv_shim()
        app_id = env["EGRET_APP_ID"].lower()
        try:
            source = api.get_version(app_id, env["EGRET_VERSION_ID"])
        except GatewayError as e:
            raise LaunchError(EX_UNAVAILABLE, f"cannot fetch version {env['EGRET_VERSION_ID']}: {e}") from e
        app_file = workdir / f"{app_id}.py"
        app_file.write_text(source, encoding="utf-8")
        install_run_mode_ipc_patch(shim)
        target = str(app_file)

    for name in SCRUBBED:
        os.environ.pop(name, None)

    # Never --watch: in edit mode every load rewrites the app file from egret's draft.
    argv = [
        mode, target,
        "--host", "0.0.0.0",
        "--port", os.environ.get("EGRET_MARIMO_PORT", PORTS[mode]),
        "--headless",
        "--base-url", env["EGRET_BASE_URL"],
        "--token-password", env["EGRET_MARIMO_TOKEN"],
    ]
    if mode == "edit":
        argv.append("--skip-update-check")  # `marimo run` has no such flag; the env var covers it
    return argv


def run_marimo(argv: list[str]) -> int:
    from marimo._cli.cli import main as marimo_main

    # marimo re-execs itself from sys.argv in some paths, so make it look like its own.
    sys.argv = ["marimo", *argv]
    try:
        marimo_main.main(args=argv, standalone_mode=True)
    except SystemExit as e:
        return e.code if isinstance(e.code, int) else (0 if e.code is None else 1)
    return 0


def main(argv: list[str] | None = None) -> int:
    args = sys.argv[1:] if argv is None else argv
    if len(args) != 1 or args[0] not in REQUIRED:
        print("usage: egret-apps edit|run", file=sys.stderr)
        return EX_USAGE
    try:
        marimo_argv = build(args[0])
    except LaunchError as e:
        print(f"egret-apps: {e}", file=sys.stderr)
        return e.code
    print(f"egret-apps: marimo {args[0]} on port {marimo_argv[marimo_argv.index('--port') + 1]}", flush=True)
    return run_marimo(marimo_argv)
