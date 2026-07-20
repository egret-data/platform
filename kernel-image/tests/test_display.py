import sys, types
from egret_kernel_helpers.display import display, _table_bundle, _jsonable

def _fake_ipython(monkeypatch):
    shown = []
    mod = types.ModuleType("IPython.display")
    mod.display = lambda obj, raw=False: shown.append((obj, raw))
    monkeypatch.setitem(sys.modules, "IPython", types.ModuleType("IPython"))
    monkeypatch.setitem(sys.modules, "IPython.display", mod)
    return shown

class _Field:
    def __init__(self, name, dtype): self.name = name; self.dtype = dtype

class _Schema:
    def __init__(self, fields): self._f = fields
    def __iter__(self): return iter(self._f)

class _FakeDF:
    """Fake Daft DataFrame: limit()->self, collect()->self, to_pydict()/schema()."""
    def __init__(self, data, dtypes):
        self._data = data; self._dtypes = dtypes; self.limited = None; self.collected = False
    def limit(self, n): self.limited = n; return self
    def collect(self): self.collected = True; return self
    def to_pydict(self): return self._data
    def schema(self): return _Schema([_Field(k, self._dtypes[k]) for k in self._data])

def _fake_daft_with_df(monkeypatch):
    mod = types.ModuleType("daft")
    mod.DataFrame = _FakeDF
    monkeypatch.setitem(sys.modules, "daft", mod)

def test_display_emits_table_mime_bundle(monkeypatch):
    shown = _fake_ipython(monkeypatch)
    _fake_daft_with_df(monkeypatch)
    df = _FakeDF({"x": [1, 2, 3], "s": [4, 5, 6]}, {"x": "Int64", "s": "Int64"})
    display(df, n=50)
    assert df.limited == 50 and df.collected  # materialized the capped frame
    (obj, raw) = shown[0]
    assert raw is True
    bundle = obj["application/vnd.egret.table+json"]
    assert bundle["columns"] == [{"name": "x", "type": "Int64"}, {"name": "s", "type": "Int64"}]
    assert bundle["rows"] == [[1, 4], [2, 5], [3, 6]]  # row-major, aligned to columns
    assert bundle["truncated"] is False
    assert "text/plain" in obj  # fallback present

def test_table_bundle_marks_truncated_when_capped(monkeypatch):
    _fake_daft_with_df(monkeypatch)
    df = _FakeDF({"a": [1, 2]}, {"a": "Int64"})
    b = _table_bundle(df, 2)  # got exactly n rows → maybe more
    assert b["truncated"] is True

def test_jsonable_coerces_non_primitives():
    assert _jsonable(1) == 1 and _jsonable("a") == "a" and _jsonable(None) is None
    import datetime
    assert isinstance(_jsonable(datetime.date(2026, 7, 20)), str)

def test_display_falls_through_for_non_dataframe(monkeypatch):
    shown = _fake_ipython(monkeypatch)
    _fake_daft_with_df(monkeypatch)
    display({"a": 1})
    assert shown == [({"a": 1}, False)]
