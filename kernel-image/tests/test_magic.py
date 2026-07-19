import pytest
from egret_kernel_helpers import magic as M


class _E:
    def __init__(self, id, engine, name=None):
        self.id = id
        self.engine = engine
        self.name = name or id


def test_dispatch_resolves_engine_id_to_kind_and_passes_id():
    calls = []
    kind_fns = {"trino": lambda sql, engine: (calls.append((sql, engine)), {"df": True})[1]}
    dispatch = M.make_dispatch([_E("uuid-1", "trino", "Warehouse")], kind_fns)
    out = dispatch("uuid-1", "SELECT 1")
    assert calls == [("SELECT 1", "uuid-1")]  # the id is threaded to the helper
    assert out == {"df": True}


def test_dispatch_falls_back_to_bare_kind():
    calls = []
    kind_fns = {"trino": lambda sql, engine: calls.append(engine) or "df"}
    dispatch = M.make_dispatch([], kind_fns)
    dispatch("trino", "SELECT 1")
    assert calls == ["trino"]


def test_dispatch_unknown_target_raises():
    dispatch = M.make_dispatch([], {"trino": lambda sql, engine: None})
    with pytest.raises(ValueError):
        dispatch("nope", "SELECT 1")


def test_run_binds_and_displays():
    displayed = []
    ns = {}
    M.run_egret_sql("SELECT 1", target="trino", into="df",
                    dispatch=lambda target, sql: {"sql": sql}, user_ns=ns, display_fn=displayed.append)
    assert ns["df"] == {"sql": "SELECT 1"}
    assert displayed == [{"sql": "SELECT 1"}]


def test_run_rejects_bad_variable_name():
    with pytest.raises(ValueError):
        M.run_egret_sql("SELECT 1", target="trino", into="9bad",
                        dispatch=lambda t, s: s, user_ns={}, display_fn=lambda x: None)


class _FakeDF:
    pass


class _FakeDaft:
    DataFrame = _FakeDF

    def __init__(self):
        self.calls = []

    def sql(self, query, **frames):
        self.calls.append((query, frames))
        return {"ran": query, "frames": sorted(frames)}


def test_run_daft_local_passes_only_dataframe_frames():
    daft = _FakeDaft()
    df1, df2 = _FakeDF(), _FakeDF()
    user_ns = {"a": df1, "b": df2, "n": 5, "sql": _FakeDF()}  # 'sql' is reserved → excluded
    out = M.run_daft_local("SELECT * FROM a", user_ns, daft)
    assert daft.calls == [("SELECT * FROM a", {"a": df1, "b": df2})]
    assert out == {"ran": "SELECT * FROM a", "frames": ["a", "b"]}


def test_dispatch_routes_daft_target_to_runner():
    seen = []
    dispatch = M.make_dispatch([], {"trino": lambda s, engine: None}, daft_runner=lambda sql: seen.append(sql) or "df")
    assert dispatch("daft", "SELECT 1") == "df"
    assert seen == ["SELECT 1"]


def test_dispatch_daft_without_runner_raises():
    dispatch = M.make_dispatch([], {"trino": lambda s, engine: None})
    with pytest.raises(ValueError):
        dispatch("daft", "SELECT 1")
