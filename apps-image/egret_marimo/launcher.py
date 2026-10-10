"""``egret-apps edit|run``: the pod's entrypoint.

Everything else comes from the environment (set by egret's launcher, P4):

| Variable             | Mode | Meaning                                     |
|----------------------|------|---------------------------------------------|
| ``EGRET_URL``        | both | egret's base URL                            |
| ``EGRET_POD_TOKEN``  | both | the pod token (pod API) — secret            |
| ``EGRET_MARIMO_TOKEN`` | both | marimo's ``--token-password`` — secret    |
| ``EGRET_BASE_URL``   | both | ``/e/<pod>`` or ``/r/<version>``            |
| ``APPS_WORKDIR``     | both | working directory (the image sets ``/work``) |
| ``EGRET_APP_ID``     | run  | the app                                     |
| ``EGRET_VERSION_ID`` | run  | the pinned version                          |

Two phases. The first checks the environment, then re-``execve``s this same
process (``python -m egret_marimo.launcher --internal <mode>``) with the secrets
removed from the environment and handed over an inherited pipe instead. Popping
them from ``os.environ`` is not enough: the process's ORIGINAL environment block
stays readable (``/proc/<pid>/environ``, ``ps eww``) by any same-uid process, and
every kernel runs app code as the same uid. The internal phase reads and closes
the pipe, then starts marimo in-process. (The real isolation is a kernel uid that
differs from the server's; that is P4/P5.)

Test-only overrides, honoured only under pytest (``PYTEST_CURRENT_TEST``):
``EGRET_APPS_CONFIG`` (the locked marimo config, default
``/opt/egret-apps/config``), ``EGRET_VENV_SHIM`` (default
``/opt/egret-apps/venv-shim``) — outside pytest both must lie under
``/opt/egret-apps`` — and ``EGRET_MARIMO_PORT`` (default 2718 edit, 2719 run).

Exit codes: 64 bad usage or a missing/invalid variable; 65 the locked
configuration could be shadowed or is missing; 69 egret could not serve the
version source.
"""

from __future__ import annotations

import json
import os
import re
import sys
from pathlib import Path

from .client import GatewayError
from .patches import install_config_lock, install_run_mode_ipc_patch
from .podapi import PodApi
from .storage import install_storage_patch, write_layout_file

EX_USAGE = 64
EX_DATAERR = 65
EX_UNAVAILABLE = 69

IMAGE_ROOT = Path("/opt/egret-apps")
DEFAULT_CONFIG = IMAGE_ROOT / "config"
DEFAULT_VENV_SHIM = IMAGE_ROOT / "venv-shim"
PORTS = {"edit": "2718", "run": "2719"}
RUN_KERNEL_MODES = {"thread", "forced_ipc"}
INTERNAL = "--internal"
SECRETS_FD = "EGRET_SECRETS_FD"
SECRETS = ("EGRET_POD_TOKEN", "EGRET_MARIMO_TOKEN")
# Written to our own pipe before exec, so it must fit the pipe buffer (16 KiB on macOS,
# 64 KiB on Linux) or the write blocks forever. Real tokens are a few hundred bytes.
MAX_SECRETS_BYTES = 4096
# -I (isolated): no cwd/script dir on sys.path (implies -P), no user site-packages
# (implies -s; ~/.local is writable by the same uid as the kernels), and no PYTHON*
# variables (implies -E). With `-m`, plain `python` would put the cwd first on
# sys.path, so a planted `json.py` or `egret_marimo/` there would run in phase 2
# while the secrets pipe is open. Nothing in the image relies on PYTHON* variables.
REEXEC_FLAGS = ["-I"]

REQUIRED = {
    "edit": ["EGRET_URL", "EGRET_POD_TOKEN", "EGRET_MARIMO_TOKEN", "EGRET_BASE_URL", "APPS_WORKDIR"],
    "run": [
        "EGRET_URL", "EGRET_POD_TOKEN", "EGRET_MARIMO_TOKEN", "EGRET_BASE_URL", "APPS_WORKDIR",
        "EGRET_APP_ID", "EGRET_VERSION_ID",
    ],
}

_VERSION_ID = re.compile(r"^[A-Za-z0-9-]+$")
_UUID = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$", re.IGNORECASE)


class LaunchError(Exception):
    def __init__(self, code: int, message: str) -> None:
        super().__init__(message)
        self.code = code


def _under_pytest() -> bool:
    return bool(os.environ.get("PYTEST_CURRENT_TEST"))


def shadowing_config(workdir: Path) -> Path | None:
    """A config file marimo would layer over the locked config, if any.

    marimo takes the first ``.marimo.toml`` walking up from the cwd (the
    launcher chdirs to the workdir), then ``$HOME``, before ``XDG_CONFIG_HOME``;
    and the nearest ``pyproject.toml`` walking up from the notebook or workdir
    is the project layer, merged OVER the user config. The security-layer lock
    (patches.install_config_lock) holds regardless; refusing here keeps the
    project layer empty at startup.
    """
    for directory in (workdir, *workdir.parents):
        for name in (".marimo.toml", "pyproject.toml"):
            if (directory / name).exists():
                return directory / name
    home = Path.home() / ".marimo.toml"
    return home if home.exists() else None


def _check_env(mode: str, secrets: dict[str, str]) -> dict[str, str]:
    env = {name: os.environ.get(name, "") for name in REQUIRED[mode]} | secrets
    missing = [name for name in REQUIRED[mode] if not env.get(name)]
    if missing:
        raise LaunchError(EX_USAGE, f"missing required environment variable(s): {', '.join(missing)}")
    if mode == "run" and not _UUID.fullmatch(env["EGRET_APP_ID"]):
        raise LaunchError(EX_USAGE, f"EGRET_APP_ID is not an app id: {env['EGRET_APP_ID']!r}")
    if mode == "run" and not _VERSION_ID.fullmatch(env["EGRET_VERSION_ID"]):
        # It is put into the pod-API URL path as is.
        raise LaunchError(EX_USAGE, f"EGRET_VERSION_ID is not a version id: {env['EGRET_VERSION_ID']!r}")
    if mode == "run":
        _run_kernel_mode()  # validates; raises before anything is fetched or started
    return env


def _image_path(var: str, default: Path) -> Path:
    path = Path(os.environ.get(var, str(default)))
    if not _under_pytest() and not path.resolve().is_relative_to(IMAGE_ROOT):
        raise LaunchError(EX_DATAERR, f"{var}={path} is outside {IMAGE_ROOT} (test-only override)")
    return path


def _run_kernel_mode() -> str:
    """D24: a run pod serves exactly one viewer, so the default kernel mode is marimo's own
    thread mode. "forced_ipc" (the P3 patch) is kept for a possible future shared-pod revival —
    not a supported production path today."""
    mode = os.environ.get("EGRET_RUN_KERNEL_MODE") or "thread"
    if mode not in RUN_KERNEL_MODES:
        raise LaunchError(EX_USAGE, f"EGRET_RUN_KERNEL_MODE must be one of {sorted(RUN_KERNEL_MODES)}, got {mode!r}")
    return mode


def _port(mode: str) -> str:
    override = os.environ.get("EGRET_MARIMO_PORT")
    if override and not _under_pytest():
        raise LaunchError(EX_USAGE, "EGRET_MARIMO_PORT is a test-only override")
    return override or PORTS[mode]


def _lock_config(workdir: Path) -> None:
    shadow = shadowing_config(workdir)
    if shadow is not None:
        raise LaunchError(EX_DATAERR, f"refusing to start: {shadow} would override the locked marimo config")
    config = _image_path("EGRET_APPS_CONFIG", DEFAULT_CONFIG)
    if not (config / "marimo" / "marimo.toml").is_file():
        raise LaunchError(EX_DATAERR, f"locked marimo config not found: {config / 'marimo' / 'marimo.toml'}")
    os.environ["XDG_CONFIG_HOME"] = str(config)
    os.environ["MARIMO_SKIP_UPDATE_CHECK"] = "1"
    install_config_lock()


def _venv_shim() -> Path:
    shim = _image_path("EGRET_VENV_SHIM", DEFAULT_VENV_SHIM)
    if not os.access(shim / "bin" / "python", os.X_OK):
        raise LaunchError(EX_DATAERR, f"venv shim not executable: {shim / 'bin' / 'python'}")
    # The shim answers marimo's install probe with this version and execs this interpreter.
    import marimo

    os.environ["EGRET_REAL_PYTHON"] = sys.executable
    os.environ["EGRET_MARIMO_VERSION"] = marimo.__version__
    return shim


def build(mode: str, secrets: dict[str, str]) -> list[str]:
    """Internal phase: validate, lock the config, install this mode's patch, return marimo's argv."""
    env = _check_env(mode, secrets)
    port = _port(mode)
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
            version = api.get_version(app_id, env["EGRET_VERSION_ID"])
        except GatewayError as e:
            raise LaunchError(EX_UNAVAILABLE, f"cannot fetch version {env['EGRET_VERSION_ID']}: {e}") from e
        app_file = workdir / f"{app_id}.py"
        app_file.write_text(version.source, encoding="utf-8")
        # The run pod never ran the save that produced the layout file the source
        # references (`layout_file=...` in its header); restore it before marimo
        # parses that header, or `read_layout_config` finds nothing.
        write_layout_file(workdir, app_id, version.layout)
        if _run_kernel_mode() == "forced_ipc":
            install_run_mode_ipc_patch(shim)
        target = str(app_file)

    # -q first (a marimo GROUP option): without it marimo prints its startup URL, which
    # carries ``?access_token=<EGRET_MARIMO_TOKEN>``, to stdout, i.e. into the pod log.
    # Never --watch: in edit mode every load rewrites the app file from egret's draft.
    argv = [
        "-q", mode, target,
        "--host", "0.0.0.0",
        "--port", port,
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


def _exec(path: str, argv: list[str], env: dict[str, str]) -> int:
    os.execve(path, argv, env)  # does not return
    return 0  # pragma: no cover


def _reexec(mode: str) -> int:
    """First phase: hand the secrets to a re-exec of this process whose environment lacks them."""
    secrets = {name: os.environ[name] for name in SECRETS}
    payload = json.dumps(secrets).encode()
    if len(payload) > MAX_SECRETS_BYTES:
        raise LaunchError(EX_USAGE, f"{' and '.join(SECRETS)} exceed {MAX_SECRETS_BYTES} bytes together")
    read_fd, write_fd = os.pipe()
    os.write(write_fd, payload)  # tokens are far below the pipe buffer, so this cannot block
    os.close(write_fd)
    os.set_inheritable(read_fd, True)
    env = {k: v for k, v in os.environ.items() if k not in SECRETS}
    env[SECRETS_FD] = str(read_fd)
    argv = [sys.executable, *REEXEC_FLAGS, "-m", "egret_marimo.launcher", INTERNAL, mode]
    return _exec(sys.executable, argv, env)


def _read_secrets() -> dict[str, str]:
    fd_text = os.environ.pop(SECRETS_FD, "")
    if not fd_text.isdigit():
        raise LaunchError(EX_USAGE, f"{INTERNAL} needs {SECRETS_FD}; run `egret-apps edit|run`")
    fd = int(fd_text)
    chunks = []
    try:
        while chunk := os.read(fd, 65536):
            chunks.append(chunk)
    finally:
        os.close(fd)
    try:
        secrets = json.loads(b"".join(chunks))
    except ValueError:  # includes JSONDecodeError and bad UTF-8
        secrets = None
    if not isinstance(secrets, dict) or not all(isinstance(secrets.get(n, ""), str) for n in SECRETS):
        raise LaunchError(EX_USAGE, f"malformed secrets on {SECRETS_FD}: expected a JSON object of strings")
    return {name: secrets.get(name, "") for name in SECRETS}


PR_SET_DUMPABLE = 4


def make_undumpable() -> bool:
    """Linux: stop other processes of this uid reading this one's `/proc/<pid>/environ` and `mem`.

    Called as soon as the secrets are in memory. The kernels this server spawns run app code as the
    same uid, so file permissions alone would let a cell read them; with the dumpable flag off those
    files belong to root. Does nothing off Linux (returns False). Kernels are fresh `exec`s and are
    dumpable again — see tests/test_undumpable.py for what this does and does not cover.
    """
    if not sys.platform.startswith("linux"):
        return False
    import ctypes

    return ctypes.CDLL(None, use_errno=True).prctl(PR_SET_DUMPABLE, 0, 0, 0, 0) == 0


def keep_dumpable() -> bool:
    """Test-only: leave the server readable, so a test can read ITS OWN view of the environment.

    Honoured only under pytest, like the launcher's other overrides, so a production pod cannot
    switch the protection off through its environment. The e2e tests read the server's environment
    block with `ps eww` to prove the tokens are not in it, and `ps` can read nothing of an
    undumpable process; the undumpable protection has its own test and a smoke check.
    """
    return bool(os.environ.get("EGRET_TEST_KEEP_DUMPABLE")) and _under_pytest()


def _internal(mode: str) -> int:
    try:
        secrets = _read_secrets()
        if not keep_dumpable():
            make_undumpable()
        marimo_argv = build(mode, secrets)
    except LaunchError as e:
        print(f"egret-apps: {e}", file=sys.stderr)
        return e.code
    print(f"egret-apps: marimo {mode} on port {marimo_argv[marimo_argv.index('--port') + 1]}", flush=True)
    return run_marimo(marimo_argv)


def main(argv: list[str] | None = None) -> int:
    args = sys.argv[1:] if argv is None else argv
    if len(args) == 2 and args[0] == INTERNAL and args[1] in REQUIRED:
        return _internal(args[1])
    if len(args) != 1 or args[0] not in REQUIRED:
        print("usage: egret-apps edit|run", file=sys.stderr)
        return EX_USAGE
    try:
        _check_env(args[0], {})
        return _reexec(args[0])
    except LaunchError as e:
        print(f"egret-apps: {e}", file=sys.stderr)
        return e.code


if __name__ == "__main__":
    sys.exit(main())
