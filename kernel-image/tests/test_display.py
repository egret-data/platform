import sys, types
from egret_kernel_helpers.display import display

def _fake_ipython(monkeypatch):
    shown = []
    mod = types.ModuleType("IPython.display")
    mod.display = lambda obj: shown.append(obj)
    monkeypatch.setitem(sys.modules, "IPython", types.ModuleType("IPython"))
    monkeypatch.setitem(sys.modules, "IPython.display", mod)
    return shown

class _FakeDF:
    def __init__(self): self.limited = None
    def limit(self, n): self.limited = n; return self

def _fake_daft_with_df(monkeypatch):
    mod = types.ModuleType("daft")
    mod.DataFrame = _FakeDF
    monkeypatch.setitem(sys.modules, "daft", mod)

def test_display_limits_dataframe(monkeypatch):
    shown = _fake_ipython(monkeypatch)
    _fake_daft_with_df(monkeypatch)
    df = _FakeDF()
    display(df, n=5)
    assert df.limited == 5
    assert shown and shown[0] is df  # limit() returns self in the fake

def test_display_falls_through_for_non_dataframe(monkeypatch):
    shown = _fake_ipython(monkeypatch)
    _fake_daft_with_df(monkeypatch)
    display({"a": 1})
    assert shown == [{"a": 1}]
