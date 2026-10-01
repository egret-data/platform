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
