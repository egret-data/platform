"""EgretStorage against marimo's real workspace and file manager (Task 2)."""

import pytest
from marimo._ast.cell import CellConfig
from marimo._server.models.models import SaveNotebookRequest
from marimo._server.workspace import DirectoryWorkspace, NotebookWorkspace
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
    original = NotebookWorkspace.load
    install_storage_patch(workdir, api)
    yield egret, workdir, api, DirectoryWorkspace(str(workdir), include_markdown=False)
    NotebookWorkspace.load = original


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
    m2 = ws.load(f"{APP}.py")  # a later request / reload: new manager on the latest draft
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


def test_3_two_tabs_same_author_second_save_conflicts(setup):
    egret, workdir, _, ws = setup
    t1 = ws.load(f"{APP}.py")
    t2 = ws.load(f"{APP}.py")
    assert t1.storage is not t2.storage
    assert t1.storage.base_revision == t2.storage.base_revision == 1
    save(t1, "x = 't1'")
    assert egret.drafts[APP]["revision"] == 2
    with pytest.raises(HTTPException) as e:
        save(t2, "x = 't2'")
    assert e.value.status_code == 409
    assert egret.drafts[APP]["revision"] == 2
    assert "x = 't1'" in egret.drafts[APP]["source"]
    assert "t2" not in egret.drafts[APP]["source"]


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
