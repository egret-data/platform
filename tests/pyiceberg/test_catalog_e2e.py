import os, uuid
import pyarrow as pa
from pyiceberg.catalog.rest import RestCatalog
from pyiceberg.schema import Schema
from pyiceberg.types import NestedField, LongType, StringType

def catalog():
    return RestCatalog("egret", **{
        "uri": os.environ["EGRET_CATALOG_URI"],          # http://localhost:8080/catalog
        "token": os.environ["EGRET_CATALOG_TOKEN"],
        "s3.endpoint": os.environ["EGRET_TEST_S3_ENDPOINT"],
        "s3.access-key-id": os.environ.get("EGRET_TEST_S3_KEY", "rustfsadmin"),
        "s3.secret-access-key": os.environ.get("EGRET_TEST_S3_SECRET", "rustfsadmin"),
        "s3.path-style-access": "true",
    })

def test_create_append_load_roundtrip():
    cat = catalog()
    ns = f"e2e_{uuid.uuid4().hex[:8]}"
    cat.create_namespace(ns)
    schema = Schema(
        NestedField(1, "id", LongType(), required=True),
        NestedField(2, "name", StringType(), required=False),
    )
    tbl = cat.create_table(f"{ns}.orders", schema=schema)
    data = pa.table({"id": [1, 2, 3], "name": ["a", "b", "c"]})
    tbl.append(data)
    loaded = cat.load_table(f"{ns}.orders")
    scanned = loaded.scan().to_arrow()
    assert scanned.num_rows == 3
    assert set(scanned.column("id").to_pylist()) == {1, 2, 3}
