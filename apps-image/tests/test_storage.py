"""EgretStorage against marimo's real workspace and file manager (Task 2)."""

import threading

import pytest
from marimo._ast.cell import CellConfig
from marimo._server.models.models import SaveNotebookRequest
from marimo._server.workspace import DirectoryWorkspace, NotebookWorkspace
from marimo._utils.http import HTTPException

from egret_marimo.podapi import PodApi
from egret_marimo.storage import BaseRevisions, EgretStorage, install_storage_patch

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
    revisions = BaseRevisions()
    original = NotebookWorkspace.load
    install_storage_patch(workdir, api, revisions)
    yield egret, workdir, api, revisions, DirectoryWorkspace(str(workdir), include_markdown=False)
    NotebookWorkspace.load = original


def other_author_saves(egret, code: str) -> None:
    """Another author's save, straight through the fake's PUT (not through this pod's registry)."""
    api = PodApi(egret.base_url, "pod-t")
    d = api.get_draft(APP)
    api.put_draft(APP, d.revision, notebook(code))


def test_1_first_load_fetches_writes_and_records_base(setup):
    egret, workdir, _, revisions, ws = setup
    m = ws.load(f"{APP}.py")
    assert (workdir / f"{APP}.py").read_text() == notebook("x = 1")
    assert revisions.get(APP) == 1
    assert isinstance(m.storage, EgretStorage)
    assert [c for c in m.app.cell_manager.codes()] == ["x = 1"]


def test_2_stale_manager_cannot_overwrite_another_authors_save(setup):
    egret, workdir, _, revisions, ws = setup
    m1 = ws.load(f"{APP}.py")
    other_author_saves(egret, "x = 'theirs'")
    assert egret.drafts[APP]["revision"] == 2
    ws.load(f"{APP}.py")  # a second request, as marimo does (SessionManager.app_manager)
    with pytest.raises(HTTPException) as e:
        save(m1, "x = 'mine'")
    assert e.value.status_code == 409
    assert "reload" in e.value.detail
    assert egret.drafts[APP] == {"source": notebook("x = 'theirs'"), "revision": 2}
    assert revisions.is_stale(APP)


def test_3_after_409_next_load_refetches_and_save_succeeds(setup):
    egret, workdir, _, revisions, ws = setup
    m1 = ws.load(f"{APP}.py")
    other_author_saves(egret, "x = 'theirs'")
    with pytest.raises(HTTPException):
        save(m1, "x = 'mine'")
    m3 = ws.load(f"{APP}.py")
    assert (workdir / f"{APP}.py").read_text() == notebook("x = 'theirs'")
    assert revisions.get(APP) == 2
    assert not revisions.is_stale(APP)
    save(m3, "x = 3")
    assert egret.drafts[APP]["revision"] == 3
    assert "x = 3" in egret.drafts[APP]["source"]


def test_4_consecutive_saves_advance_the_base(setup):
    egret, workdir, _, revisions, ws = setup
    m = ws.load(f"{APP}.py")
    save(m, "x = 2")
    assert (egret.drafts[APP]["revision"], revisions.get(APP)) == (2, 2)
    save(m, "x = 3")
    assert (egret.drafts[APP]["revision"], revisions.get(APP)) == (3, 3)
    assert "x = 3" in egret.drafts[APP]["source"]
    assert (workdir / f"{APP}.py").read_text() == egret.drafts[APP]["source"]


def test_4b_load_with_known_base_does_not_fetch_or_touch_disk(setup, monkeypatch):
    egret, workdir, api, revisions, ws = setup
    fetches = []
    real_get = api.get_draft
    monkeypatch.setattr(api, "get_draft", lambda app_id: fetches.append(app_id) or real_get(app_id))
    ws.load(f"{APP}.py")
    other_author_saves(egret, "x = 'theirs'")
    ws.load(f"{APP}.py")
    assert fetches == [APP]
    assert (workdir / f"{APP}.py").read_text() == notebook("x = 1")
    assert revisions.get(APP) == 1


@pytest.mark.parametrize("where", ["outside", "not-uuid", "nested"])
def test_5_other_files_load_untouched(setup, tmp_path, where):
    egret, workdir, _, revisions, _ = setup
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
    assert revisions.get(APP) is None


def test_6_413_is_http_exception_413_with_server_message(setup):
    egret, workdir, _, revisions, ws = setup
    m = ws.load(f"{APP}.py")
    with pytest.raises(HTTPException) as e:
        save(m, "x = '" + "a" * (1024 * 1024) + "'")
    assert e.value.status_code == 413
    assert "source too large" in e.value.detail
    assert egret.drafts[APP]["revision"] == 1
    assert revisions.get(APP) == 1 and not revisions.is_stale(APP)


def test_first_load_of_missing_file_fetches_it(setup):
    egret, workdir, _, revisions, ws = setup
    assert not (workdir / f"{APP}.py").exists()
    m = ws.load(f"{APP}.py")
    assert isinstance(m.storage, EgretStorage)
    assert (workdir / f"{APP}.py").exists()


def test_set_clears_stale_mark():
    r = BaseRevisions()
    r.set("a", 1)
    r.mark_stale("a")
    assert r.is_stale("a")
    r.set("a", 2)
    assert (r.get("a"), r.is_stale("a")) == (2, False)


def test_registry_is_thread_safe():
    r = BaseRevisions()

    def work(i: int) -> None:
        for n in range(500):
            r.set(f"app{i}", n)
            r.mark_stale(f"app{i}")
            r.is_stale(f"app{i}")
            r.get(f"app{i}")

    ts = [threading.Thread(target=work, args=(i,)) for i in range(8)]
    for t in ts:
        t.start()
    for t in ts:
        t.join()
    assert all(r.get(f"app{i}") == 499 and r.is_stale(f"app{i}") for i in range(8))
