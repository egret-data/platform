import json
from egret_kernel_helpers.runtime import load_runtime

MANIFEST = {
    "username": "alice",
    "catalog": {"url": "http://catalog:8181", "warehouse": "main",
                "namespace": "notebook_alice", "token": "tok-abc"},
    "engines": [{"id": "11111111-1111-1111-1111-111111111111", "name": "trino-dev",
                 "engine": "trino", "enabled": True}],
    "catalogs": [{"id": "22222222-2222-2222-2222-222222222222", "name": "lakehouse", "enabled": True}],
}

def test_load_runtime_parses_manifest():
    rt = load_runtime({"EGRET_NOTEBOOK_RUNTIME": json.dumps(MANIFEST)})
    assert rt.username == "alice"
    assert rt.catalog.url == "http://catalog:8181"
    assert rt.catalog.namespace == "notebook_alice"
    assert rt.catalog.token == "tok-abc"
    assert rt.engines[0].name == "trino-dev"
    assert rt.engines[0].id == "11111111-1111-1111-1111-111111111111"

def test_load_runtime_missing_env_returns_none():
    assert load_runtime({}) is None
