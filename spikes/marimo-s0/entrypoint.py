"""egret's marimo launcher, spike version (spec §5.1, §5.2, §6.3)."""

import argparse
import os
import pathlib
import sys

import httpx

EGRET = os.environ.get("EGRET_URL", "http://127.0.0.1:8700")
POD_TOKEN = "pod-kernel-token"
POD_MARIMO_TOKEN = "pod-secret"
WORKDIR = pathlib.Path(os.environ["APPS_WORKDIR"]).resolve()
HERE = pathlib.Path(__file__).parent.resolve()


SANDBOX_HEADER = """# /// script
# dependencies = ["marimo>=0.25.0", "httpx", "polars", "duckdb", "sqlglot"]
# ///
"""


def fetch_draft(app_id: str) -> dict:
    r = httpx.get(f"{EGRET}/api/apps/{app_id}/draft", headers={"authorization": f"Bearer {POD_TOKEN}"})
    r.raise_for_status()
    return r.json()


def install_storage_patch() -> None:
    from marimo._server.workspace import _base
    from marimo._session.notebook.storage import FilesystemStorage
    from marimo._utils.http import HTTPException

    class EgretStorage(FilesystemStorage):
        def __init__(self, app_id: str, revision: int) -> None:
            super().__init__()
            self.app_id, self.revision = app_id, revision

        def write(self, path, content: str) -> None:
            r = httpx.put(
                f"{EGRET}/api/apps/{self.app_id}/draft",
                content=content.encode(),
                headers={"authorization": f"Bearer {POD_TOKEN}", "if-match": str(self.revision)},
            )
            if r.status_code == 409:
                raise HTTPException(409, "This draft was saved by someone else — reload.")
            r.raise_for_status()
            self.revision = r.json()["revision"]
            super().write(path, content)

    original_load = _base.NotebookWorkspace.load

    def load(self, key, defaults=None):
        resolved = self.resolve(key)
        app_id = None
        if resolved and pathlib.Path(resolved).resolve().parent == WORKDIR:
            app_id = pathlib.Path(resolved).stem
            draft = fetch_draft(app_id)
            pathlib.Path(resolved).write_text(draft["source"])
        manager = original_load(self, key, defaults)
        if app_id:
            manager.storage = EgretStorage(app_id, draft["revision"])
            print(f"[s0] EgretStorage bound to {app_id} rev {draft['revision']}", flush=True)
        return manager

    _base.NotebookWorkspace.load = load


def install_force_ipc_patch() -> None:
    """Run mode, one kernel subprocess per session, without a uv sandbox (S0 question 2, variant ipc)."""
    from marimo._session import session as session_mod
    from marimo._session.managers import ipc as ipc_mod
    from marimo._session.model import SessionMode

    original_create = session_mod.SessionImpl.create.__func__

    async def create(cls, *args, **kwargs):
        # Verified (Step 7): Session.create is keyword-only (`*,` at
        # marimo/_session/session.py:101), so `mode` and `sandbox` are always kwargs.
        if kwargs.get("mode") == SessionMode.RUN:
            kwargs["sandbox"] = True
        return await original_create(cls, *args, **kwargs)

    session_mod.SessionImpl.create = classmethod(create)

    # Step 7 adaptation: IPCKernelManagerImpl has no `_sandbox` flag in 0.25.0. Without a
    # configured `[venv] path` it always builds a uv NotebookSandbox
    # (marimo/_session/managers/ipc.py:331-431). Pointing the venv config at this
    # interpreter's prefix is the equivalent of "use this venv, not a uv sandbox". It is
    # patched here, not written into marimo.toml, so `--process-mode sandbox` keeps its
    # real uv sandbox (a configured venv takes precedence, ipc.py:333).
    original_get_venv_config = ipc_mod._get_venv_config

    def get_venv_config(config_manager):
        return {**original_get_venv_config(config_manager), "path": sys.prefix}

    ipc_mod._get_venv_config = get_venv_config

    from timing import mark

    original_start_kernel = ipc_mod.IPCKernelManagerImpl.start_kernel

    async def start_kernel(self):
        mark("kernel_spawn_begin")
        result = await original_start_kernel(self)
        mark("kernel_ready")
        return result

    ipc_mod.IPCKernelManagerImpl.start_kernel = start_kernel


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--mode", choices=["edit", "run"], required=True)
    p.add_argument("--process-mode", choices=["thread", "sandbox", "ipc"], default="thread")
    p.add_argument("--app", default="probe")
    a = p.parse_args()

    os.environ["XDG_CONFIG_HOME"] = str(HERE / "config")
    os.environ.setdefault("MARIMO_SKIP_UPDATE_CHECK", "1")
    sys.path.insert(0, str(HERE))  # so kernels can import egret_marimo
    os.environ["PYTHONPATH"] = f"{HERE}:{os.environ.get('PYTHONPATH', '')}"
    WORKDIR.mkdir(parents=True, exist_ok=True)

    if a.mode == "edit":
        install_storage_patch()
        for app_id in ("probe", "xss"):
            try:
                (WORKDIR / f"{app_id}.py").write_text(fetch_draft(app_id)["source"])
            except httpx.HTTPStatusError as e:  # apps/xss.py arrives in a later task
                print(f"[s0] skipping draft {app_id}: {e.response.status_code}", flush=True)
        argv = ["edit", str(WORKDIR), "--port", "2718", "--base-url", "/e"]
    else:
        source = fetch_draft(a.app)["source"]
        if a.process_mode == "sandbox":
            # the sandbox kernel runs in a fresh uv env holding only marimo: the app's imports must be declared (PEP 723)
            source = SANDBOX_HEADER + source
        (WORKDIR / f"{a.app}.py").write_text(source)
        argv = ["run", str(WORKDIR / f"{a.app}.py"), "--port", "2719", "--base-url", "/r"]
        if a.process_mode == "sandbox":
            argv.append("--sandbox")
        if a.process_mode == "ipc":
            os.environ["S0_VENV"] = sys.prefix
            install_force_ipc_patch()

    argv += ["--host", "127.0.0.1", "--headless", "--token-password", POD_MARIMO_TOKEN]
    print(f"[s0] marimo {' '.join(argv)}", flush=True)

    from marimo._cli.cli import main as marimo_main

    # --sandbox re-execs `python -m marimo <sys.argv[1:]>`, so sys.argv must look like marimo's own
    sys.argv = ["marimo", *argv]
    marimo_main.main(args=argv, standalone_mode=True)


if __name__ == "__main__":
    main()
