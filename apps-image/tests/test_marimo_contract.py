"""The private marimo API that egret_marimo.storage patches. An upgrade must fail here first."""

import inspect
from pathlib import Path

import marimo
from marimo._ast.cell import CellConfig
from marimo._server.models.models import SaveNotebookRequest
from marimo._server.workspace import DirectoryWorkspace, NotebookWorkspace
from marimo._session.notebook.file_manager import AppFileManager
from marimo._session.notebook.storage import FilesystemStorage
from marimo._utils.http import HTTPException

SOURCE = (
    "import marimo\n\napp = marimo.App()\n\n\n@app.cell\ndef _():\n    x = 1\n    return\n\n\n"
    'if __name__ == "__main__":\n    app.run()\n'
)


def test_marimo_version_is_pinned():
    assert marimo.__version__ == "0.25.0"


def test_workspace_load_signature():
    params = list(inspect.signature(NotebookWorkspace.load).parameters)
    assert params == ["self", "key", "defaults"]
    assert inspect.signature(NotebookWorkspace.load).parameters["defaults"].default is None


def test_directory_workspace_inherits_load_and_resolve_raises_404_for_missing(tmp_path):
    assert DirectoryWorkspace.load is NotebookWorkspace.load
    ws = DirectoryWorkspace(str(tmp_path), include_markdown=False)
    try:
        ws.resolve("missing.py")
    except HTTPException as e:
        assert e.status_code == 404
    else:
        raise AssertionError("resolve of a missing file must raise 404")


class RecordingStorage(FilesystemStorage):
    def __init__(self) -> None:
        super().__init__()
        self.writes: list[tuple[Path, str]] = []

    def write(self, path, content):
        self.writes.append((path, content))
        super().write(path, content)


def test_storage_is_assignable_and_save_calls_storage_write(tmp_path):
    f = tmp_path / "nb.py"
    f.write_text(SOURCE)
    manager = AppFileManager(str(f))
    rec = RecordingStorage()
    manager.storage = rec
    assert manager.storage is rec
    ids = list(manager.app.cell_manager.cell_ids())
    out = manager.save(
        SaveNotebookRequest(
            cell_ids=ids, codes=["x = 2"], names=["_"], configs=[CellConfig()], filename=str(f)
        )
    )
    assert len(rec.writes) == 1
    path, contents = rec.writes[0]
    assert Path(path).resolve() == f.resolve()
    assert contents == out and "x = 2" in contents


def test_filesystem_storage_shape():
    for name in ("read", "write", "exists", "rename", "is_same_path", "get_absolute_path"):
        assert callable(getattr(FilesystemStorage, name))
    assert list(inspect.signature(FilesystemStorage.write).parameters) == ["self", "path", "content"]
    FilesystemStorage()  # no-arg constructor


def test_http_exception_shape():
    e = HTTPException(409, "detail")
    assert (e.status_code, e.detail) == (409, "detail")
    assert list(inspect.signature(HTTPException.__init__).parameters) == [
        "self",
        "status_code",
        "detail",
    ]


def test_notebook_file_lock_is_what_save_takes():
    from marimo._environments import script_metadata
    from marimo._session.notebook import file_manager

    assert file_manager.notebook_file_lock is script_metadata.notebook_file_lock
    assert "notebook_file_lock(str(path))" in inspect.getsource(AppFileManager._save_file)


def test_resume_path_shape(tmp_path):
    from marimo._config.manager import get_default_config_manager
    from marimo._server.lsp import NoopLspServer
    from marimo._server.resume_strategies import EditModeResumeStrategy
    from marimo._server.session_manager import SessionManager
    from marimo._session.model import ConnectionState, SessionMode
    from marimo._session.session_repository import SessionRepository

    sig = inspect.signature(SessionManager.maybe_resume_session).parameters
    assert list(sig) == ["self", "new_session_id", "file_key"]
    assert list(inspect.signature(SessionManager.close_session).parameters) == ["self", "session_id"]
    assert callable(SessionManager._resolve_file_key)
    for name in ("get_all_by_file_key", "get_session_id", "add_sync", "remove_sync"):
        assert callable(getattr(SessionRepository, name))
    assert ConnectionState.ORPHANED
    sm = SessionManager(
        workspace=DirectoryWorkspace(str(tmp_path), include_markdown=False),
        mode=SessionMode.EDIT, quiet=True, include_code=True, lsp_server=NoopLspServer(),
        config_manager=get_default_config_manager(current_path=None), cli_args={}, argv=None,
        auth_token=None, redirect_console_to_browser=False, ttl_seconds=None,
    )
    assert isinstance(sm._repository, SessionRepository)
    assert isinstance(sm._resume_strategy, EditModeResumeStrategy)
    # The edit-mode reconnect resumes through maybe_resume_session (ws_session_connector step 4).
    from marimo._server.api.endpoints.ws import ws_session_connector

    assert "self.manager.maybe_resume_session(" in inspect.getsource(ws_session_connector)


# --- Task 3: the run-mode IPC patch, the D14 shim, the locked config ---------


def test_session_create_takes_mode_and_sandbox_as_keywords():
    from marimo._session.session import SessionImpl

    assert isinstance(inspect.getattr_static(SessionImpl, "create"), classmethod)
    params = inspect.signature(SessionImpl.create).parameters
    for name in ("mode", "sandbox", "app_host_context"):
        assert params[name].kind is inspect.Parameter.KEYWORD_ONLY, name
    assert params["sandbox"].default is False
    src = inspect.getsource(SessionImpl.create)
    # sandbox=True takes the IPC kernel path unless an app host is in play.
    assert "elif sandbox:" in src and "IPCKernelManagerImpl(" in src


def test_session_manager_passes_mode_and_sandbox_by_keyword():
    from marimo._server.session_manager import SessionManager

    src = inspect.getsource(SessionManager)
    assert "SessionImpl.create(" in src
    assert "mode=self.mode," in src and "sandbox=self.sandbox," in src


def test_ipc_start_kernel_reads_venv_config_through_the_module_global():
    from marimo._session.managers import ipc

    assert callable(ipc._get_venv_config)
    assert list(inspect.signature(ipc._get_venv_config).parameters) == ["config_manager"]
    src = inspect.getsource(ipc.IPCKernelManagerImpl.start_kernel)
    assert "_get_venv_config(self.config_manager)" in src
    assert "has_marimo_installed(venv_python)" in src
    assert 'venv_config.get("writable", False)' in src


def test_install_probe_is_what_the_shim_answers():
    from marimo._session import _venv

    probe = "import marimo, msgspec, zmq; print(marimo.__version__)"
    assert f'"{probe}"' in inspect.getsource(_venv.has_marimo_installed)
    shim = (Path(__file__).resolve().parent.parent / "venv-shim" / "bin" / "python").read_text()
    assert f'"$2" = "{probe}"' in shim
    assert 'venv / "bin" / "python"' in inspect.getsource(_venv._find_python_in_venv)


def test_rtc_v2_is_the_experimental_key_the_ws_connector_reads():
    from marimo._config.config import ExperimentalConfig
    from marimo._server.api.endpoints.ws import ws_connection_validator

    assert "rtc_v2" in ExperimentalConfig.__annotations__
    assert '"rtc_v2", False' in inspect.getsource(ws_connection_validator)


def test_cli_entry_accepts_args_and_standalone_mode():
    import click
    from marimo._cli.cli import main

    assert isinstance(main, click.Group)
    assert {"edit", "run"} <= set(main.commands)
    edit = {p.name for p in main.commands["edit"].params}
    run = {p.name for p in main.commands["run"].params}
    common = {"host", "port", "headless", "base_url", "token_password", "watch"}
    assert common <= edit and common <= run
    assert "skip_update_check" in edit and "skip_update_check" not in run


def test_security_config_partials_are_merged_last():
    from marimo._config import manager

    assert callable(manager.SecurityConfigManager.get_config)
    assert "SecurityConfigManager()" in inspect.getsource(manager.get_default_config_manager)
    init = inspect.getsource(manager.MarimoConfigManager.__init__)
    assert "isinstance(p, SecurityConfigManager)" in init
    overrides = inspect.getsource(manager.MarimoConfigManager.get_config_overrides)
    assert "(*self.partials, *self.security_partials)" in overrides
    with_partial = inspect.getsource(manager.MarimoConfigManager.with_partial)
    assert "partial,\n            *self.security_partials," in with_partial
    session_create = inspect.getsource(__import__("marimo._session.session", fromlist=["x"]).SessionImpl.create)
    assert "config_manager.with_partial(" in session_create
