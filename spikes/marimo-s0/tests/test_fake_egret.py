import pytest
from starlette.testclient import TestClient

import fake_egret

POD = {"authorization": "Bearer pod-kernel-token"}


@pytest.fixture
def client():
    fake_egret.STATE.reset()
    return TestClient(fake_egret.app)


def test_draft_put_requires_matching_revision(client):
    d = client.get("/api/apps/probe/draft", headers=POD).json()
    assert d["revision"] == 1
    ok = client.put("/api/apps/probe/draft", content=b"x = 1", headers={**POD, "if-match": "1"})
    assert ok.status_code == 200 and ok.json()["revision"] == 2
    stale = client.put("/api/apps/probe/draft", content=b"x = 2", headers={**POD, "if-match": "1"})
    assert stale.status_code == 409 and stale.json()["current_revision"] == 2
    assert client.get("/api/apps/probe/draft", headers=POD).json()["source"] == "x = 1"


def test_draft_requires_pod_token(client):
    assert client.get("/api/apps/probe/draft").status_code == 401


def test_ticket_is_single_use_and_mints_a_query_token(client):
    client.get("/login?user=alice")
    page = client.get("/apps/probe/run").text
    ticket = page.split("_ticket?t=")[1].split('"')[0]
    first = client.post("/internal/tickets/redeem", json={"ticket": ticket})
    assert first.status_code == 200 and first.json()["user"] == "alice"
    assert client.post("/internal/tickets/redeem", json={"ticket": ticket}).status_code == 403
    tok = first.json()["session_token"]
    q = client.post(
        "/api/apps/query",
        json={"database": "trino_prod", "sql": "SELECT 1", "params": [], "pid": 1},
        headers={"authorization": f"Bearer {tok}"},
    )
    assert q.json()["rows"] == [["alice", "SELECT 1"]]
    client.post("/internal/tokens/revoke", json={"session_token": tok})
    again = client.post(
        "/api/apps/query",
        json={"database": "trino_prod", "sql": "SELECT 1", "params": [], "pid": 1},
        headers={"authorization": f"Bearer {tok}"},
    )
    assert again.status_code == 401


def test_mutate_is_double_submit(client):
    client.get("/login?user=alice")
    csrf = client.cookies.get("egret_csrf")
    assert client.post("/api/mutate").status_code == 403
    assert client.post("/api/mutate", headers={"x-egret-csrf": "wrong"}).status_code == 403
    assert client.post("/api/mutate", headers={"x-egret-csrf": csrf}).status_code == 200
