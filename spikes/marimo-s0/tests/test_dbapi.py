import os
import threading
import time

import httpx
import pytest
import uvicorn

import fake_egret


@pytest.fixture(scope="module")
def live_egret():
    server = uvicorn.Server(uvicorn.Config(fake_egret.app, host="127.0.0.1", port=8790, log_level="warning"))
    t = threading.Thread(target=server.run, daemon=True)
    t.start()
    while not server.started:
        time.sleep(0.05)
    yield "http://127.0.0.1:8790"
    server.should_exit = True


def _mint_token(base: str) -> str:
    c = httpx.Client(base_url=base)
    c.get("/login?user=alice")
    page = c.get("/apps/probe/run").text
    ticket = page.split("_ticket?t=")[1].split('"')[0]
    return c.post("/internal/tickets/redeem", json={"ticket": ticket}).json()["session_token"]


def test_dbapi_roundtrip_carries_the_viewer(live_egret, monkeypatch):
    monkeypatch.setenv("EGRET_URL", live_egret)
    monkeypatch.setenv("EGRET_TEST_TOKEN", _mint_token(live_egret))
    import egret_marimo

    conn = egret_marimo.connect("trino_prod")
    cur = conn.cursor()
    cur.execute("SELECT 42")
    assert [d[0] for d in cur.description] == ["viewer", "sql"]
    assert cur.fetchall() == [("alice", "SELECT 42")]
    df = egret_marimo.sql("SELECT 7", database="trino_prod")
    assert df["viewer"].to_list() == ["alice"]


def test_dbapi_is_detected_by_marimo():
    from marimo._sql.engines.dbapi import DBAPIEngine

    import egret_marimo

    assert DBAPIEngine.is_compatible(egret_marimo.connect("trino_prod"))
