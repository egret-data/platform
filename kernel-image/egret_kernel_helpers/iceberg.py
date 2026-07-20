"""Direct-Iceberg scratch plane — read/write the workspace's PRIVATE namespace via
a namespace-scoped catalog token. Governed data still goes through trino()/
starrocks(). The target namespace is FORCED to the workspace's reserved one, so
this helper cannot address anything outside it (the catalog guard also rejects).

`name` is always a SINGLE table name placed in `runtime.namespace`; it is never
split into namespace parts, so a dotted name (e.g. "other.t") stays a literal
table inside the reserved namespace — it cannot escape it. This is the
confinement invariant locked by tests/test_iceberg.py.
"""


def _catalog(runtime):
    """Build a RestCatalog from the runtime's namespace-scoped catalog token and
    return (catalog, reserved_namespace_tuple). Raises if the workspace has no
    notebook warehouse configured (no catalog token in the manifest)."""
    if not getattr(runtime, "catalog_jwt", None):
        raise RuntimeError(
            "egret: notebook warehouse not configured — set it in workspace settings"
        )
    from pyiceberg.catalog.rest import RestCatalog
    cat = RestCatalog(
        "egret",
        uri=runtime.catalog_uri,
        token=runtime.catalog_jwt,
        warehouse=runtime.catalog_warehouse,
    )
    return cat, tuple(runtime.namespace)


def write_iceberg(df, name, mode="append", *, runtime=None):
    """Write a Daft DataFrame to ``<workspace-namespace>.<name>`` (Iceberg, direct).

    ``name`` is a single table name — it is always placed in the reserved
    namespace, never interpreted as catalog/schema. ``mode`` is Daft's
    write mode ("append" | "overwrite"). Returns Daft's write-metrics DataFrame.
    """
    from .runtime import load_runtime
    rt = runtime if runtime is not None else load_runtime()
    cat, ns = _catalog(rt)
    cat.create_namespace_if_not_exists(ns)
    ident = ns + (name,)
    try:
        tbl = cat.load_table(ident)
    except Exception:  # noqa: BLE001 — any "not found" → create it
        tbl = cat.create_table(ident, schema=df.to_arrow().schema)
    return df.write_iceberg(tbl, mode=mode)


def read_iceberg(name, *, runtime=None):
    """Read ``<workspace-namespace>.<name>`` back as a lazy Daft DataFrame (direct)."""
    import daft
    from .runtime import load_runtime
    rt = runtime if runtime is not None else load_runtime()
    cat, ns = _catalog(rt)
    return daft.read_iceberg(cat.load_table(ns + (name,)))
