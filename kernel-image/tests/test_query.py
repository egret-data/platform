import json, sys, types
import pytest
from egret_kernel_helpers import query as q

MANIFEST = {
    "username": "alice",
    "catalog": {"url": "http://c", "warehouse": "w", "namespace": "notebook_alice", "token": "t"},
    "engines": [{"id": "eng-1", "name": "trino-dev", "engine": "trino", "enabled": True}],
    "catalogs": [],
}
ENV = {"EGRET_NOTEBOOK_RUNTIME": json.dumps(MANIFEST),
       "EGRET_API_URL": "http://egret:3000", "EGRET_KERNEL_TOKEN": "egk_test"}

def _fake_daft(monkeypatch):
    captured = {}
    fake = types.ModuleType("daft")
    def _from_pydict(d):
        captured["data"] = d
        return "DAFT_DF"
    fake.from_pydict = _from_pydict
    monkeypatch.setitem(sys.modules, "daft", fake)
    return captured

def test_query_posts_and_builds_dataframe(monkeypatch):
    captured = _fake_daft(monkeypatch)
    calls = {}
    def fake_post(url, body, token):
        calls["url"] = url; calls["body"] = json.loads(body); calls["token"] = token
        return {"columns": [{"name": "n", "type": "integer"}], "rows": [[1], [2]], "truncated": False}
    df = q.query("trino-dev", "SELECT 1", env=ENV, http=fake_post)
    assert df == "DAFT_DF"
    assert calls["url"] == "http://egret:3000/api/kernel/query"
    assert calls["token"] == "egk_test"
    # engine name resolved to id; sql + max_rows forwarded
    assert calls["body"] == {"engine": "eng-1", "sql": "SELECT 1", "max_rows": 10000}
    # rows pivoted into column-oriented dict for daft.from_pydict
    assert captured["data"] == {"n": [1, 2]}

def test_query_requires_configuration():
    with pytest.raises(RuntimeError):
        q.query("trino-dev", "SELECT 1", env={}, http=lambda *a: {})
