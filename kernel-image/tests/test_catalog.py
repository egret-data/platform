import json, sys, types
import pytest
from egret_kernel_helpers.catalog import catalog

MANIFEST = {
    "username": "alice",
    "catalog": {"url": "http://c:8181", "warehouse": "main", "namespace": "notebook_alice", "token": "tok"},
    "engines": [], "catalogs": [],
}
ENV = {"EGRET_NOTEBOOK_RUNTIME": json.dumps(MANIFEST)}

def _fake_pyiceberg(monkeypatch):
    captured = {}
    mod_rest = types.ModuleType("pyiceberg.catalog.rest")
    class RestCatalog:
        def __init__(self, name, **props):
            captured["name"] = name; captured["props"] = props
    mod_rest.RestCatalog = RestCatalog
    monkeypatch.setitem(sys.modules, "pyiceberg", types.ModuleType("pyiceberg"))
    monkeypatch.setitem(sys.modules, "pyiceberg.catalog", types.ModuleType("pyiceberg.catalog"))
    monkeypatch.setitem(sys.modules, "pyiceberg.catalog.rest", mod_rest)
    return captured

def test_catalog_builds_rest_catalog_from_manifest(monkeypatch):
    captured = _fake_pyiceberg(monkeypatch)
    catalog(env=ENV)
    assert captured["props"] == {"uri": "http://c:8181", "warehouse": "main", "token": "tok"}

def test_catalog_without_runtime_raises():
    with pytest.raises(RuntimeError):
        catalog(env={})
