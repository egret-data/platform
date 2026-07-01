import sys, types
from egret_kernel_helpers import magic as m


def test_daft_target_calls_daft_sql(monkeypatch):
    calls = {}
    fake_daft = types.SimpleNamespace(sql=lambda s: calls.update(sql=s) or "DF")
    monkeypatch.setitem(sys.modules, "daft", fake_daft)
    out = m.run_egret_sql("--target daft", "SELECT 1")
    assert calls["sql"] == "SELECT 1"
    assert out == "DF"


def test_engine_target_calls_query(monkeypatch):
    calls = {}
    monkeypatch.setattr(m, "query", lambda engine, sql: calls.update(engine=engine, sql=sql) or "QDF")
    out = m.run_egret_sql("--target trino", "SELECT 2")
    assert calls == {"engine": "trino", "sql": "SELECT 2"}
    assert out == "QDF"


def test_default_target_is_daft(monkeypatch):
    fake_daft = types.SimpleNamespace(sql=lambda s: "DF")
    monkeypatch.setitem(sys.modules, "daft", fake_daft)
    assert m.run_egret_sql("", "SELECT 3") == "DF"
