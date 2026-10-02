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


# Keys no config layer may turn back on. The image's marimo.toml sets them too, but
# that is only the USER layer: a project pyproject.toml ([tool.marimo]) or the app's
# own PEP 723 header ([tool.marimo] in the script) merges over it. marimo merges
# SecurityConfigManager partials after every other layer (marimo/_config/manager.py,
# MarimoConfigManager.__init__ / with_partial), so values returned there always win.
# - ai.enabled: AI panels and actions (a project could otherwise point them at any
#   provider URL).
# - experimental.rtc_v2: with RTC an edit-mode reconnect reattaches to the orphaned
#   session before resume runs, bypassing the conflicted-session close in storage.py.
# Not locked: runtime.serve_cached_sessions_in_apps is header-settable but harmless
# here: in run mode the session cache is read-only (CacheMode.READ, session.py), and
# nothing writes `__marimo__/session/` into a run pod's fresh workdir.
# - sharing.*: every external code-sharing affordance (shareable WASM links, molab, HTML
#   publishing). The pod's egress is closed, so none can complete, but the buttons would invite
#   an author to try and a viewer to wonder; marimo hides them all when each flag is False. The
#   keys come from marimo's own schema (as it does for MARIMO_RESTRICT_SHARING), so a sharing
#   target added by an upgrade is locked without anyone remembering to.
# What a header may still set (runtime.dotenv, pythonpath, on_cell_change, ...) grants the author
# nothing a cell does not already have; see tests/test_config_lock.py for the measurement.
def _sharing_off() -> dict[str, bool]:
    from marimo._config.config import SharingConfig

    return dict.fromkeys(SharingConfig.__annotations__, False)


LOCKED_CONFIG = {
    "ai": {"enabled": False},
    "experimental": {"rtc_v2": False},
    "sharing": _sharing_off(),
}


def install_config_lock() -> None:
    from marimo._config.manager import SecurityConfigManager

    original = getattr(SecurityConfigManager, "_egret_original_get_config", SecurityConfigManager.get_config)

    def get_config(self, *, hide_secrets: bool = True):
        config = dict(original(self, hide_secrets=hide_secrets))
        for section, values in LOCKED_CONFIG.items():
            config[section] = {**config.get(section, {}), **values}
        return config

    SecurityConfigManager._egret_original_get_config = original  # type: ignore[attr-defined]
    SecurityConfigManager.get_config = get_config  # type: ignore[method-assign]
