"""Dataframe-first engine helpers: trino(), starrocks() — direct Daft access as the logged-in user."""
import base64
import datetime as _dt
import json
import os
import struct
import tempfile

_REFRESH_MARGIN = _dt.timedelta(seconds=30)


def _write_temp_pem(pem):
    """Write a PEM string to a temp file and return its path (for pinning a CA)."""
    cf = tempfile.NamedTemporaryFile(mode="w", suffix=".pem", delete=False)
    cf.write(pem)
    cf.close()
    return cf.name


def _tls_verify(tls_cert, insecure):
    """Resolve the requests/Trino TLS `verify` arg for an engine connection.

    - `insecure` (explicit EGRET_ENGINE_TLS_INSECURE opt-out) → ``False``: accept
      ANY cert. MITM-unsafe; off by default, for break-glass debugging only.
    - a pinned cert PEM (`tls_cert`, delivered by egret for the engine's
      self-signed coordinator cert) → a temp CA file: the client still validates
      TLS but trusts ONLY this cert (not "accept anything"). Mirrors the backend's
      `reqwest::Certificate` pinning. Secure default.
    - neither → ``True``: verify against the system CA bundle (real cert case).
    """
    if insecure:
        return False
    if tls_cert:
        return _write_temp_pem(tls_cert)
    return True


def _starrocks_ssl_kwargs(tls_cert, insecure):
    """pymysql SSL kwargs for a StarRocks (MySQL-protocol) connection.

    Mirrors the backend's `mysql_async::SslOpts` (connector/starrocks.rs): PIN the
    delivered self-signed FE cert as the trusted CA, but SKIP hostname/SAN matching
    (the FE cert's SAN typically doesn't match the connection host). `insecure`
    opt-out → TLS without verifying. No cert → no SSL kwargs (plain connection)."""
    if insecure:
        return {"ssl": {}, "ssl_verify_cert": False, "ssl_verify_identity": False}
    if tls_cert:
        return {"ssl": {"ca": _write_temp_pem(tls_cert)}, "ssl_verify_identity": False}
    return {}


def _parse_expiry(s):
    # RFC3339; tolerate trailing "Z".
    return _dt.datetime.fromisoformat(s.replace("Z", "+00:00"))


def _now():
    return _dt.datetime.now(_dt.timezone.utc)


def _post(url, body, token):
    import urllib.error
    import urllib.request
    req = urllib.request.Request(
        url, data=body, method="POST",
        headers={"Authorization": "Bearer " + token, "Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            return json.loads(r.read().decode())
    except urllib.error.HTTPError as e:
        if e.code == 401:
            raise RuntimeError("egret: kernel token expired — restart the kernel") from None
        if e.code in (403, 404):
            raise RuntimeError("egret: engine not enabled for notebooks or not permitted") from None
        detail = e.read().decode()[:200]
        raise RuntimeError(f"egret: engine-token failed (HTTP {e.code}): {detail}") from None
    except Exception as e:  # noqa: BLE001 — surface a clear message, never silent-empty
        raise RuntimeError(f"egret: cannot reach the engine-token endpoint ({e})") from None


# Module-level cache: {engine: {jwt, expires_at, kind, host, port}}.
_CACHE: dict = {}


def _engine_token(engine, *, env=None, http=None, cache=None):
    """Fetch a fresh per-user JWT + engine connection coordinates from egret,
    caching until near expiry (the kernel is long-lived, the JWT is not)."""
    env = os.environ if env is None else env
    cache = _CACHE if cache is None else cache
    cached = cache.get(engine)
    if cached is not None and _parse_expiry(cached["expires_at"]) - _REFRESH_MARGIN > _now():
        return cached
    api = env.get("EGRET_API_URL")
    token = env.get("EGRET_KERNEL_TOKEN")
    if not api or not token:
        raise RuntimeError(
            "egret: kernel not configured (missing EGRET_API_URL / EGRET_KERNEL_TOKEN)"
        )
    body = json.dumps({"engine": engine}).encode()
    resp = (http or _post)(api.rstrip("/") + "/api/kernel/engine-token", body, token)
    cache[engine] = resp
    return resp


def _trino_conn(engine="trino", *, env=None, cache=None, catalog=None, schema=None):
    """Build a per-connection SQLAlchemy factory for the workspace Trino,
    authenticated with a fresh per-user JWT over TLS. Shared by ``trino()`` (read)
    and ``write_table()`` (write) so both go through the same governed engine.

    When ``catalog``/``schema`` are given they become the connection's default
    catalog/schema (``trino://…/catalog/schema``) — required by ``write_table``
    because Daft's SQL writer passes the bare table name to ``pandas.to_sql``."""

    def _conn():
        # Fetch the token per-connection: Daft may open connections later and
        # across partitions, so each picks up a cache-refreshed JWT rather than
        # one frozen at call time.
        tok = _engine_token(engine, env=env, cache=cache)
        from sqlalchemy import create_engine
        from trino.auth import JWTAuthentication
        # TLS verification is ALWAYS on. The coordinator serves a self-signed cert,
        # so egret delivers that cert (tok["tls_cert"]) and we PIN it: verify the
        # server against THIS cert only — the client still validates TLS, it just
        # trusts this one cert (not "accept any cert"). Mirrors the backend's
        # reqwest::Certificate pinning. EGRET_ENGINE_TLS_INSECURE=1 is an explicit,
        # off-by-default opt-out escape hatch that disables verification entirely.
        _env = env or os.environ
        insecure = _env.get("EGRET_ENGINE_TLS_INSECURE", "").lower() in ("1", "true", "yes")
        verify = _tls_verify(tok.get("tls_cert"), insecure)
        # The username is the real user from the JWT (preferred_username) —
        # Trino sends it as X-Trino-User for the governed path. http_scheme="https"
        # is required: egret enforces Trino JWT auth over TLS.
        url = f"trino://{_jwt_username(tok['jwt'])}@{tok['host']}:{tok['port']}"
        if catalog and schema:
            url += f"/{catalog}/{schema}"
        elif catalog:
            url += f"/{catalog}"
        eng = create_engine(
            url,
            connect_args={
                "auth": JWTAuthentication(tok["jwt"]),
                "http_scheme": "https",
                "verify": verify,
            },
        )
        return eng.connect()

    return _conn


def trino(sql, *, engine="trino", env=None, cache=None):
    """Run SQL on the workspace Trino as you, returning a lazy Daft DataFrame
    (pushdown). Authenticated with a fresh per-user JWT."""
    import daft

    return daft.read_sql(sql, _trino_conn(engine, env=env, cache=cache))


def _jwt_username(jwt):
    """Extract the `preferred_username` claim from a JWT (the engine login user
    must equal the JWT principal). Defensive: any failure falls back to "egret"."""
    try:
        p = jwt.split(".")[1]
        p += "=" * (-len(p) % 4)
        return json.loads(base64.urlsafe_b64decode(p)).get("preferred_username") or "egret"
    except Exception:  # noqa: BLE001 — malformed/absent claim → safe fallback
        return "egret"


_OPENID_PLUGIN = "authentication_openid_connect_client"


def _lenenc(n):
    if n < 0xFB:
        return bytes([n])
    if n < 0x1_0000:
        return b"\xfc" + struct.pack("<H", n)
    if n < 0x100_0000:
        return b"\xfd" + struct.pack("<I", n)[:3]
    return b"\xfe" + struct.pack("<Q", n)


def _openid_handler(jwt):
    """PyMySQL custom auth-plugin for StarRocks' authentication_openid_connect_client:
    replies to the auth-switch with [0x01][lenenc(JWT)][JWT] (no NUL). See spike §4."""
    class _Handler:
        def __init__(self, con):
            self._con = con

        def authenticate(self, pkt):
            t = jwt.encode()
            self._con.write_packet(b"\x01" + _lenenc(len(t)) + t)
            reply = self._con._read_packet()
            reply.check_error()
            return reply

    return _Handler


def _starrocks_conn(engine="starrocks", *, env=None, cache=None, catalog=None, schema=None):
    """Build a per-connection SQLAlchemy factory for the workspace StarRocks,
    authenticated with a fresh per-user JWT over the MySQL protocol via a custom
    OpenID-Connect auth-plugin handler. Shared by ``starrocks()`` and
    ``write_table()``. ``schema`` (when given) is the default database; StarRocks
    external-catalog selection (``catalog``) is left to the caller's fully-qualified
    names / ``SET CATALOG`` (see note in ``write_table``)."""

    def _conn():
        import pymysql
        from sqlalchemy import create_engine
        tok = _engine_token(engine, env=env, cache=cache)

        _env = env or os.environ
        insecure = _env.get("EGRET_ENGINE_TLS_INSECURE", "").lower() in ("1", "true", "yes")

        def _creator():
            # StarRocks rejects a mismatched login: the connection user MUST equal
            # the JWT principal (preferred_username); the JWT then carries auth.
            kwargs = dict(
                host=tok["host"], port=tok["port"],
                user=_jwt_username(tok["jwt"]), password="",
                auth_plugin_map={_OPENID_PLUGIN: _openid_handler(tok["jwt"])},
            )
            # TLS to the FE MySQL port: pin egret's delivered self-signed cert.
            kwargs.update(_starrocks_ssl_kwargs(tok.get("tls_cert"), insecure))
            if schema:
                kwargs["database"] = schema
            return pymysql.connect(**kwargs)

        eng = create_engine("mysql+pymysql://", creator=_creator)
        return eng.connect()

    return _conn


def starrocks(sql, *, engine="starrocks", env=None, cache=None):
    """Run SQL on the workspace StarRocks as you, returning a lazy Daft DataFrame
    (pushdown). Authenticated with a fresh per-user JWT over the MySQL protocol via
    a custom OpenID-Connect auth-plugin handler."""
    import daft

    return daft.read_sql(sql, _starrocks_conn(engine, env=env, cache=cache))


def _split_qualified(table):
    """Split a fully-qualified ``catalog.schema.table`` into its three parts.

    write_table requires the 3-part form because the engine INSERT needs an
    explicit catalog + schema (Daft's SQL writer carries no catalog context)."""
    parts = table.split(".")
    if len(parts) != 3:
        raise ValueError(
            "egret: write_table expects a fully-qualified 'catalog.schema.table' "
            f"name, got {table!r} "
            "(e.g. write_table(df, 'lakehouse.analytics.summary'))."
        )
    return parts[0], parts[1], parts[2]


def write_table(df, table, *, engine="trino", mode="overwrite",
                column_types=None, env=None, cache=None):
    """Write a Daft DataFrame back to the shared catalog THROUGH the query engine,
    so the write is governed by the SAME policies as your reads — one authorization
    plane for read and write.

    The notebook never touches the Iceberg REST catalog directly: the engine
    (Trino/StarRocks) performs the INSERT into the governed catalog, and policy is
    enforced on the write exactly as on a read.

    Args:
        df:      a Daft DataFrame to persist.
        table:   the fully-qualified target ``"catalog.schema.table"``,
                 e.g. ``"lakehouse.analytics.summary"``.
        engine:  ``"trino"`` (default, spike-validated) or ``"starrocks"``.
        mode:    ``"append"`` | ``"overwrite"`` | ``"fail"`` (default ``"overwrite"``).
        column_types: optional {column: SQLAlchemy type} overrides for table creation.

    Returns Daft's single-row write-metrics DataFrame (rows/bytes written).

    Notes:
        * Daft's SQL writer passes the bare table name to ``pandas.to_sql``, so the
          catalog + schema are set as the connection's defaults (``…/catalog/schema``).
        * ``write_sql`` creates the table but NOT the schema; for Trino we first
          ``CREATE SCHEMA IF NOT EXISTS``. StarRocks' external-catalog write is not
          yet spike-validated — prefer ``engine="trino"``.
        * Each partition is materialized via pandas → ``to_sql``; intended for
          EDA-scale outputs (summaries, feature tables), not bulk loads.
    """
    catalog, schema, name = _split_qualified(table)
    conn_for = _trino_conn if engine == "trino" else _starrocks_conn

    if engine == "trino":
        # write_sql creates the table but not the namespace — ensure it exists.
        from sqlalchemy import text
        ddl = conn_for(engine, env=env, cache=cache, catalog=catalog)()
        try:
            ddl.execute(text(f"CREATE SCHEMA IF NOT EXISTS {catalog}.{schema}"))
            try:
                ddl.commit()
            except Exception:
                pass
        finally:
            ddl.close()

    # The connection carries the default catalog+schema; write_sql receives the
    # BARE table name (SQLDataSink hands it straight to pandas.to_sql).
    conn = conn_for(engine, env=env, cache=cache, catalog=catalog, schema=schema)
    return df.write_sql(name, conn, write_mode=mode, column_types=column_types)
