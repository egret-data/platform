"""Run mode: one kernel process per viewer session (spec D7).

``marimo run`` executes every session's kernel in a thread of the server
process, so all viewers share one interpreter: module globals, ``os.environ``
and the request context of whoever ran last. This patch makes each RUN session
use marimo's IPC kernel manager instead, which spawns a fresh interpreter.

Two private hooks in marimo 0.25.0 (``tests/test_marimo_contract.py`` pins them):

- ``SessionImpl.create(..., mode=, sandbox=)`` (keyword-only) takes the IPC
  path when ``sandbox`` is true. We force it for ``SessionMode.RUN`` only;
  edit mode keeps marimo's own process-per-notebook kernel.
- ``IPCKernelManagerImpl.start_kernel`` reads the venv through the module
  global ``marimo._session.managers.ipc._get_venv_config``. With no ``path``
  it builds a uv sandbox per kernel; with a ``path`` it launches
  ``<path>/bin/python -m marimo._ipc.launch_kernel``. We point it at the D14
  shim venv, read-only, so the per-launch install probe is answered by the
  shim and nothing is ever installed.
"""

from __future__ import annotations

from pathlib import Path


def install_run_mode_ipc_patch(venv_shim: Path) -> None:
    from marimo._session import session as session_mod
    from marimo._session.managers import ipc as ipc_mod
    from marimo._session.model import SessionMode

    session_cls = session_mod.SessionImpl
    original_create = getattr(session_cls, "_egret_original_create", session_cls.create.__func__)

    async def create(cls, **kwargs):
        # SessionImpl.create is keyword-only, so mode and sandbox are always kwargs.
        if kwargs.get("mode") == SessionMode.RUN:
            kwargs["sandbox"] = True
        return await original_create(cls, **kwargs)

    session_cls._egret_original_create = original_create  # type: ignore[attr-defined]
    session_cls.create = classmethod(create)  # type: ignore[method-assign]

    original_get_venv_config = getattr(ipc_mod, "_egret_original_get_venv_config", ipc_mod._get_venv_config)
    shim = str(Path(venv_shim))

    def get_venv_config(config_manager):
        # writable=False: never pip-install marimo into the shim; the probe is answered instead.
        return {**original_get_venv_config(config_manager), "path": shim, "writable": False}

    ipc_mod._egret_original_get_venv_config = original_get_venv_config  # type: ignore[attr-defined]
    ipc_mod._get_venv_config = get_venv_config
