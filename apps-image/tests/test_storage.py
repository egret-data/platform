"""EgretStorage against marimo's real workspace and file manager (Task 2)."""

import threading
from pathlib import Path

import pytest
from marimo._ast.cell import CellConfig
from marimo._server.models.models import SaveNotebookRequest
from marimo._config.manager import get_default_config_manager
from marimo._environments.script_metadata import notebook_file_lock
from marimo._server.lsp import NoopLspServer
from marimo._server.session_manager import SessionManager
from marimo._server.workspace import DirectoryWorkspace, NotebookWorkspace
from marimo._session.model import ConnectionState, SessionMode
from marimo._session.types import KernelState
from marimo._utils.http import HTTPException

from egret_marimo.podapi import PodApi
from egret_marimo.storage import EgretStorage, install_storage_patch

APP = "11111111-1111-4111-8111-111111111111"


def notebook(code: str) -> str:
    return (
        "import marimo\n\napp = marimo.App()\n\n\n@app.cell\ndef _():\n"
        f"    {code}\n    return\n\n\nif __name__ == \"__main__\":\n    app.run()\n"
    )


def save(manager, code: str) -> None:
    ids = list(manager.app.cell_manager.cell_ids())
    manager.save(
        SaveNotebookRequest(
            cell_ids=ids,
            codes=[code] * len(ids),
            names=["_"] * len(ids),
            configs=[CellConfig() for _ in ids],
            filename=str(manager.path),
        )
    )


@pytest.fixture
def setup(egret, tmp_path):
    egret.pod_tokens["pod-t"] = {"mode": "edit"}
    egret.drafts[APP] = {"source": notebook("x = 1"), "revision": 1}
    workdir = tmp_path / "apps"
    workdir.mkdir()
    api = PodApi(egret.base_url, "pod-t")
    original_load, original_resume = NotebookWorkspace.load, SessionManager.maybe_resume_session
    install_storage_patch(workdir, api)
    yield egret, workdir, api, DirectoryWorkspace(str(workdir), include_markdown=False)
    NotebookWorkspace.load = original_load
    SessionManager.maybe_resume_session = original_resume


def other_author_saves(egret, code: str) -> None:
    """Another author's save, straight through the fake's PUT (not through this pod)."""
    api = PodApi(egret.base_url, "pod-t")
    d = api.get_draft(APP)
    api.put_draft(APP, d.revision, notebook(code))


def test_1_first_load_fetches_writes_and_binds_rev_1(setup):
    egret, workdir, _, ws = setup
    m = ws.load(f"{APP}.py")
    assert (workdir / f"{APP}.py").read_text() == notebook("x = 1")
    assert isinstance(m.storage, EgretStorage)
    assert (m.storage.app_id, m.storage.base_revision) == (APP, 1)
    assert list(m.app.cell_manager.codes()) == ["x = 1"]


def test_2_stale_session_never_overwrites_and_fresh_session_saves(setup):
    egret, workdir, _, ws = setup
    m1 = ws.load(f"{APP}.py")
    other_author_saves(egret, "x = 'theirs'")
    # Storage unit: a second manager built by a later load, on the latest draft. (In marimo a
    # browser reload RESUMES the session; see test_reload_after_conflict_* for that path.)
    m2 = ws.load(f"{APP}.py")
    assert m2.storage.base_revision == 2
    assert (workdir / f"{APP}.py").read_text() == notebook("x = 'theirs'")
    theirs = {"source": notebook("x = 'theirs'"), "revision": 2}
    for attempt in ("mine", "mine again"):
        with pytest.raises(HTTPException) as e:
            save(m1, f"x = '{attempt}'")
        assert e.value.status_code == 409
        assert "reload" in e.value.detail
        assert egret.drafts[APP] == theirs
    assert m1.storage.base_revision == 1
    save(m2, "x = 3")
    assert egret.drafts[APP]["revision"] == 3
    assert "x = 3" in egret.drafts[APP]["source"]
    assert m2.storage.base_revision == 3


def test_3_two_managers_same_base_second_save_conflicts(setup):
    """Storage unit: two managers loaded at the same revision. (A second browser tab in marimo
    edit mode shares the session — kiosk/RTC, ws_session_connector.py:108-122 — so this is not
    "two tabs"; it pins that bases are per manager, never shared.)"""
    egret, workdir, _, ws = setup
    m_a = ws.load(f"{APP}.py")
    m_b = ws.load(f"{APP}.py")
    assert m_a.storage is not m_b.storage
    assert m_a.storage.base_revision == m_b.storage.base_revision == 1
    save(m_a, "x = 'm_a'")
    assert egret.drafts[APP]["revision"] == 2
    with pytest.raises(HTTPException) as e:
        save(m_b, "x = 'm_b'")
    assert e.value.status_code == 409
    assert egret.drafts[APP]["revision"] == 2
    assert "x = 'm_a'" in egret.drafts[APP]["source"]
    assert "m_b" not in egret.drafts[APP]["source"]


def test_4_consecutive_saves_advance_the_base(setup):
    egret, workdir, _, ws = setup
    m = ws.load(f"{APP}.py")
    save(m, "x = 2")
    assert (egret.drafts[APP]["revision"], m.storage.base_revision) == (2, 2)
    save(m, "x = 3")
    assert (egret.drafts[APP]["revision"], m.storage.base_revision) == (3, 3)
    assert "x = 3" in egret.drafts[APP]["source"]
    assert (workdir / f"{APP}.py").read_text() == egret.drafts[APP]["source"]


@pytest.mark.parametrize("where", ["outside", "not-uuid", "nested"])
def test_5_other_files_load_untouched(setup, tmp_path, monkeypatch, where):
    egret, workdir, api, _ = setup
    fetches = []
    monkeypatch.setattr(api, "get_draft", lambda app_id: fetches.append(app_id))
    if where == "outside":
        root = tmp_path / "elsewhere"
        f = root / f"{APP}.py"
    elif where == "not-uuid":
        root = workdir
        f = root / "notes.py"
    else:
        root = workdir
        f = root / "sub" / f"{APP}.py"
    f.parent.mkdir(parents=True, exist_ok=True)
    f.write_text(notebook("y = 9"))
    m = DirectoryWorkspace(str(root), include_markdown=False).load(str(f.relative_to(root)))
    assert not isinstance(m.storage, EgretStorage)
    assert f.read_text() == notebook("y = 9")
    assert fetches == []


def test_6_413_is_http_exception_413_with_server_message(setup):
    egret, workdir, _, ws = setup
    m = ws.load(f"{APP}.py")
    with pytest.raises(HTTPException) as e:
        save(m, "x = '" + "a" * (1024 * 1024) + "'")
    assert e.value.status_code == 413
    assert "source too large" in e.value.detail
    assert egret.drafts[APP]["revision"] == 1
    assert m.storage.base_revision == 1


def test_first_load_of_missing_file_fetches_it(setup):
    egret, workdir, _, ws = setup
    assert not (workdir / f"{APP}.py").exists()
    m = ws.load(f"{APP}.py")
    assert isinstance(m.storage, EgretStorage)
    assert (workdir / f"{APP}.py").read_text() == notebook("x = 1")


def test_other_gateway_error_on_save_is_502(setup, monkeypatch):
    egret, workdir, api, ws = setup
    m = ws.load(f"{APP}.py")
    egret.pod_tokens.clear()  # the pod token is now rejected (401 from the pod API)
    with pytest.raises(HTTPException) as e:
        save(m, "x = 2")
    assert e.value.status_code == 502
    assert m.storage.base_revision == 1


class OrphanedSession:
    """A stand-in for marimo's SessionImpl (which needs a kernel), with the members the
    real SessionRepository, EditModeResumeStrategy and SessionManager.close_session use."""

    def __init__(self, file_key: str, app_file_manager) -> None:
        self.initialization_id = file_key
        self.app_file_manager = app_file_manager
        self.closed = False

    def connection_state(self) -> ConnectionState:
        return ConnectionState.ORPHANED

    def kernel_state(self) -> KernelState:
        return KernelState.RUNNING

    def close(self) -> None:
        self.closed = True


def session_manager(ws) -> SessionManager:
    return SessionManager(
        workspace=ws,
        mode=SessionMode.EDIT,
        quiet=True,
        include_code=True,
        lsp_server=NoopLspServer(),
        config_manager=get_default_config_manager(current_path=None),
        cli_args={},
        argv=None,
        auth_token=None,
        redirect_console_to_browser=False,
        ttl_seconds=None,
    )


def test_reload_after_conflict_closes_the_stale_session_and_recovers(setup):
    egret, workdir, _, ws = setup
    key = f"{APP}.py"
    sm = session_manager(ws)
    stale = OrphanedSession(key, sm.app_manager(key))
    sm._repository.add_sync("old-sid", stale)
    other_author_saves(egret, "x = 'theirs'")
    with pytest.raises(HTTPException) as e:
        save(stale.app_file_manager, "x = 'mine'")
    assert e.value.status_code == 409

    # The browser reloads: ws_session_connector step 4 asks to resume for the file.
    assert sm.maybe_resume_session("new-sid", key) is None
    assert stale.closed
    assert sm.get_session_by_file_key(key) is None

    # Step 5 then creates a new session, whose manager comes from workspace.load.
    fresh = sm.app_manager(key)
    assert fresh.storage.base_revision == 2
    save(fresh, "x = 'mine, on top of theirs'")
    assert egret.drafts[APP]["revision"] == 3


def test_reload_without_conflict_still_resumes(setup):
    egret, workdir, _, ws = setup
    key = f"{APP}.py"
    sm = session_manager(ws)
    session = OrphanedSession(key, sm.app_manager(key))
    sm._repository.add_sync("old-sid", session)
    save(session.app_file_manager, "x = 2")
    assert sm.maybe_resume_session("new-sid", key) is session
    assert not session.closed
    assert sm._repository.get_session_id(session) == "new-sid"


def test_fetch_of_deleted_app_is_404(setup):
    egret, workdir, _, ws = setup
    del egret.drafts[APP]
    with pytest.raises(HTTPException) as e:
        ws.load(f"{APP}.py")
    assert e.value.status_code == 404


def test_load_waits_for_the_notebook_file_lock(setup):
    egret, workdir, _, ws = setup
    path = workdir / f"{APP}.py"
    path.write_text("old")
    held, release, done = threading.Event(), threading.Event(), threading.Event()

    def holder() -> None:  # stands in for a save in progress (file_manager.py:227-266)
        with notebook_file_lock(str(path)):
            held.set()
            release.wait(5)

    threading.Thread(target=holder).start()
    held.wait(5)
    threading.Thread(target=lambda: (ws.load(path.name), done.set())).start()
    assert not done.wait(0.5)
    assert path.read_text() == "old"
    release.set()
    assert done.wait(5)
    assert path.read_text() == notebook("x = 1")


# --- spec §5.1: one storage writes one draft; renaming is egret's ---------------


def _events(egret, kind: str) -> list[dict]:
    return [e for e in egret.events if e["kind"] == kind]


def test_copy_through_marimo_never_puts_another_files_content_into_the_draft(setup):
    """marimo's copy endpoint writes the copy through the SOURCE notebook's storage."""
    from marimo._server.models.models import CopyNotebookRequest

    egret, workdir, _, ws = setup
    m = ws.load(f"{APP}.py")
    other = workdir / "other.py"
    other.write_text(notebook("z = 'other'"))
    before = dict(egret.drafts[APP])
    with pytest.raises(HTTPException) as e:
        m.copy(CopyNotebookRequest(source=str(other), destination=str(workdir / "copy.py")))
    assert e.value.status_code == 403
    assert egret.drafts[APP] == before
    assert _events(egret, "draft_saved") == [] and _events(egret, "draft_conflict") == []
    assert not (workdir / "copy.py").exists()
    # The bound file still saves.
    save(m, "x = 2")
    assert egret.drafts[APP]["revision"] == 2


def test_copy_of_the_app_itself_to_a_new_name_is_refused_too(setup):
    from marimo._server.models.models import CopyNotebookRequest

    egret, workdir, _, ws = setup
    m = ws.load(f"{APP}.py")
    with pytest.raises(HTTPException) as e:
        m.copy(CopyNotebookRequest(source=str(workdir / f"{APP}.py"), destination=str(workdir / "dup.py")))
    assert e.value.status_code == 403
    assert egret.drafts[APP]["revision"] == 1 and _events(egret, "draft_saved") == []
    assert not (workdir / "dup.py").exists()


def test_rename_through_marimo_is_refused_and_moves_nothing(setup):
    egret, workdir, _, ws = setup
    m = ws.load(f"{APP}.py")
    app_file = workdir / f"{APP}.py"
    with pytest.raises(HTTPException) as e:
        m.rename(str(workdir / "renamed.py"))
    assert e.value.status_code == 403
    assert "renaming is done in egret" in e.value.detail
    assert app_file.read_text() == notebook("x = 1")
    assert not (workdir / "renamed.py").exists()
    assert m.path is not None and Path(m.path).resolve() == app_file.resolve()
    assert egret.drafts[APP]["revision"] == 1 and _events(egret, "draft_saved") == []
    save(m, "x = 2")  # still bound to, and saving, the app's draft
    assert egret.drafts[APP]["revision"] == 2


def test_write_to_any_path_but_the_bound_file_is_refused_before_egret_is_asked(setup, monkeypatch):
    egret, workdir, api, ws = setup
    m = ws.load(f"{APP}.py")
    puts = []
    monkeypatch.setattr(api, "put_draft", lambda *a: puts.append(a) or 99)
    for path in (workdir / "x.py", workdir / "sub" / f"{APP}.py", workdir.parent / f"{APP}.py"):
        with pytest.raises(HTTPException) as e:
            m.storage.write(path, "content")
        assert e.value.status_code == 403
    assert puts == []
    # A relative path to the bound file (the launcher chdirs to the workdir) is the bound file.
    monkeypatch.chdir(workdir)
    m.storage.write(Path(f"{APP}.py"), notebook("x = 5"))
    assert len(puts) == 1 and m.storage.base_revision == 99
