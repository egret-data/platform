import pytest
from egret_kernel_helpers import magic as M


def test_run_binds_dataframe_and_displays():
    engines = {"trino": lambda sql: {"sql": sql, "engine": "trino"}}
    displayed = []
    ns = {}
    M.run_egret_sql(
        "SELECT 1", target="trino", into="df",
        user_ns=ns, engines=engines, display_fn=displayed.append,
    )
    assert ns["df"] == {"sql": "SELECT 1", "engine": "trino"}
    assert displayed == [{"sql": "SELECT 1", "engine": "trino"}]


def test_run_rejects_bad_variable_name():
    with pytest.raises(ValueError):
        M.run_egret_sql("SELECT 1", target="trino", into="9bad",
                        user_ns={}, engines={"trino": lambda s: s}, display_fn=lambda x: None)


def test_run_rejects_unknown_target():
    with pytest.raises(ValueError):
        M.run_egret_sql("SELECT 1", target="nope", into="df",
                        user_ns={}, engines={"trino": lambda s: s}, display_fn=lambda x: None)
