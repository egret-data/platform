"""Kernel-side SQL through egret's gateway, authenticated as the viewer."""

import os

import httpx
import polars as pl

GATEWAY_TIMEOUT_S = 70  # above the gateway's own 60 s limit


class GatewayError(RuntimeError):
    def __init__(self, status: int, detail: str) -> None:
        super().__init__(f"egret gateway error {status}: {detail}")
        self.status = status
        self.detail = detail


class SessionExpired(GatewayError):
    def __init__(self) -> None:
        super().__init__(401, "session expired — reload the app")


def error_for(response: httpx.Response) -> GatewayError:
    """Map a non-2xx response to the error every egret client raises."""
    if response.status_code == 401:
        return SessionExpired()
    try:
        detail = response.json()["error"]
    except (ValueError, KeyError, TypeError):
        detail = response.text
    return GatewayError(response.status_code, str(detail))


def _base() -> str:
    return os.environ["EGRET_URL"].rstrip("/")


def _session_token() -> str:
    """The x-egret-app-token of the request that triggered this cell.

    EGRET_TEST_TOKEN is honoured only under pytest: a production pod must never
    accept a token from its own environment.
    """
    override = os.environ.get("EGRET_TEST_TOKEN")
    if override and os.environ.get("PYTEST_CURRENT_TEST"):
        return override
    import marimo as mo

    request = mo.app_meta().request
    if request is None:
        raise RuntimeError("egret_marimo: no request context — cannot identify the viewer")
    headers = {k.lower(): v for k, v in dict(request.headers).items()}
    token = headers.get("x-egret-app-token")
    if not token:
        raise RuntimeError("egret_marimo: no x-egret-app-token on the request")
    return token


def _query(database: str, sql: str, params=()) -> dict:
    body = {"database": database, "sql": sql, "params": list(params)}
    if os.environ.get("EGRET_TEST_INCLUDE_PID"):
        body["pid"] = os.getpid()
    r = httpx.post(
        f"{_base()}/api/apps/query",
        json=body,
        headers={"authorization": f"Bearer {_session_token()}"},
        timeout=GATEWAY_TIMEOUT_S,
    )
    if not r.is_success:
        raise error_for(r)
    return r.json()


class Cursor:
    def __init__(self, database: str) -> None:
        self._database = database
        self.description: list[tuple] | None = None
        self.rowcount = -1
        self._rows: list[tuple] = []

    def execute(self, sql: str, params=()) -> "Cursor":
        data = _query(self._database, sql, params)
        self.description = [(c["name"], None, None, None, None, None, None) for c in data["columns"]]
        self._rows = [tuple(row) for row in data["rows"]]
        self.rowcount = len(self._rows)
        return self

    def fetchall(self) -> list[tuple]:
        return self._rows

    def fetchone(self) -> tuple | None:
        return self._rows[0] if self._rows else None

    def close(self) -> None:
        pass


class Connection:
    """DB-API 2.0 surface marimo's DBAPIEngine requires."""

    def __init__(self, database: str, dialect: str = "sql") -> None:
        self.database = database
        self.dialect = dialect

    def cursor(self) -> Cursor:
        return Cursor(self.database)

    def commit(self) -> None:
        pass

    def rollback(self) -> None:
        pass

    def close(self) -> None:
        pass


def connect(database: str, dialect: str = "sql") -> Connection:
    return Connection(database, dialect)


def sql(query: str, database: str, params=()) -> pl.DataFrame:
    data = _query(database, query, params)
    names = [c["name"] for c in data["columns"]]
    return pl.DataFrame(data["rows"], schema=names, orient="row")
