import base64
import datetime as dt
import json
import pytest
from egret_kernel_helpers import engines as E


def _mk_jwt(preferred_username):
    """Build a real-ish 3-part JWT whose payload carries preferred_username."""
    payload = base64.urlsafe_b64encode(
        json.dumps({"preferred_username": preferred_username}).encode()
    ).rstrip(b"=").decode()
    return "h." + payload + ".sig"


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
    jwt = _mk_jwt("tester")
    cache = {"trino": {"jwt": jwt, "expires_at": "2999-01-01T00:00:00Z",
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
    assert engine_calls["url"] == "trino://tester@trino:8080"
    assert engine_calls["connect_args"]["http_scheme"] == "https"
    assert auth_calls["jwt"] == jwt
    assert engine_calls["connect_args"]["auth"] == ("AUTH", jwt)


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


def test_jwt_username_extracts_preferred_username():
    assert E._jwt_username(_mk_jwt("alice")) == "alice"
    assert E._jwt_username("garbage") == "egret"


def test_starrocks_conn_factory_uses_pymysql_openid(monkeypatch):
    import sys
    env = {"EGRET_API_URL": "http://egret", "EGRET_KERNEL_TOKEN": "kt"}
    cache = {"sr": {"jwt": _mk_jwt("tester"), "expires_at": "2999-01-01T00:00:00Z",
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
    assert kw["host"] == "sr" and kw["port"] == 9030 and kw["user"] == "tester"
    assert "authentication_openid_connect_client" in kw["auth_plugin_map"]


def test_trino_conn_factory_tls_verify(monkeypatch):
    import sys
    jwt = _mk_jwt("tester")
    cache = {"trino": {"jwt": jwt, "expires_at": "2999-01-01T00:00:00Z",
                       "kind": "trino", "host": "trino", "port": 8080}}

    def _build(env):
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
        fake_trino = type("T", (), {})
        fake_trino_auth = type("TA", (), {
            "JWTAuthentication": staticmethod(lambda j: ("AUTH", j))})
        monkeypatch.setitem(sys.modules, "trino", fake_trino)
        monkeypatch.setitem(sys.modules, "trino.auth", fake_trino_auth)
        E.trino("SELECT 1", env=env, cache=cache)
        captured["conn"]()
        return engine_calls

    # Default (var unset) → verify=True (prod-correct).
    secure = _build({"EGRET_API_URL": "http://egret", "EGRET_KERNEL_TOKEN": "kt"})
    assert secure["connect_args"]["verify"] is True

    # EGRET_ENGINE_TLS_INSECURE=1 → verify=False (dev self-signed cert).
    insecure = _build({"EGRET_API_URL": "http://egret", "EGRET_KERNEL_TOKEN": "kt",
                       "EGRET_ENGINE_TLS_INSECURE": "1"})
    assert insecure["connect_args"]["verify"] is False


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


def test_iceberg_defaults_to_private_namespace(monkeypatch):
    import sys
    loaded = {}
    class FakeCat:
        def load_table(self, name):
            loaded["name"] = name
            return f"TBL:{name}"
    monkeypatch.setattr(E, "catalog", lambda *a, **k: FakeCat())
    monkeypatch.setattr(E, "load_runtime", lambda env=None: type("R", (), {
        "catalog": type("C", (), {"namespace": "notebook_alice"})()})())
    fake_daft = type("D", (), {"read_iceberg": staticmethod(lambda t, **kw: "DF")})
    monkeypatch.setitem(sys.modules, "daft", fake_daft)
    out = E.iceberg("sales")
    assert out == "DF"
    assert loaded["name"] == "notebook_alice.sales"


def _fake_df(schema="ARROW_SCHEMA"):
    class _Schema:
        def to_pyarrow_schema(self):
            return schema
    class _DF:
        def schema(self):
            return _Schema()
        def write_iceberg(self, tbl, mode="append"):
            _DF.last = {"tbl": tbl, "mode": mode}
            return "WRITE_RESULT"
    return _DF()


def _install_fake_pyiceberg_exc(monkeypatch):
    import sys, types
    exc_mod = types.ModuleType("pyiceberg.exceptions")
    class NoSuchTableError(Exception):
        pass
    exc_mod.NoSuchTableError = NoSuchTableError
    pkg = types.ModuleType("pyiceberg")
    monkeypatch.setitem(sys.modules, "pyiceberg", pkg)
    monkeypatch.setitem(sys.modules, "pyiceberg.exceptions", exc_mod)
    return NoSuchTableError


def test_write_iceberg_creates_missing_table_then_writes(monkeypatch):
    NoSuchTableError = _install_fake_pyiceberg_exc(monkeypatch)
    calls = {}
    class FakeCat:
        def load_table(self, ident):
            calls["load"] = ident
            raise NoSuchTableError("nope")
        def create_table(self, ident, schema):
            calls["create"] = (ident, schema)
            return "NEW_TBL"
    monkeypatch.setattr(E, "catalog", lambda *a, **k: FakeCat())
    monkeypatch.setattr(E, "load_runtime", lambda env=None: type("R", (), {
        "catalog": type("C", (), {"namespace": "notebook_alice"})()})())
    df = _fake_df()
    out = E.write_iceberg(df, "sales", mode="overwrite")
    assert out == "WRITE_RESULT"
    assert calls["load"] == "notebook_alice.sales"
    assert calls["create"] == ("notebook_alice.sales", "ARROW_SCHEMA")
    assert df.__class__.last == {"tbl": "NEW_TBL", "mode": "overwrite"}


def test_write_iceberg_uses_existing_table(monkeypatch):
    _install_fake_pyiceberg_exc(monkeypatch)
    class FakeCat:
        def load_table(self, ident):
            return "EXISTING_TBL"
        def create_table(self, *a):
            raise AssertionError("should not create when table exists")
    monkeypatch.setattr(E, "catalog", lambda *a, **k: FakeCat())
    df = _fake_df()
    out = E.write_iceberg(df, "notebook_bob.orders")
    assert out == "WRITE_RESULT"
    assert df.__class__.last == {"tbl": "EXISTING_TBL", "mode": "append"}


def test_write_table_trino_qualifies_name_and_creates_schema(monkeypatch):
    # write_table goes THROUGH the engine (df.write_sql). SQLDataSink hands the
    # bare table name to pandas.to_sql, so the catalog+schema are the connection's
    # defaults, and the schema is CREATE-SCHEMA-ensured first.
    ddl_sql = []
    factories = []

    class _DDLConn:
        def execute(self, stmt):
            ddl_sql.append(str(stmt))

        def commit(self):
            pass

        def close(self):
            pass

    def _fake_trino_conn(engine="trino", *, env=None, cache=None, catalog=None, schema=None):
        factories.append((catalog, schema))
        return lambda: _DDLConn()

    monkeypatch.setattr(E, "_trino_conn", _fake_trino_conn)
    captured = {}

    class _DF:
        def write_sql(self, name, conn, write_mode="append", column_types=None):
            captured.update(name=name, conn=conn, mode=write_mode)
            return "METRICS"

    out = E.write_table(_DF(), "lakehouse.analytics.summary",
                        engine="trino", mode="overwrite")
    assert out == "METRICS"
    assert captured["name"] == "summary"          # BARE table name to write_sql
    assert captured["mode"] == "overwrite"
    assert callable(captured["conn"])             # a connection factory
    # a catalog-only DDL factory + a catalog+schema write factory were built
    assert ("lakehouse", None) in factories
    assert ("lakehouse", "analytics") in factories
    assert any("CREATE SCHEMA" in s and "lakehouse.analytics" in s for s in ddl_sql)


def test_write_table_dispatches_to_starrocks_conn(monkeypatch):
    seen = {}

    def _fake_sr_conn(engine="starrocks", *, env=None, cache=None, catalog=None, schema=None):
        seen["dispatch"] = (engine, catalog, schema)
        return lambda: "SR_CONN"

    monkeypatch.setattr(E, "_starrocks_conn", _fake_sr_conn)

    class _DF:
        def write_sql(self, name, conn, write_mode="append", column_types=None):
            seen.update(name=name, conn=conn())
            return "M"

    E.write_table(_DF(), "lakehouse.s.t", engine="starrocks")
    assert seen["dispatch"] == ("starrocks", "lakehouse", "s")
    assert seen["name"] == "t"                    # bare table name
    assert seen["conn"] == "SR_CONN"


def test_write_table_requires_fully_qualified_name():
    for bad in ("justatable", "schema.table"):
        with pytest.raises(ValueError):
            E.write_table(object(), bad)
