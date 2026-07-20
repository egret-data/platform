"""Unit tests for the direct-Iceberg scratch helpers (write_iceberg/read_iceberg).

pyiceberg + daft are MOCKED (no live catalog) — the real path is the Task 6 e2e.
The invariant locked here is CONFINEMENT: the target namespace is always the
workspace's reserved `runtime.namespace`, and `name` is a single table name that
is never split into namespace parts (so a dotted name cannot escape the ns)."""
import sys
import types

import pytest

from egret_kernel_helpers import iceberg as I


class _FakeTable:
    def __init__(self, identifier):
        self.identifier = identifier


class _FakeCatalog:
    """Records the args RestCatalog was built with + every identifier it saw."""

    def __init__(self, name, **kwargs):
        self.name = name
        self.kwargs = kwargs
        self.loaded = []
        self.created = []
        self.namespaces = []

    def create_namespace_if_not_exists(self, ns):
        self.namespaces.append(ns)

    def load_table(self, identifier):
        self.loaded.append(identifier)
        return _FakeTable(identifier)

    def create_table(self, identifier, schema=None):
        self.created.append((identifier, schema))
        return _FakeTable(identifier)


class _FakeDF:
    def __init__(self):
        self.written = None

    def write_iceberg(self, table, mode=None):
        self.written = (table, mode)
        return {"rows": 1}

    def to_arrow(self):
        return types.SimpleNamespace(schema="arrow-schema")


class _FakeRuntime:
    def __init__(self, jwt="tok", uri="http://egret:8080/catalog",
                 warehouse="wh1", namespace=("notebook",)):
        self.catalog_jwt = jwt
        self.catalog_uri = uri
        self.catalog_warehouse = warehouse
        self.namespace = list(namespace)


def _install_fakes(monkeypatch):
    """Inject fake `pyiceberg.catalog.rest.RestCatalog` + `daft` modules; return
    the list every constructed catalog is appended to, and the daft module."""
    built = []

    class _RestCatalog(_FakeCatalog):
        def __init__(self, name, **kwargs):
            super().__init__(name, **kwargs)
            built.append(self)

    pyiceberg = types.ModuleType("pyiceberg")
    catalog_pkg = types.ModuleType("pyiceberg.catalog")
    rest_mod = types.ModuleType("pyiceberg.catalog.rest")
    rest_mod.RestCatalog = _RestCatalog
    monkeypatch.setitem(sys.modules, "pyiceberg", pyiceberg)
    monkeypatch.setitem(sys.modules, "pyiceberg.catalog", catalog_pkg)
    monkeypatch.setitem(sys.modules, "pyiceberg.catalog.rest", rest_mod)

    daft = types.ModuleType("daft")
    daft.read_calls = []
    daft.read_iceberg = lambda table: daft.read_calls.append(table) or {"df": table}
    monkeypatch.setitem(sys.modules, "daft", daft)
    return built, daft


def test_write_iceberg_builds_rest_catalog_and_forces_namespace(monkeypatch):
    built, _ = _install_fakes(monkeypatch)
    rt = _FakeRuntime()
    df = _FakeDF()

    I.write_iceberg(df, "scratch", runtime=rt)

    assert len(built) == 1
    cat = built[0]
    # RestCatalog built with the workspace catalog wiring from the runtime.
    assert cat.kwargs["uri"] == "http://egret:8080/catalog"
    assert cat.kwargs["token"] == "tok"
    assert cat.kwargs["warehouse"] == "wh1"
    # Target identifier is the FORCED namespace + the caller's table name.
    assert cat.loaded == [("notebook", "scratch")]
    # The write went to that loaded table, with the mode.
    table, mode = df.written
    assert table.identifier == ("notebook", "scratch")
    assert mode == "append"


def test_write_iceberg_passes_mode(monkeypatch):
    _install_fakes(monkeypatch)
    df = _FakeDF()
    I.write_iceberg(df, "scratch", mode="overwrite", runtime=_FakeRuntime())
    assert df.written[1] == "overwrite"


def test_write_iceberg_creates_table_when_missing(monkeypatch):
    built, _ = _install_fakes(monkeypatch)

    # A catalog whose load_table raises → helper falls back to create_table.
    class _MissingCatalog(_FakeCatalog):
        def load_table(self, identifier):
            raise KeyError("no such table")

    monkeypatch.setattr(
        sys.modules["pyiceberg.catalog.rest"], "RestCatalog", _MissingCatalog
    )
    df = _FakeDF()
    I.write_iceberg(df, "scratch", runtime=_FakeRuntime())
    # Fell back to create_table on the forced identifier.
    assert df.written[0].identifier == ("notebook", "scratch")


def test_read_iceberg_reads_same_identifier(monkeypatch):
    built, daft = _install_fakes(monkeypatch)
    I.read_iceberg("scratch", runtime=_FakeRuntime())
    assert len(daft.read_calls) == 1
    assert daft.read_calls[0].identifier == ("notebook", "scratch")


def test_write_iceberg_without_warehouse_raises(monkeypatch):
    _install_fakes(monkeypatch)
    rt = _FakeRuntime(jwt=None)
    with pytest.raises(RuntimeError, match="notebook warehouse not configured"):
        I.write_iceberg(_FakeDF(), "x", runtime=rt)


def test_read_iceberg_without_warehouse_raises(monkeypatch):
    _install_fakes(monkeypatch)
    with pytest.raises(RuntimeError, match="notebook warehouse not configured"):
        I.read_iceberg("x", runtime=_FakeRuntime(jwt=None))


def test_dotted_name_stays_single_table_in_reserved_namespace(monkeypatch):
    """CONFINEMENT invariant: a dotted/qualified name is a LITERAL table name in
    the reserved namespace — never split into ("other","t") catalog/schema parts."""
    built, _ = _install_fakes(monkeypatch)
    df = _FakeDF()
    I.write_iceberg(df, "other.t", runtime=_FakeRuntime())
    cat = built[0]
    assert cat.loaded == [("notebook", "other.t")]
    assert df.written[0].identifier == ("notebook", "other.t")
