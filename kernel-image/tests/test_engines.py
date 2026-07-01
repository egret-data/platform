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


def test_trino_conn_factory_wires_jwt_and_url(monkeypatch):
    import sys
    env = {"EGRET_API_URL": "http://egret", "EGRET_KERNEL_TOKEN": "kt"}
    cache = {"trino": {"jwt": "JWT", "expires_at": "2999-01-01T00:00:00Z",
                       "kind": "trino", "host": "trino", "port": 8080}}
    captured = {}
    fake_daft = type("D", (), {"read_sql": staticmethod(
        lambda sql, conn, **kw: captured.update(conn=conn) or "DF")})
    monkeypatch.setitem(sys.modules, "daft", fake_daft)

    engine_calls = {}

    class _Eng:
        def connect(self):
            return "CONN"

    def _create_engine(url, connect_args=None):
        engine_calls.update(url=url, connect_args=connect_args)
        return _Eng()

    fake_sqlalchemy = type("SA", (), {"create_engine": staticmethod(_create_engine)})
    monkeypatch.setitem(sys.modules, "sqlalchemy", fake_sqlalchemy)

    auth_calls = {}

    def _JWTAuthentication(jwt):
        auth_calls["jwt"] = jwt
        return ("AUTH", jwt)

    fake_trino = type("T", (), {})
    fake_trino_auth = type("TA", (), {"JWTAuthentication": staticmethod(_JWTAuthentication)})
    monkeypatch.setitem(sys.modules, "trino", fake_trino)
    monkeypatch.setitem(sys.modules, "trino.auth", fake_trino_auth)

    E.trino("SELECT 1", env=env, cache=cache)
    conn = captured["conn"]
    assert conn() == "CONN"
    assert engine_calls["url"] == "trino://egret@trino:8080"
    assert engine_calls["connect_args"]["http_scheme"] == "https"
    assert auth_calls["jwt"] == "JWT"
    assert engine_calls["connect_args"]["auth"] == ("AUTH", "JWT")


def test_lenenc_boundaries():
    assert E._lenenc(250) == b"\xfa"
    assert E._lenenc(251) == b"\xfc\xfb\x00"
    assert E._lenenc(65535) == b"\xfc\xff\xff"
    assert E._lenenc(65536) == b"\xfd\x00\x00\x01"
    assert E._lenenc(16777216) == b"\xfe\x00\x00\x00\x01\x00\x00\x00\x00"


def test_openid_handler_writes_jwt_frame():
    written = {}

    class _Reply:
        def check_error(self):
            pass

    class _Con:
        def write_packet(self, data):
            written["data"] = data

        def _read_packet(self):
            return _Reply()

    Handler = E._openid_handler("mytoken")
    handler = Handler(_Con())
    handler.authenticate(None)
    assert written["data"] == b"\x01" + E._lenenc(len(b"mytoken")) + b"mytoken"


def test_starrocks_builds_read_sql_with_jwt(monkeypatch):
    import sys
    env = {"EGRET_API_URL": "http://egret", "EGRET_KERNEL_TOKEN": "kt"}
    cache = {"sr": {"jwt": "JWT", "expires_at": "2999-01-01T00:00:00Z",
                    "kind": "starrocks", "host": "sr", "port": 9030}}
    captured = {}
    fake_daft = type("D", (), {"read_sql": staticmethod(
        lambda sql, conn, **kw: captured.update(sql=sql, conn=conn) or "DF")})
    monkeypatch.setitem(sys.modules, "daft", fake_daft)
    out = E.starrocks("SELECT 1", engine="sr", env=env, cache=cache)
    assert out == "DF"
    assert captured["sql"] == "SELECT 1"
    assert callable(captured["conn"])


def test_starrocks_conn_factory_uses_pymysql_openid(monkeypatch):
    import sys
    env = {"EGRET_API_URL": "http://egret", "EGRET_KERNEL_TOKEN": "kt"}
    cache = {"sr": {"jwt": "JWT", "expires_at": "2999-01-01T00:00:00Z",
                    "kind": "starrocks", "host": "sr", "port": 9030}}
    seen = {}
    def fake_connect(**kw):
        seen["connect_kwargs"] = kw
        return "PYMYSQL_CONN"
    fake_pymysql = type("P", (), {"connect": staticmethod(fake_connect)})
    def fake_create_engine(url, **kw):
        seen["url"] = url
        seen["creator"] = kw.get("creator")
        class _Eng:
            def connect(self):
                return kw["creator"]()
        return _Eng()
    fake_sa = type("SA", (), {"create_engine": staticmethod(fake_create_engine)})
    # daft.read_sql returns the factory so the test can invoke it
    fake_daft = type("D", (), {"read_sql": staticmethod(lambda sql, conn, **kw: conn)})
    monkeypatch.setitem(sys.modules, "pymysql", fake_pymysql)
    monkeypatch.setitem(sys.modules, "sqlalchemy", fake_sa)
    monkeypatch.setitem(sys.modules, "daft", fake_daft)
    factory = E.starrocks("SELECT 1", engine="sr", env=env, cache=cache)
    conn = factory()                    # invoke the connection factory -> eng.connect() -> creator()
    assert conn == "PYMYSQL_CONN"
    assert seen["url"] == "mysql+pymysql://"
    assert callable(seen["creator"])
    kw = seen["connect_kwargs"]
    assert kw["host"] == "sr" and kw["port"] == 9030 and kw["user"] == "egret"
    assert "authentication_openid_connect_client" in kw["auth_plugin_map"]


def test_iceberg_reads_via_catalog(monkeypatch):
    import sys
    loaded = {}
    class FakeCat:
        def load_table(self, name):
            loaded["name"] = name
            return f"TBL:{name}"
    monkeypatch.setattr(E, "catalog", lambda *a, **k: FakeCat())
    captured = {}
    fake_daft = type("D", (), {"read_iceberg": staticmethod(
        lambda t, **kw: captured.update(tbl=t) or "DF")})
    monkeypatch.setitem(sys.modules, "daft", fake_daft)
    out = E.iceberg("sales.orders")
    assert out == "DF"
    assert loaded["name"] == "sales.orders"
    assert captured["tbl"] == "TBL:sales.orders"
