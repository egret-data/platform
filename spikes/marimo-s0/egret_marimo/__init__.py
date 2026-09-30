"""Kernel-side helper: SQL through egret's gateway with the session token (spec §4.4)."""

import os

import httpx
import polars as pl


def _base() -> str:
    return os.environ.get("EGRET_URL", "http://127.0.0.1:8700")


def _token() -> str:
    override = os.environ.get("EGRET_TEST_TOKEN")
    if override:
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
    from timing import mark

    mark("gateway_request", sql=sql)
    r = httpx.post(
        f"{_base()}/api/apps/query",
        json={"database": database, "sql": sql, "params": list(params), "pid": os.getpid()},
        headers={"authorization": f"Bearer {_token()}"},
        timeout=60,
    )
    mark("gateway_response", sql=sql, status=r.status_code)
    if r.status_code == 401:
        raise RuntimeError("session expired — reload the app")
    r.raise_for_status()
    return r.json()


class Cursor:
    def __init__(self, database: str) -> None:
        self._database = database
        self.description = None
        self._rows: list[tuple] = []

    def execute(self, sql: str, params=()) -> "Cursor":
        data = _query(self._database, sql, params)
        self.description = [(c["name"], None, None, None, None, None, None) for c in data["columns"]]
        self._rows = [tuple(row) for row in data["rows"]]
        return self

    def fetchall(self) -> list[tuple]:
        return self._rows

    def fetchone(self):
        return self._rows[0] if self._rows else None

    def close(self) -> None:
        pass


class Connection:
    dialect = "trino"

    def __init__(self, database: str) -> None:
        self.database = database

    def cursor(self) -> Cursor:
        return Cursor(self.database)

    def commit(self) -> None:
        pass

    def rollback(self) -> None:
        pass

    def close(self) -> None:
        pass


def connect(database: str) -> Connection:
    return Connection(database)


def sql(query: str, database: str) -> pl.DataFrame:
    data = _query(database, query)
    names = [c["name"] for c in data["columns"]]
    return pl.DataFrame(data["rows"], schema=names, orient="row")
