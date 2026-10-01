import types

import polars as pl
import pytest
from marimo._sql.engines.dbapi import DBAPIEngine

from egret_marimo import client
from egret_marimo.client import GatewayError, SessionExpired, connect, sql


@pytest.fixture(autouse=True)
def token(egret, monkeypatch):
    egret.session_tokens["tok"] = {"user": "alice"}
    monkeypatch.setenv("EGRET_TEST_TOKEN", "tok")
    monkeypatch.delenv("EGRET_TEST_INCLUDE_PID", raising=False)


def test_execute_sets_description_and_rows(egret):
    cur = connect("db").cursor().execute("select 1", (5,))
    assert [d[0] for d in cur.description] == ["viewer", "sql"]
    assert cur.fetchall() == [("alice", "select 1")]
    assert cur.rowcount == 1
    assert cur.fetchone() == ("alice", "select 1")
    assert egret.queries[-1]["params"] == [5]
    assert egret.queries[-1]["pid"] is None


def test_pid_only_when_test_flag(egret, monkeypatch):
    monkeypatch.setenv("EGRET_TEST_INCLUDE_PID", "1")
    sql("select 1", "db")
    assert egret.queries[-1]["pid"] is not None


def test_sql_returns_polars(egret):
    df = sql("select 1", "db")
    assert isinstance(df, pl.DataFrame)
    assert df.columns == ["viewer", "sql"]
    assert egret.queries[-1]["database"] == "db"


def test_401_raises_session_expired(monkeypatch):
    monkeypatch.setenv("EGRET_TEST_TOKEN", "bad")
    with pytest.raises(SessionExpired):
        sql("select 1", "db")


def test_400_raises_gateway_error_with_message():
    with pytest.raises(GatewayError) as e:
        sql("BAD", "db")
    assert e.value.status == 400
    assert "syntax error near BAD" in e.value.detail


def test_dbapi_engine_compatible():
    assert DBAPIEngine.is_compatible(connect("db"))
    assert connect("db").dialect == "sql"
    assert connect("db", "trino").dialect == "trino"


def _stub_request(monkeypatch, request):
    import marimo as mo

    monkeypatch.setattr(mo, "app_meta", lambda: types.SimpleNamespace(request=request))


def test_session_token_from_request_header(monkeypatch):
    monkeypatch.delenv("EGRET_TEST_TOKEN")
    _stub_request(monkeypatch, types.SimpleNamespace(headers={"X-Egret-App-Token": "abc"}))
    assert client._session_token() == "abc"


def test_session_token_raises_without_request(monkeypatch):
    monkeypatch.delenv("EGRET_TEST_TOKEN")
    _stub_request(monkeypatch, None)
    with pytest.raises(RuntimeError):
        client._session_token()


def test_session_token_raises_without_header(monkeypatch):
    monkeypatch.delenv("EGRET_TEST_TOKEN")
    _stub_request(monkeypatch, types.SimpleNamespace(headers={}))
    with pytest.raises(RuntimeError):
        client._session_token()


def test_test_token_ignored_outside_pytest(monkeypatch):
    monkeypatch.delenv("PYTEST_CURRENT_TEST")
    _stub_request(monkeypatch, None)
    with pytest.raises(RuntimeError):
        client._session_token()


def test_unreachable_gateway_is_gateway_error(monkeypatch):
    monkeypatch.setenv("EGRET_URL", "http://127.0.0.1:1")
    with pytest.raises(GatewayError) as e:
        sql("select 1", "db")
    assert e.value.status == 0
