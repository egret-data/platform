import json
from egret_kernel_helpers.runtime import load_runtime

MANIFEST = {
    "username": "alice",
    "engines": [{"id": "11111111-1111-1111-1111-111111111111", "name": "trino-dev",
                 "engine": "trino", "enabled": True}],
}

def test_load_runtime_parses_manifest():
    rt = load_runtime({"EGRET_NOTEBOOK_RUNTIME": json.dumps(MANIFEST)})
    assert rt.username == "alice"
    assert rt.engines[0].name == "trino-dev"
    assert rt.engines[0].id == "11111111-1111-1111-1111-111111111111"

def test_load_runtime_missing_env_returns_none():
    assert load_runtime({}) is None


def test_load_runtime_parses_catalog_fields():
    manifest = dict(
        MANIFEST,
        catalog_jwt="cat.jwt.token",
        catalog_jwt_expires_at="2099-01-01T00:00:00Z",
        catalog_uri="http://localhost:8181/catalog",
        catalog_warehouse="wh-lakekeeper-id",
        namespace=["notebook"],
    )
    rt = load_runtime({"EGRET_NOTEBOOK_RUNTIME": json.dumps(manifest)})
    assert rt.catalog_jwt == "cat.jwt.token"
    assert rt.catalog_uri == "http://localhost:8181/catalog"
    assert rt.catalog_warehouse == "wh-lakekeeper-id"
    assert rt.namespace == ["notebook"]


def test_load_runtime_catalog_fields_default_when_absent():
    rt = load_runtime({"EGRET_NOTEBOOK_RUNTIME": json.dumps(MANIFEST)})
    assert rt.catalog_jwt is None
    assert rt.catalog_uri is None
    assert rt.catalog_warehouse is None
    assert rt.namespace == []
