import datetime as dt
import json
import pytest
from egret_kernel_helpers import engines as E


def _fake_http(responses):
    calls = []
    def http(url, body, token):
        calls.append((url, json.loads(body.decode()), token))
        return responses.pop(0)
    http.calls = calls
    return http


def test_engine_token_fetches_and_caches(monkeypatch):
    env = {"EGRET_API_URL": "http://egret", "EGRET_KERNEL_TOKEN": "kt"}
    http = _fake_http([
        {"jwt": "J1", "expires_at": "2999-01-01T00:00:00Z",
         "kind": "trino", "host": "trino", "port": 8080},
    ])
    cache = {}
    tok = E._engine_token("trino", env=env, http=http, cache=cache)
    assert tok["jwt"] == "J1" and tok["host"] == "trino" and tok["port"] == 8080
    # Cached: a second call (far from expiry) does not hit http again.
    tok2 = E._engine_token("trino", env=env, http=http, cache=cache)
    assert tok2["jwt"] == "J1"
    assert len(http.calls) == 1
    assert http.calls[0][0] == "http://egret/api/kernel/engine-token"
    assert http.calls[0][1] == {"engine": "trino"}


def test_engine_token_refetches_when_near_expiry(monkeypatch):
    env = {"EGRET_API_URL": "http://egret", "EGRET_KERNEL_TOKEN": "kt"}
    # Cached entry expiring within the refresh margin (10s from now) → stale.
    near = (E._now() + dt.timedelta(seconds=10)).strftime("%Y-%m-%dT%H:%M:%SZ")
    cache = {"trino": {"jwt": "OLD", "expires_at": near,
                       "kind": "trino", "host": "trino", "port": 8080}}
    http = _fake_http([
        {"jwt": "NEW", "expires_at": "2999-01-01T00:00:00Z",
         "kind": "trino", "host": "trino", "port": 8080},
    ])
    tok = E._engine_token("trino", env=env, http=http, cache=cache)
    # A fresh HTTP call fired; the stale cached entry was NOT used.
    assert len(http.calls) == 1
    assert tok["jwt"] == "NEW"


def test_engine_token_missing_env_raises():
    with pytest.raises(RuntimeError, match="not configured"):
        E._engine_token("trino", env={}, http=_fake_http([]), cache={})


def test_trino_builds_read_sql_with_jwt(monkeypatch):
    import sys
    env = {"EGRET_API_URL": "http://egret", "EGRET_KERNEL_TOKEN": "kt"}
    # seed the cache so _engine_token doesn't hit http (far-future expiry)
    cache = {"trino": {"jwt": "JWT", "expires_at": "2999-01-01T00:00:00Z",
                       "kind": "trino", "host": "trino", "port": 8080}}
    captured = {}
    fake_daft = type("D", (), {"read_sql": staticmethod(
        lambda sql, conn, **kw: captured.update(sql=sql, conn=conn) or "DF")})
    monkeypatch.setitem(sys.modules, "daft", fake_daft)
    out = E.trino("SELECT 1", env=env, cache=cache)
    assert out == "DF"
    assert captured["sql"] == "SELECT 1"
    assert callable(captured["conn"])   # a connection factory, not a string
