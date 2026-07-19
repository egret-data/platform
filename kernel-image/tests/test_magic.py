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
