import json
import egret_kernel_helpers as ek

def test_exports_present():
    for name in ("query", "catalog", "display", "load_runtime", "iceberg", "trino"):
        assert hasattr(ek, name), f"missing export {name}"

def test_me_reads_username(monkeypatch):
    monkeypatch.setenv("EGRET_NOTEBOOK_RUNTIME", json.dumps({
        "username": "bob",
        "catalog": {"url": "u", "warehouse": "w", "namespace": "notebook_bob", "token": "t"},
        "engines": [], "catalogs": [],
    }))
    import importlib
    importlib.reload(ek)
    assert ek.me == "bob"
