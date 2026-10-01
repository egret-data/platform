# Data Apps S0 — marimo spike Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Answer the six S0 questions of `docs/en/specs/2026-09-30-marimo-data-apps-design.md` §9 with measurements, and record them in a result document that either confirms the design or triggers its documented fallbacks.

**Architecture:** A throwaway, Python-only model of the design running on one machine. A *fake egret* (Starlette, `egret.localhost:8700`) plays login, tickets, the draft API and the query gateway. A *proxy* (Starlette, `apps.egret.localhost:8701`) plays egret's apps-origin proxy. An *entrypoint* launches real marimo 0.25.0 in edit or run mode with egret's patches. A helper package `egret_marimo` plays the kernel side. Browser probes (Playwright, Chromium) drive it and write JSON results.

**Tech Stack:** Python 3.12 via uv, `marimo==0.25.0`, starlette, uvicorn, httpx, websockets, polars, anywidget, pytest, playwright (Chromium).

## Global Constraints

- **The spike's code is throwaway.** It lives on branch `spike/marimo-s0`, directory `spikes/marimo-s0/`, and is **never merged**. Only the result document is merged, on the spec branch.
- **Pin `marimo==0.25.0`** from PyPI. Line numbers in the spec come from a checkout 37 commits past the 0.25.0 tag (`data-portal/marimo`). When a step says "read X", read the installed copy under `.venv/lib/python3.12/site-packages/marimo/`, not the checkout.
- **Hostnames, not ports, separate the origins.** Use `egret.localhost` and `apps.egret.localhost`: Chromium resolves `*.localhost` to loopback, and cookies are **not** isolated by port. Server-to-server calls use `127.0.0.1`.
- Ports: fake egret 8700, proxy 8701, marimo edit 2718, marimo run 2719.
- Fixed secrets (spike only): the pod's marimo token is `pod-secret`, and the pod's token for egret's draft API is `pod-kernel-token`.
- Wording: a browser probe is *automated Playwright*. Anything checked by hand is written as *manual* in the result document. Never blur the two.
- Every question's answer in the result document carries a number or an observed artefact (log line, screenshot path, pid list). "It worked" alone is not an answer.

---

## File structure

All paths are relative to `spikes/marimo-s0/`.

| File | Responsibility |
|---|---|
| `pyproject.toml` | uv project, pinned dependencies |
| `README.md` | how to start the four processes and run each probe |
| `fake_egret.py` | login, host page with iframe, tickets, draft API, query gateway, CSRF-protected mutate, event log |
| `proxy.py` | ticket redemption to a cookie; HTTP and WebSocket relay; header filtering; route blocking |
| `headers.py` | the pure header-filtering function, shared by proxy and tests |
| `entrypoint.py` | starts marimo edit/run with `EgretStorage`, the patched load, and a process-mode switch |
| `egret_marimo/__init__.py` | DB-API connection and `sql()` calling the gateway with the session token |
| `apps/probe.py` | marimo notebook: slider → Python query + SQL-cell query, shows pid and viewer |
| `apps/xss.py` | marimo notebook: anywidget, `mo.Html` script and `onerror` attempts at egret |
| `config/marimo/marimo.toml` | `[ai] enabled = false`, loaded via `XDG_CONFIG_HOME` |
| `tests/test_headers.py` | unit tests for header filtering |
| `tests/test_fake_egret.py` | draft revision/409, ticket single-use, gateway token checks |
| `tests/test_dbapi.py` | `egret_marimo` against a live fake egret |
| `probes/common.py` | Playwright helpers: login, open app, read iframe text, fetch events |
| `probes/q1_token.py` … `probes/q6_routes.py` | one probe per question, each writing `results/qN.json` |
| `results/` | probe output (committed on the spike branch) |

The result document is `docs/en/plans/<run-date>-marimo-s0-result.md` on branch `docs/marimo-data-apps-spec`.

---

### Task 1: Scaffold, header filter, and fake egret

**Files:**
- Create: `spikes/marimo-s0/pyproject.toml`, `spikes/marimo-s0/headers.py`, `spikes/marimo-s0/fake_egret.py`
- Create: `spikes/marimo-s0/apps/probe.py` (seed content, used as the initial draft)
- Test: `spikes/marimo-s0/tests/test_headers.py`, `spikes/marimo-s0/tests/test_fake_egret.py`

**Interfaces:**
- Produces: `headers.forward_headers(incoming: Mapping[str, str], session_token: str, pod_token: str) -> dict[str, str]`
- Produces (HTTP, fake egret on 8700):
  - `GET /login?user=<name>` sets `egret_session` (HttpOnly) and `egret_csrf` (readable), both SameSite=Lax. When `COOKIE_PREFIX=__Host-` is set in the environment, both names get that prefix and the `Secure` flag.
  - `GET /apps/{app_id}/{mode}` returns the host page with the iframe `http://apps.egret.localhost:8701/_ticket?t=<ticket>`.
  - `POST /internal/tickets/redeem {"ticket"}` returns `{user, app_id, mode, session_token}` (single use, 30 s TTL), or 403.
  - `POST /internal/tokens/revoke {"session_token"}` returns 204.
  - `GET|PUT /api/apps/{app_id}/draft` with `Authorization: Bearer pod-kernel-token`. PUT requires `If-Match: <revision>` and returns 409 `{current_revision}` on a mismatch.
  - `POST /api/apps/query` with `Authorization: Bearer <session_token>`, body `{database, sql, params, pid}`. Returns `{columns:[{name}], rows, truncated}`, where the rows are `[[viewer, sql]]`.
  - `GET /api/whoami` returns `{user}` from the egret cookie.
  - `POST /api/mutate` applies double-submit CSRF exactly like `crates/egret-auth/src/csrf.rs`.
  - `GET /_events` and `POST /_events/reset` read and clear the event log.

- [ ] **Step 1: Create the branch and worktree off `origin/main`**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
git fetch origin
git worktree add -b spike/marimo-s0 ../egret-wt-marimo-s0 origin/main
cd ../egret-wt-marimo-s0 && mkdir -p spikes/marimo-s0/{apps,tests,probes,results,egret_marimo,config/marimo}
```

- [ ] **Step 2: Write `pyproject.toml` and install**

```toml
[project]
name = "marimo-s0"
version = "0.0.0"
requires-python = ">=3.12,<3.13"
dependencies = [
  "marimo==0.25.0",
  "starlette==0.46.2",
  "uvicorn==0.34.2",
  "httpx==0.28.1",
  "websockets==15.0.1",
  "polars==1.30.0",
  "anywidget==0.9.18",
  "pytest==8.4.0",
  "pytest-asyncio==1.0.0",
  "playwright==1.52.0",
]

[tool.pytest.ini_options]
asyncio_mode = "auto"
```

```bash
cd spikes/marimo-s0 && uv venv --python 3.12 && uv sync && uv run playwright install chromium
uv run python -c "import marimo; print(marimo.__version__)"
```

Expected: `0.25.0`. If any pinned version fails to resolve, bump that one package to the nearest resolvable version and note it in the README. Never change the marimo pin.

- [ ] **Step 3: Write the failing header test**

`tests/test_headers.py`:

```python
from headers import forward_headers


def test_strips_client_egret_headers_cookies_auth_and_hop_by_hop():
    incoming = {
        "X-Egret-App-Token": "forged",
        "x-egret-anything": "forged",
        "Cookie": "apps_sid=abc",
        "Authorization": "Bearer forged",
        "Connection": "keep-alive",
        "Host": "apps.egret.localhost:8701",
        "Marimo-Server-Token": "skew-123",
        "Accept": "text/html",
    }
    out = forward_headers(incoming, session_token="real", pod_token="pod-secret")
    assert out["x-egret-app-token"] == "real"
    assert out["authorization"] == "Bearer pod-secret"
    assert out["marimo-server-token"] == "skew-123"  # skew protection must pass through
    assert out["accept"] == "text/html"
    for gone in ("cookie", "connection", "host", "x-egret-anything"):
        assert gone not in out
    assert "forged" not in out.values()
```

- [ ] **Step 4: Run it to see it fail**

Run: `uv run pytest tests/test_headers.py -v`
Expected: FAIL with `ModuleNotFoundError: No module named 'headers'`.

- [ ] **Step 5: Implement `headers.py`**

```python
"""Header filtering for the apps-origin proxy (spec §6.2)."""

from collections.abc import Mapping

HOP_BY_HOP = {
    "connection", "keep-alive", "proxy-authenticate", "proxy-authorization", "te",
    "trailers", "transfer-encoding", "upgrade", "host", "content-length",
}
# WebSocket handshake headers are regenerated by the upstream client library.
WS_HANDSHAKE = {
    "sec-websocket-key", "sec-websocket-version", "sec-websocket-extensions",
    "sec-websocket-accept", "sec-websocket-protocol",
}


def forward_headers(
    incoming: Mapping[str, str], session_token: str, pod_token: str
) -> dict[str, str]:
    out: dict[str, str] = {}
    for key, value in incoming.items():
        k = key.lower()
        if k in HOP_BY_HOP or k in WS_HANDSHAKE or k in ("cookie", "authorization"):
            continue
        if k.startswith("x-egret-"):
            continue
        out[k] = value
    out["authorization"] = f"Bearer {pod_token}"
    out["x-egret-app-token"] = session_token
    return out
```

- [ ] **Step 6: Run the header test until it passes**

Run: `uv run pytest tests/test_headers.py -v`
Expected: PASS.

- [ ] **Step 7: Write the seed notebook `apps/probe.py`**

```python
import marimo

__generated_with = "0.25.0"
app = marimo.App()


@app.cell
def _():
    import os

    import marimo as mo

    import egret_marimo as egret

    return egret, mo, os


@app.cell
def _(egret):
    db = egret.connect("trino_prod")
    return (db,)


@app.cell
def _(mo):
    n = mo.ui.slider(1, 10, value=1, label="n")
    n
    return (n,)


@app.cell
def _(egret, mo, n, os):
    py_df = egret.sql(f"SELECT {n.value} AS n", database="trino_prod")
    mo.md(
        f"PY pid={os.getpid()} viewer={py_df['viewer'][0]} sql={py_df['sql'][0]}"
    )
    return


@app.cell
def _(db, mo, n):
    sql_df = mo.sql(f"SELECT {n.value} AS via_sql_cell", engine=db)
    mo.md(f"SQLCELL viewer={sql_df['viewer'][0]}")
    return


if __name__ == "__main__":
    app.run()
```

- [ ] **Step 8: Write the failing fake-egret tests**

`tests/test_fake_egret.py`:

```python
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
```

- [ ] **Step 9: Run them to see them fail**

Run: `uv run pytest tests/test_fake_egret.py -v`
Expected: FAIL with `ModuleNotFoundError: No module named 'fake_egret'`.

- [ ] **Step 10: Implement `fake_egret.py`**

```python
"""A fake egret: just enough of login, tickets, drafts and the query gateway (spec §4–§6)."""

import os
import pathlib
import secrets
import time

from starlette.applications import Starlette
from starlette.requests import Request
from starlette.responses import HTMLResponse, JSONResponse, PlainTextResponse, Response
from starlette.routing import Route

HERE = pathlib.Path(__file__).parent
APPS_ORIGIN = "http://apps.egret.localhost:8701"
POD_TOKEN = "pod-kernel-token"
PREFIX = os.environ.get("COOKIE_PREFIX", "")  # "" or "__Host-"
SESSION_COOKIE, CSRF_COOKIE = f"{PREFIX}egret_session", f"{PREFIX}egret_csrf"


class State:
    def reset(self) -> None:
        self.drafts = {
            p.stem: {"source": p.read_text(), "revision": 1}
            for p in (HERE / "apps").glob("*.py")
        }
        self.sessions: dict[str, str] = {}
        self.tickets: dict[str, dict] = {}
        self.app_tokens: dict[str, dict] = {}
        self.events: list[dict] = []

    def log(self, kind: str, **kw) -> None:
        self.events.append({"t": time.time(), "kind": kind, **kw})


STATE = State()
STATE.reset()


def _user(request: Request) -> str | None:
    return STATE.sessions.get(request.cookies.get(SESSION_COOKIE, ""))


def _pod_ok(request: Request) -> bool:
    return request.headers.get("authorization") == f"Bearer {POD_TOKEN}"


async def login(request: Request) -> Response:
    user = request.query_params["user"]
    sid, csrf = secrets.token_urlsafe(16), secrets.token_urlsafe(16)
    STATE.sessions[sid] = user
    secure = PREFIX == "__Host-"
    resp = PlainTextResponse(f"logged in as {user}")
    resp.set_cookie(SESSION_COOKIE, sid, httponly=True, samesite="lax", secure=secure, path="/")
    resp.set_cookie(CSRF_COOKIE, csrf, httponly=False, samesite="lax", secure=secure, path="/")
    return resp


async def host_page(request: Request) -> Response:
    user = _user(request)
    if not user:
        return PlainTextResponse("login first", status_code=401)
    app_id, mode = request.path_params["app_id"], request.path_params["mode"]
    ticket = secrets.token_urlsafe(16)
    STATE.tickets[ticket] = {"user": user, "app_id": app_id, "mode": mode, "exp": time.time() + 30}
    return HTMLResponse(
        f"""<!doctype html><title>egret host</title>
<div id="hdr">egret header | {user} | {app_id} ({mode})</div>
<iframe id="app" style="width:100%;height:90vh;border:0"
  sandbox="allow-scripts allow-same-origin allow-forms allow-downloads"
  src="{APPS_ORIGIN}/_ticket?t={ticket}"></iframe>"""
    )


async def redeem(request: Request) -> Response:
    ticket = (await request.json())["ticket"]
    tk = STATE.tickets.pop(ticket, None)
    if not tk or tk["exp"] < time.time():
        STATE.log("ticket_rejected", ticket=ticket)
        return JSONResponse({"error": "bad ticket"}, status_code=403)
    token = secrets.token_urlsafe(24)
    STATE.app_tokens[token] = {k: tk[k] for k in ("user", "app_id", "mode")}
    STATE.log("ticket_redeemed", **STATE.app_tokens[token])
    return JSONResponse({**STATE.app_tokens[token], "session_token": token})


async def revoke(request: Request) -> Response:
    token = (await request.json())["session_token"]
    info = STATE.app_tokens.pop(token, None)
    STATE.log("token_revoked", user=(info or {}).get("user"))
    return Response(status_code=204)


async def draft(request: Request) -> Response:
    if not _pod_ok(request):
        return Response(status_code=401)
    app_id = request.path_params["app_id"]
    d = STATE.drafts[app_id]
    if request.method == "GET":
        return JSONResponse(d, headers={"etag": str(d["revision"])})
    sent = request.headers.get("if-match")
    if sent != str(d["revision"]):
        STATE.log("draft_conflict", app_id=app_id, sent=sent, current=d["revision"])
        return JSONResponse({"current_revision": d["revision"]}, status_code=409)
    d["source"] = (await request.body()).decode()
    d["revision"] += 1
    STATE.log("draft_saved", app_id=app_id, revision=d["revision"])
    return JSONResponse({"revision": d["revision"]})


async def query(request: Request) -> Response:
    token = request.headers.get("authorization", "").removeprefix("Bearer ")
    info = STATE.app_tokens.get(token)
    if not info:
        STATE.log("query_rejected")
        return JSONResponse({"error": "session expired"}, status_code=401)
    body = await request.json()
    STATE.log("query", user=info["user"], app_id=info["app_id"], sql=body["sql"], pid=body.get("pid"))
    return JSONResponse(
        {"columns": [{"name": "viewer"}, {"name": "sql"}], "rows": [[info["user"], body["sql"]]], "truncated": False}
    )


async def whoami(request: Request) -> Response:
    STATE.log("whoami", user=_user(request), origin=request.headers.get("origin"),
              raw_cookie=request.headers.get("cookie"))
    return JSONResponse({"user": _user(request)})


async def mutate(request: Request) -> Response:
    cookie, header = request.cookies.get(CSRF_COOKIE), request.headers.get("x-egret-csrf")
    ok = bool(cookie) and cookie == header and _user(request) is not None
    STATE.log("mutate", ok=ok, origin=request.headers.get("origin"), raw_cookie=request.headers.get("cookie"))
    return PlainTextResponse("ok" if ok else "csrf check failed", status_code=200 if ok else 403)


async def events(request: Request) -> Response:
    if request.method == "POST":
        STATE.events.clear()
        return Response(status_code=204)
    return JSONResponse(STATE.events)


app = Starlette(routes=[
    Route("/login", login),
    Route("/apps/{app_id}/{mode}", host_page),
    Route("/internal/tickets/redeem", redeem, methods=["POST"]),
    Route("/internal/tokens/revoke", revoke, methods=["POST"]),
    Route("/api/apps/{app_id}/draft", draft, methods=["GET", "PUT"]),
    Route("/api/apps/query", query, methods=["POST"]),
    Route("/api/whoami", whoami),
    Route("/api/mutate", mutate, methods=["POST"]),
    Route("/_events", events, methods=["GET"]),
    Route("/_events/reset", events, methods=["POST"]),
])
```

- [ ] **Step 11: Run all tests until they pass**

Run: `uv run pytest tests -v`
Expected: 5 passed.

- [ ] **Step 12: Commit**

```bash
git add spikes/marimo-s0 && git commit -m "spike(marimo-s0): scaffold, header filter, fake egret"
```

---

### Task 2: `egret_marimo`, entrypoint, and the proxy

**Files:**
- Create: `spikes/marimo-s0/egret_marimo/__init__.py`, `spikes/marimo-s0/entrypoint.py`, `spikes/marimo-s0/proxy.py`, `spikes/marimo-s0/config/marimo/marimo.toml`, `spikes/marimo-s0/README.md`
- Test: `spikes/marimo-s0/tests/test_dbapi.py`

**Interfaces:**
- Consumes: `headers.forward_headers` and the fake egret HTTP surface from Task 1.
- Produces:
  - `egret_marimo.connect(database: str) -> Connection`, a DB-API 2.0 object with `cursor()`, `commit()`, `rollback()`, `close()` and `dialect = "trino"`. `Cursor.execute(sql, params=())` sets `description`, and `fetchall()` returns the rows.
  - `egret_marimo.sql(query: str, database: str) -> polars.DataFrame`
  - `egret_marimo._token() -> str`, which reads `x-egret-app-token` from `mo.app_meta().request.headers`. It can be overridden with `EGRET_TEST_TOKEN` for unit tests only.
  - `entrypoint.py --mode edit|run --process-mode thread|sandbox|ipc`. Edit serves the directory `$APPS_WORKDIR` at base URL `/e` on port 2718; run serves `$APPS_WORKDIR/probe.py` (or `--app`) at `/r` on 2719.
  - Proxy on 8701: `/_ticket?t=`, then HTTP and WebSocket relay under `/e/…` and `/r/…`. `BLOCKED_PREFIXES` is read from the `S0_BLOCKED` env var (comma-separated, matched after the `/e` or `/r` prefix).

- [ ] **Step 1: Write the failing DB-API test**

`tests/test_dbapi.py`:

```python
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
```

- [ ] **Step 2: Run it to see it fail**

Run: `uv run pytest tests/test_dbapi.py -v`
Expected: FAIL with `ModuleNotFoundError: No module named 'egret_marimo'`.

- [ ] **Step 3: Implement `egret_marimo/__init__.py`**

```python
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
    r = httpx.post(
        f"{_base()}/api/apps/query",
        json={"database": database, "sql": sql, "params": list(params), "pid": os.getpid()},
        headers={"authorization": f"Bearer {_token()}"},
        timeout=60,
    )
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
```

- [ ] **Step 4: Run the DB-API tests until they pass**

Run: `uv run pytest tests/test_dbapi.py -v`
Expected: 2 passed. If `is_compatible` rejects the connection, read `site-packages/marimo/_sql/engines/dbapi.py` `is_compatible`, fix `Connection` to satisfy it, and record the reason in `README.md` under "Findings".

- [ ] **Step 5: Write `config/marimo/marimo.toml`**

```toml
[ai]
enabled = false
```

- [ ] **Step 6: Implement `entrypoint.py`**

```python
"""egret's marimo launcher, spike version (spec §5.1, §5.2, §6.3)."""

import argparse
import os
import pathlib
import sys

import httpx

EGRET = os.environ.get("EGRET_URL", "http://127.0.0.1:8700")
POD_TOKEN = "pod-kernel-token"
POD_MARIMO_TOKEN = "pod-secret"
WORKDIR = pathlib.Path(os.environ["APPS_WORKDIR"]).resolve()
HERE = pathlib.Path(__file__).parent.resolve()


def fetch_draft(app_id: str) -> dict:
    r = httpx.get(f"{EGRET}/api/apps/{app_id}/draft", headers={"authorization": f"Bearer {POD_TOKEN}"})
    r.raise_for_status()
    return r.json()


def install_storage_patch() -> None:
    from marimo._server.workspace import _base
    from marimo._session.notebook.storage import FilesystemStorage
    from marimo._utils.http import HTTPException

    class EgretStorage(FilesystemStorage):
        def __init__(self, app_id: str, revision: int) -> None:
            super().__init__()
            self.app_id, self.revision = app_id, revision

        def write(self, path, content: str) -> None:
            r = httpx.put(
                f"{EGRET}/api/apps/{self.app_id}/draft",
                content=content.encode(),
                headers={"authorization": f"Bearer {POD_TOKEN}", "if-match": str(self.revision)},
            )
            if r.status_code == 409:
                raise HTTPException(409, "This draft was saved by someone else — reload.")
            r.raise_for_status()
            self.revision = r.json()["revision"]
            super().write(path, content)

    original_load = _base.NotebookWorkspace.load

    def load(self, key, defaults=None):
        resolved = self.resolve(key)
        app_id = None
        if resolved and pathlib.Path(resolved).resolve().parent == WORKDIR:
            app_id = pathlib.Path(resolved).stem
            draft = fetch_draft(app_id)
            pathlib.Path(resolved).write_text(draft["source"])
        manager = original_load(self, key, defaults)
        if app_id:
            manager.storage = EgretStorage(app_id, draft["revision"])
            print(f"[s0] EgretStorage bound to {app_id} rev {draft['revision']}", flush=True)
        return manager

    _base.NotebookWorkspace.load = load


def install_force_ipc_patch() -> None:
    """Run mode, one kernel subprocess per session, without a uv sandbox (S0 question 2, variant ipc)."""
    from marimo._session import session as session_mod
    from marimo._session.managers import ipc as ipc_mod
    from marimo._session.model import SessionMode

    original_create = session_mod.Session.create.__func__

    async def create(cls, *args, **kwargs):
        # Step 7 verifies that the call site passes `mode` and `sandbox` as keywords.
        if kwargs.get("mode") == SessionMode.RUN:
            kwargs["sandbox"] = True
        return await original_create(cls, *args, **kwargs)

    session_mod.Session.create = classmethod(create)

    original_init = ipc_mod.IPCKernelManagerImpl.__init__

    def init(self, *args, **kwargs):
        original_init(self, *args, **kwargs)
        self._sandbox = False  # use the configured venv = this interpreter, not a uv sandbox

    ipc_mod.IPCKernelManagerImpl.__init__ = init


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--mode", choices=["edit", "run"], required=True)
    p.add_argument("--process-mode", choices=["thread", "sandbox", "ipc"], default="thread")
    p.add_argument("--app", default="probe")
    a = p.parse_args()

    os.environ["XDG_CONFIG_HOME"] = str(HERE / "config")
    os.environ.setdefault("MARIMO_SKIP_UPDATE_CHECK", "1")
    sys.path.insert(0, str(HERE))  # so kernels can import egret_marimo
    os.environ["PYTHONPATH"] = f"{HERE}:{os.environ.get('PYTHONPATH', '')}"
    WORKDIR.mkdir(parents=True, exist_ok=True)

    if a.mode == "edit":
        install_storage_patch()
        for app_id in ("probe", "xss"):
            (WORKDIR / f"{app_id}.py").write_text(fetch_draft(app_id)["source"])
        argv = ["edit", str(WORKDIR), "--port", "2718", "--base-url", "/e"]
    else:
        (WORKDIR / f"{a.app}.py").write_text(fetch_draft(a.app)["source"])
        argv = ["run", str(WORKDIR / f"{a.app}.py"), "--port", "2719", "--base-url", "/r"]
        if a.process_mode == "sandbox":
            argv.append("--sandbox")
        if a.process_mode == "ipc":
            os.environ["S0_VENV"] = sys.prefix
            install_force_ipc_patch()

    argv += ["--host", "127.0.0.1", "--headless", "--token-password", POD_MARIMO_TOKEN]
    print(f"[s0] marimo {' '.join(argv)}", flush=True)

    from marimo._cli.cli import main as marimo_main

    marimo_main.main(args=argv, standalone_mode=True)


if __name__ == "__main__":
    main()
```

- [ ] **Step 7: Verify the two assumptions the patches make**

Run: `grep -n "Session.create(" -A12 .venv/lib/python3.12/site-packages/marimo/_session/managers/*.py .venv/lib/python3.12/site-packages/marimo/_server/session_manager.py | head -40`
Expected: `mode=` and `sandbox=` are passed as keyword arguments. If they are positional, change `create()` in `install_force_ipc_patch` to bind the arguments with `inspect.signature(original_create).bind(cls, *args, **kwargs)`, set `bound.arguments["sandbox"] = True`, and call `original_create(*bound.args, **bound.kwargs)`.

Run: `grep -n "_sandbox\|venv_config.get(\"path\")\|get_configured_venv_python" .venv/lib/python3.12/site-packages/marimo/_session/managers/ipc.py`
Expected: with `self._sandbox` false, the kernel Python comes from `[venv] path`, or falls back to the current interpreter. If it requires `[venv] path`, add to `config/marimo/marimo.toml` a `[venv]` section with `path = "<absolute path of .venv>"` — write the literal absolute path, since TOML does not expand variables — and note that in the README.

- [ ] **Step 8: Implement `proxy.py`**

```python
"""The apps-origin proxy, spike version (spec §6.1, §6.2)."""

import asyncio
import os
import secrets

import httpx
import websockets
from starlette.applications import Starlette
from starlette.background import BackgroundTask
from starlette.requests import Request
from starlette.responses import PlainTextResponse, RedirectResponse, Response, StreamingResponse
from starlette.routing import Route, WebSocketRoute
from starlette.websockets import WebSocket

from headers import HOP_BY_HOP, forward_headers

EGRET = "http://127.0.0.1:8700"
UPSTREAMS = {"e": "127.0.0.1:2718", "r": "127.0.0.1:2719"}
POD_MARIMO_TOKEN = "pod-secret"
BLOCKED = [b for b in os.environ.get("S0_BLOCKED", "").split(",") if b]
SESSIONS: dict[str, dict] = {}
CLIENT = httpx.AsyncClient(timeout=None)


def _blocked(prefix: str, path: str) -> bool:
    rest = path[len(prefix) + 1:]
    return any(rest.startswith(b) for b in BLOCKED)


async def ticket(request: Request) -> Response:
    r = await CLIENT.post(f"{EGRET}/internal/tickets/redeem", json={"ticket": request.query_params.get("t", "")})
    if r.status_code != 200:
        return PlainTextResponse("ticket rejected", status_code=403)
    sess = r.json()
    sid = secrets.token_urlsafe(16)
    SESSIONS[sid] = sess
    target = f"/e/?file={sess['app_id']}.py" if sess["mode"] == "edit" else "/r/"
    resp = RedirectResponse(target, status_code=303)
    resp.set_cookie("apps_sid", sid, httponly=True, samesite="lax", path="/")
    return resp


def _session(cookies, prefix: str) -> dict | None:
    sess = SESSIONS.get(cookies.get("apps_sid", ""))
    if not sess:
        return None
    if prefix == "e" and sess["mode"] != "edit":
        return None
    return sess


async def relay(request: Request) -> Response:
    prefix = request.path_params["prefix"]
    if prefix not in UPSTREAMS:
        return PlainTextResponse("not found", status_code=404)
    sess = _session(request.cookies, prefix)
    if not sess:
        return PlainTextResponse("no session", status_code=401)
    if _blocked(prefix, request.url.path):
        return PlainTextResponse("blocked by egret", status_code=403)
    url = f"http://{UPSTREAMS[prefix]}{request.url.path}"
    if request.url.query:
        url += f"?{request.url.query}"
    upstream = CLIENT.build_request(
        request.method, url,
        headers=forward_headers(request.headers, sess["session_token"], POD_MARIMO_TOKEN),
        content=await request.body(),
    )
    r = await CLIENT.send(upstream, stream=True)
    headers = {k: v for k, v in r.headers.items() if k.lower() not in HOP_BY_HOP}
    return StreamingResponse(r.aiter_raw(), status_code=r.status_code, headers=headers,
                             background=BackgroundTask(r.aclose))


async def ws_relay(ws: WebSocket) -> None:
    prefix = ws.path_params["prefix"]
    sess = _session(ws.cookies, prefix) if prefix in UPSTREAMS else None
    if not sess or _blocked(prefix, ws.url.path):
        await ws.close(code=4403)
        return
    await ws.accept()
    url = f"ws://{UPSTREAMS[prefix]}{ws.url.path}" + (f"?{ws.url.query}" if ws.url.query else "")
    headers = forward_headers(ws.headers, sess["session_token"], POD_MARIMO_TOKEN)
    headers.pop("origin", None)
    async with websockets.connect(url, additional_headers=headers, max_size=None) as up:
        async def client_to_upstream():
            while True:
                msg = await ws.receive()
                if msg["type"] == "websocket.disconnect":
                    return
                await up.send(msg["text"] if msg.get("text") is not None else msg["bytes"])

        async def upstream_to_client():
            async for msg in up:
                await (ws.send_text(msg) if isinstance(msg, str) else ws.send_bytes(msg))

        tasks = [asyncio.create_task(client_to_upstream()), asyncio.create_task(upstream_to_client())]
        await asyncio.wait(tasks, return_when=asyncio.FIRST_COMPLETED)
        for t in tasks:
            t.cancel()
    if "/ws" in ws.url.path:
        await CLIENT.post(f"{EGRET}/internal/tokens/revoke", json={"session_token": sess["session_token"]})


app = Starlette(routes=[
    Route("/_ticket", ticket),
    WebSocketRoute("/{prefix}/{path:path}", ws_relay),
    Route("/{prefix}/{path:path}", relay, methods=["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"]),
])
```

- [ ] **Step 9: Write `README.md` with the start commands**

````markdown
# marimo S0 spike — throwaway, never merged

```bash
uv run uvicorn fake_egret:app --host 127.0.0.1 --port 8700            # terminal 1
uv run uvicorn proxy:app --host 127.0.0.1 --port 8701                 # terminal 2
APPS_WORKDIR=$PWD/.work/edit uv run python entrypoint.py --mode edit  # terminal 3
APPS_WORKDIR=$PWD/.work/run  uv run python entrypoint.py --mode run --process-mode thread  # terminal 4
```

Browser URLs: `http://egret.localhost:8700/login?user=alice`, then `http://egret.localhost:8700/apps/probe/run`.

## Findings
(append one line per surprise, with the file/line in site-packages that explains it)
````

- [ ] **Step 10: Smoke-test the whole chain by hand**

Start the four processes as the README says. Then run:

```bash
uv run python - <<'EOF'
import asyncio
from playwright.async_api import async_playwright
async def main():
    async with async_playwright() as p:
        b = await p.chromium.launch()
        pg = await b.new_page()
        await pg.goto("http://egret.localhost:8700/login?user=alice")
        await pg.goto("http://egret.localhost:8700/apps/probe/run")
        f = pg.frame_locator("#app")
        await f.get_by_text("PY pid=").wait_for(timeout=60000)
        print(await f.get_by_text("PY pid=").inner_text())
        await pg.screenshot(path="results/smoke.png", full_page=True)
        await b.close()
asyncio.run(main())
EOF
```

Expected: a line such as `PY pid=12345 viewer=alice sql=SELECT 1 AS n`.

If it fails, fix the spike rather than the question. For each fix, add a line under "Findings" in the README. Likely suspects:
- the Bearer auth on marimo's WebSocket (`site-packages/marimo/_server/api/auth.py`);
- `--base-url` handling;
- the skew-protection header;
- `request.headers` shape in `egret_marimo._token`.

A failure that cannot be fixed without changing marimo is itself an answer to question 4: stop and record it.

- [ ] **Step 11: Commit**

```bash
git add spikes/marimo-s0 && git commit -m "spike(marimo-s0): egret_marimo, entrypoint, apps-origin proxy"
```

---

### Task 3: Probe Q1 (token reaches the right kernel) and Q5 (SQL cell engine)

**Files:**
- Create: `spikes/marimo-s0/probes/common.py`, `spikes/marimo-s0/probes/q1_token.py`
- Output: `spikes/marimo-s0/results/q1.json`, `results/q5.png`

**Interfaces:**
- Consumes: the running stack from Task 2 (run server in `--process-mode thread`), and `apps/probe.py`.
- Produces: `probes.common` with `open_app(browser, user, app_id, mode) -> (page, frame)`, `events() -> list[dict]`, `reset_events() -> None`, `wait_text(frame, prefix) -> str`.

- [ ] **Step 1: Write `probes/common.py`**

```python
import httpx

EGRET_PUBLIC = "http://egret.localhost:8700"
EGRET_LOCAL = "http://127.0.0.1:8700"


async def open_app(browser, user: str, app_id: str, mode: str):
    ctx = await browser.new_context()
    page = await ctx.new_page()
    await page.goto(f"{EGRET_PUBLIC}/login?user={user}")
    await page.goto(f"{EGRET_PUBLIC}/apps/{app_id}/{mode}")
    return page, page.frame_locator("#app")


async def wait_text(frame, prefix: str, timeout: int = 90000) -> str:
    loc = frame.get_by_text(prefix).first
    await loc.wait_for(timeout=timeout)
    return await loc.inner_text()


def events() -> list[dict]:
    return httpx.get(f"{EGRET_LOCAL}/_events").json()


def reset_events() -> None:
    httpx.post(f"{EGRET_LOCAL}/_events/reset")
```

- [ ] **Step 2: Write `probes/q1_token.py`**

```python
"""Q1: the header reaches the right session's kernel, including reactive re-runs. Q5: SQL cell uses the engine."""

import asyncio
import json
import pathlib

from playwright.async_api import async_playwright

from common import events, open_app, reset_events, wait_text

OUT = pathlib.Path(__file__).parent.parent / "results"


async def bump_slider(frame) -> None:
    slider = frame.get_by_role("slider").first
    await slider.focus()
    for _ in range(3):
        await slider.press("ArrowRight")


async def main() -> None:
    reset_events()
    async with async_playwright() as p:
        browser = await p.chromium.launch()
        (pa, fa), (pb, fb) = await asyncio.gather(
            open_app(browser, "alice", "probe", "run"), open_app(browser, "bob", "probe", "run")
        )
        first_a, first_b = await asyncio.gather(wait_text(fa, "PY pid="), wait_text(fb, "PY pid="))
        await asyncio.gather(bump_slider(fa), bump_slider(fb))
        await asyncio.sleep(3)
        after_a, after_b = await asyncio.gather(wait_text(fa, "PY pid="), wait_text(fb, "PY pid="))
        sqlcell_a = await wait_text(fa, "SQLCELL")
        await pa.screenshot(path=str(OUT / "q5.png"), full_page=True)
        await browser.close()

    queries = [e for e in events() if e["kind"] == "query"]
    result = {
        "alice": {"first": first_a, "after_slider": after_a, "sqlcell": sqlcell_a},
        "bob": {"first": first_b, "after_slider": after_b},
        "gateway_queries": queries,
        "every_alice_page_shows_alice": all("viewer=alice" in s for s in (first_a, after_a)),
        "every_bob_page_shows_bob": all("viewer=bob" in s for s in (first_b, after_b)),
        "sql_cell_used_gateway": "viewer=alice" in sqlcell_a,
        "rejected_queries": [e for e in events() if e["kind"] == "query_rejected"],
    }
    (OUT / "q1.json").write_text(json.dumps(result, indent=2))
    print(json.dumps({k: v for k, v in result.items() if k != "gateway_queries"}, indent=2))


asyncio.run(main())
```

- [ ] **Step 3: Run it**

Run: `cd probes && uv run python q1_token.py`
Expected: `every_alice_page_shows_alice: true`, `every_bob_page_shows_bob: true`, `sql_cell_used_gateway: true`, `rejected_queries: []`, and `gateway_queries` holds at least 8 entries whose `sql` values include `n` greater than 1 (the reactive re-runs).

A `false` is an answer, not a bug to hide. Record which read returned the wrong viewer, then test the fallback: append `&egret_token=<token>` to the target URL in `proxy.ticket`, and read it from `request.query_params` in `_token()`. Save that run as `results/q1-fallback.json`.

- [ ] **Step 4: Check Q5 manually in the editor**

Open `http://egret.localhost:8700/apps/probe/edit` as alice in a headed browser. Add a new SQL cell, open its engine dropdown, and screenshot it as `results/q5-dropdown.png`. Record whether `db` is listed (manual check).

- [ ] **Step 5: Commit**

```bash
git add spikes/marimo-s0/probes spikes/marimo-s0/results && git commit -m "spike(marimo-s0): Q1 token per session, Q5 SQL cell engine"
```

---

### Task 4: Probe Q2 (one process per run session, and its cost)

**Files:**
- Create: `spikes/marimo-s0/probes/q2_process.py`
- Output: `spikes/marimo-s0/results/q2-thread.json`, `q2-sandbox.json`, `q2-ipc.json`

**Interfaces:**
- Consumes: `probes.common`; the run server restarted with each `--process-mode`.

- [ ] **Step 1: Write `probes/q2_process.py`**

```python
"""Q2: distinct kernel pids per viewer session, and time to first render, for one process mode."""

import asyncio
import json
import pathlib
import sys
import time

from playwright.async_api import async_playwright

from common import open_app, wait_text

MODE = sys.argv[1]  # thread | sandbox | ipc — must match how the run server was started
OUT = pathlib.Path(__file__).parent.parent / "results"


async def one(browser, user):
    t0 = time.monotonic()
    _, frame = await open_app(browser, user, "probe", "run")
    text = await wait_text(frame, "PY pid=", timeout=300000)
    return {"user": user, "pid": int(text.split("pid=")[1].split()[0]), "seconds": round(time.monotonic() - t0, 2)}


async def main() -> None:
    async with async_playwright() as p:
        browser = await p.chromium.launch()
        cold = await one(browser, "u0")
        warm = await asyncio.gather(*(one(browser, f"u{i}") for i in range(1, 4)))
        await browser.close()
    sessions = [cold, *warm]
    result = {
        "mode": MODE,
        "sessions": sessions,
        "distinct_pids": len({s["pid"] for s in sessions}),
        "max_seconds": max(s["seconds"] for s in sessions),
    }
    (OUT / f"q2-{MODE}.json").write_text(json.dumps(result, indent=2))
    print(json.dumps(result, indent=2))


asyncio.run(main())
```

- [ ] **Step 2: Measure thread mode (the baseline)**

The run server was started with `--process-mode thread`. Run: `cd probes && uv run python q2_process.py thread`
Expected: `distinct_pids: 1`, because every session is a thread in the server process. This confirms the problem the spec's D7 is about.

- [ ] **Step 3: Measure the sandbox mode, offline, with a warm uv cache**

Restart the run server: `APPS_WORKDIR=$PWD/.work/run uv run python entrypoint.py --mode run --process-mode sandbox`. Load the app once so uv fills its cache, then restart the server with `UV_OFFLINE=1` in its environment.

Run: `cd probes && uv run python q2_process.py sandbox`
Record `distinct_pids` (4 is the goal) and `max_seconds`. If the offline start fails, record the error text. A sandbox that needs PyPI cannot run in a pod whose egress is closed, which is itself the answer for this variant.

- [ ] **Step 4: Measure the forced-IPC mode**

Restart the run server with `--process-mode ipc`.
Run: `cd probes && uv run python q2_process.py ipc`
Record `distinct_pids` and `max_seconds`. Also run `probes/q1_token.py` again in this mode, saved as `results/q1-ipc.json`: the token must still reach the right kernel through the subprocess boundary.

- [ ] **Step 5: Measure memory per session in the best mode**

With 4 sessions open, run `ps -o pid,rss,command -p <each pid>` and save the output as `results/q2-rss.txt`. The spec's run-pod limit of 20 sessions per pod depends on this number.

- [ ] **Step 6: Commit**

```bash
git add spikes/marimo-s0 && git commit -m "spike(marimo-s0): Q2 process per session — thread vs sandbox vs ipc"
```

---

### Task 5: Probe Q3 (draft round-trip and what the author sees on 409)

**Files:**
- Create: `spikes/marimo-s0/probes/q3_storage.py`
- Output: `spikes/marimo-s0/results/q3.json`, `results/q3-after-save.png`, `results/q3-conflict.png`

**Interfaces:**
- Consumes: `probes.common`; the edit server from Task 2; the fake egret draft API.

- [ ] **Step 1: Write `probes/q3_storage.py`**

```python
"""Q3: an edit saved in marimo lands in egret's draft; a stale save gets 409 — what does the author see?"""

import asyncio
import json
import pathlib

import httpx
from playwright.async_api import async_playwright

from common import EGRET_LOCAL, events, open_app, reset_events, wait_text

OUT = pathlib.Path(__file__).parent.parent / "results"
POD = {"authorization": "Bearer pod-kernel-token"}
MARKER = "s0_marker = 12345"


def draft() -> dict:
    return httpx.get(f"{EGRET_LOCAL}/api/apps/probe/draft", headers=POD).json()


async def add_cell_and_save(page, frame, code: str) -> None:
    await frame.get_by_text("PY pid=").first.wait_for(timeout=90000)
    editor = frame.locator(".cm-content").last
    await editor.click()
    await page.keyboard.press("End")
    await page.keyboard.press("Enter")
    await page.keyboard.type(code)
    await page.keyboard.press("Meta+s")
    await asyncio.sleep(2)


async def main() -> None:
    reset_events()
    before = draft()
    async with async_playwright() as p:
        browser = await p.chromium.launch()
        page, frame = await open_app(browser, "alice", "probe", "edit")
        await add_cell_and_save(page, frame, MARKER)
        await page.screenshot(path=str(OUT / "q3-after-save.png"), full_page=True)
        saved = draft()

        # someone else saves first
        httpx.put(f"{EGRET_LOCAL}/api/apps/probe/draft", content=(saved["source"] + "\n# other editor\n").encode(),
                  headers={**POD, "if-match": str(saved["revision"])})

        await add_cell_and_save(page, frame, "s0_second = 1")
        await page.screenshot(path=str(OUT / "q3-conflict.png"), full_page=True)
        visible = await frame.locator("body").inner_text()
        await browser.close()

    result = {
        "revision_before": before["revision"],
        "revision_after_first_save": saved["revision"],
        "marker_in_draft": MARKER in saved["source"],
        "conflict_events": [e for e in events() if e["kind"] == "draft_conflict"],
        "ui_mentions_conflict": any(w in visible.lower() for w in ("reload", "someone else", "conflict", "409")),
        "ui_text_excerpt": visible[-2000:],
    }
    (OUT / "q3.json").write_text(json.dumps(result, indent=2))
    print(json.dumps({k: v for k, v in result.items() if k != "ui_text_excerpt"}, indent=2))


asyncio.run(main())
```

- [ ] **Step 2: Run it**

Run: `cd probes && uv run python q3_storage.py`

Expected:
- `marker_in_draft: true`, and `revision_after_first_save` equals `revision_before + 1`.
- At least one `conflict_events` entry.

`ui_mentions_conflict` is the open question. `false` means the spec's fallback applies: the egret header shows the conflict. If the editor's selectors (`.cm-content`, `Meta+s`) do not match marimo 0.25.0, adjust them to what the page actually uses, and note the working selectors in the README.

- [ ] **Step 3: Inspect the conflict screenshot by hand and describe it**

Open `results/q3-conflict.png` and note in `results/q3.json`, as `"manual_description"`, what the author sees: a toast, an unsaved-changes dot, or nothing at all.

- [ ] **Step 4: Commit**

```bash
git add spikes/marimo-s0 && git commit -m "spike(marimo-s0): Q3 draft round-trip and 409 visibility"
```

---

### Task 6: Probe Q4 (separate origin, ticket → cookie, and what author JavaScript can reach)

**Files:**
- Create: `spikes/marimo-s0/apps/xss.py`, `spikes/marimo-s0/probes/q4_origin.py`
- Output: `spikes/marimo-s0/results/q4.json`, `results/q4-host-prefix.json`

**Interfaces:**
- Consumes: `probes.common`; the run server restarted with `--app xss`; fake egret started once plainly and once with `COOKIE_PREFIX=__Host-`.

- [ ] **Step 1: Write the attacker notebook `apps/xss.py`**

```python
import marimo

__generated_with = "0.25.0"
app = marimo.App()


@app.cell
def _():
    import anywidget
    import marimo as mo
    import traitlets

    return anywidget, mo, traitlets


@app.cell
def _(anywidget, mo, traitlets):
    class Probe(anywidget.AnyWidget):
        _esm = """
        export default { async render({ el }) {
          const out = {};
          try { out.parentDoc = String(!!window.parent.document.title); }
          catch (e) { out.parentDoc = "blocked:" + e.name; }
          out.cookieVisible = document.cookie;
          try {
            const r = await fetch("http://egret.localhost:8700/api/whoami", { credentials: "include" });
            out.whoamiReadable = "status " + r.status + " " + (await r.text());
          } catch (e) { out.whoamiReadable = "blocked:" + e.name; }
          try {
            const r = await fetch("http://egret.localhost:8700/api/mutate",
              { method: "POST", credentials: "include", headers: { "x-egret-csrf": "evil" } });
            out.mutateWithHeader = "status " + r.status;
          } catch (e) { out.mutateWithHeader = "blocked:" + e.name; }
          document.cookie = "egret_csrf=evil; domain=egret.localhost; path=/";
          document.cookie = "__Host-egret_csrf=evil; domain=egret.localhost; path=/; secure";
          out.cookieAfterToss = document.cookie;
          try {
            await fetch("http://egret.localhost:8700/api/mutate",
              { method: "POST", credentials: "include", mode: "no-cors" });
            out.mutateNoCors = "sent";
          } catch (e) { out.mutateNoCors = "blocked:" + e.name; }
          el.innerHTML = "<pre id='xss-result'>XSSRESULT " + JSON.stringify(out) + "</pre>";
        } };
        """
        value = traitlets.Int(0).tag(sync=True)

    Probe()
    return


@app.cell
def _(mo):
    mo.Html("<div id='html-script'>HTMLSCRIPT pending</div><script>document.getElementById('html-script').textContent='HTMLSCRIPT executed'</script>")
    return


@app.cell
def _(mo):
    mo.Html("<div id='html-onerror'>ONERROR pending</div><img src=x onerror=\"document.getElementById('html-onerror').textContent='ONERROR executed'\">")
    return


if __name__ == "__main__":
    app.run()
```

- [ ] **Step 2: Write `probes/q4_origin.py`**

```python
"""Q4: marimo works behind the proxy on its own origin; the ticket is single-use; author JS cannot reach egret."""

import asyncio
import json
import os
import pathlib

from playwright.async_api import async_playwright

from common import events, open_app, reset_events, wait_text

OUT = pathlib.Path(__file__).parent.parent / "results"
SUFFIX = "-host-prefix" if os.environ.get("COOKIE_PREFIX") == "__Host-" else ""


async def main() -> None:
    reset_events()
    async with async_playwright() as p:
        browser = await p.chromium.launch()
        page, frame = await open_app(browser, "alice", "xss", "run")
        xss = await wait_text(frame, "XSSRESULT")
        html_script = await wait_text(frame, "HTMLSCRIPT")
        onerror = await wait_text(frame, "ONERROR")
        iframe_src = await page.locator("#app").get_attribute("src")
        ticket = iframe_src.split("t=")[1]
        replay = await page.context.request.get(f"http://apps.egret.localhost:8701/_ticket?t={ticket}", max_redirects=0)
        no_cookie_ctx = await browser.new_context()
        direct = await no_cookie_ctx.request.get("http://apps.egret.localhost:8701/r/")
        await browser.close()

    evs = events()
    result = {
        "author_js": json.loads(xss.removeprefix("XSSRESULT ")),
        "mo_html_script": html_script,
        "mo_html_onerror": onerror,
        "ticket_replay_status": replay.status,
        "apps_without_cookie_status": direct.status,
        "egret_saw_whoami": [e for e in evs if e["kind"] == "whoami"],
        "egret_saw_mutate": [e for e in evs if e["kind"] == "mutate"],
        "any_mutate_succeeded": any(e["kind"] == "mutate" and e["ok"] for e in evs),
    }
    (OUT / f"q4{SUFFIX}.json").write_text(json.dumps(result, indent=2))
    print(json.dumps(result, indent=2))


asyncio.run(main())
```

- [ ] **Step 3: Run it against plain cookie names**

Restart the run server with `--app xss`, then run: `cd probes && uv run python q4_origin.py`

Expected:
- `author_js.parentDoc` starts with `blocked:`.
- `author_js.cookieVisible` contains no `egret_` cookie.
- `author_js.whoamiReadable` is `blocked:TypeError` (CORS).
- `author_js.mutateWithHeader` is `blocked:TypeError` (the preflight fails).
- `any_mutate_succeeded` is false.
- `ticket_replay_status` is 403 and `apps_without_cookie_status` is 401.

Record the rest as observed, because it describes the attack surface:
- whether egret logged `whoami` with `user: alice` (a same-site cookie was sent, even though the response was unreadable);
- whether `mutate`'s `raw_cookie` shows **two** `egret_csrf` values (the toss reached egret);
- whether `mo_html_script` and `mo_html_onerror` say `executed`.

- [ ] **Step 4: Run it against `__Host-` cookie names**

Restart the fake egret with `COOKIE_PREFIX=__Host-`, then run: `cd probes && COOKIE_PREFIX=__Host- uv run python q4_origin.py`

Expected: `mutate`'s `raw_cookie` never contains a tossed `__Host-egret_csrf=evil`. If Chromium refuses to set `Secure` cookies on `http://egret.localhost`, the login itself fails. Record that, and note that this check must be repeated over HTTPS on kind in P4.

- [ ] **Step 5: Commit**

```bash
git add spikes/marimo-s0 && git commit -m "spike(marimo-s0): Q4 separate origin, ticket, author-JS reach"
```

---

### Task 7: Probe Q6 (which routes the proxy blocks)

**Files:**
- Create: `spikes/marimo-s0/probes/q6_routes.py`
- Output: `spikes/marimo-s0/results/q6-routes.json`, `results/q6.json`

**Interfaces:**
- Consumes: `marimo._server.api.router.build_routes`; the stack restarted with `S0_BLOCKED` set on the proxy; `probes/q3_storage.py` as the "editing still works" check.

- [ ] **Step 1: Write `probes/q6_routes.py`, which lists every marimo route**

```python
"""Q6: enumerate marimo's routes so the proxy's block list is derived, not guessed."""

import json
import pathlib

from marimo._server.api.router import build_routes

OUT = pathlib.Path(__file__).parent.parent / "results"


def walk(routes, prefix=""):
    for r in routes:
        path = prefix + getattr(r, "path", "")
        children = getattr(r, "routes", None)
        if children:
            yield from walk(children, path)
        else:
            methods = sorted(getattr(r, "methods", None) or ["WEBSOCKET"])
            yield {"path": path, "methods": methods}


routes = sorted(walk(build_routes("")), key=lambda r: r["path"])
(OUT / "q6-routes.json").write_text(json.dumps(routes, indent=2))
for r in routes:
    print(f'{",".join(r["methods"]):20} {r["path"]}')
```

Run: `cd probes && uv run python q6_routes.py`
Expected: a list that includes `/terminal/…`, `/api/packages/…`, `/api/secrets/…`, `/api/files/…`, `/api/ai/…`, `/api/storage/…`, `/api/home/…`, `/api/kernel/save`.

- [ ] **Step 2: Choose the candidate block list from the output and record the reasoning**

Start from this candidate and adjust it to the real paths printed in Step 1:

```text
/terminal,/api/packages,/api/secrets,/api/files,/api/ai,/api/storage,/api/home
```

In `results/q6.json`, record each prefix with a one-line reason. Also record every `/api/kernel/*` route that creates, renames or copies files (for example `save_as`, `rename`, `copy`), and add those routes to the list as full paths.

- [ ] **Step 3: Re-run the editing probes with the block list on**

Restart the proxy with `S0_BLOCKED=<the list>`. Then run `uv run python q3_storage.py` and `uv run python q1_token.py` again (the latter against a run server in thread mode).

Expected: both give the same answers as before. Anything that breaks means a prefix is too broad. Narrow it, re-run, and record which route the editor needed.

- [ ] **Step 4: Confirm the terminal is unreachable**

Open the edit app as alice and try to open marimo's terminal panel. Screenshot it as `results/q6-terminal.png` (manual check). The proxy's access output should show 403 or 4403 for `/e/terminal/…`.

- [ ] **Step 5: Commit**

```bash
git add spikes/marimo-s0 && git commit -m "spike(marimo-s0): Q6 route inventory and block list"
```

---

### Task 8: Result document, and spec updates the answers require

**Files:**
- Create (on `docs/marimo-data-apps-spec`): `docs/en/plans/<run-date>-marimo-s0-result.md`
- Modify if a fallback triggered: `docs/en/specs/2026-09-30-marimo-data-apps-design.md` and `docs/vi/specs/2026-09-30-marimo-data-apps-design.md`

**Interfaces:**
- Consumes: every `results/*` file and the spike README's "Findings".

- [ ] **Step 1: Write the result document**

Use this structure. Every cell is filled from a result file, and "manual" marks anything checked by hand:

```markdown
# Data Apps S0 — result

**Run:** <date>, marimo 0.25.0, Chromium <version>, branch `spike/marimo-s0` @ <sha> (never merged)
**Spec:** `docs/en/specs/2026-09-30-marimo-data-apps-design.md` §9

| # | Question | Answer | Evidence | Design consequence |
|---|---|---|---|---|
| 1 | Header reaches the right kernel, incl. reactive re-runs | yes/no | results/q1.json: … | none / fallback: … |
| 2 | Process per run session | thread: 1 pid; sandbox: n pids, s s; ipc: n pids, s s | results/q2-*.json, q2-rss.txt | D7 via <mode> / fallback pod per viewer |
| 3 | Draft round-trip; what the author sees on 409 | … | results/q3.json, q3-conflict.png (manual) | header-driven 409 needed: yes/no |
| 4 | Separate origin + ticket → cookie; author-JS reach | … | results/q4.json, q4-host-prefix.json | … |
| 5 | SQL cell accepts the DB-API connection | … | results/q1.json sql_cell_used_gateway, q5-dropdown.png (manual) | … |
| 6 | Block list | <final list> | results/q6.json | proxy block list for P4 |

## Findings
<one paragraph per surprise, with the site-packages file:line that explains it>

## What the spec must now say
<exact edits, or "no change">
```

- [ ] **Step 2: Apply the spec edits that the answers require**

For each fallback that triggered, edit the matching spec section in `en`, then mirror the edit in `vi`:

| Fallback | Spec section to edit |
|---|---|
| Token passed by query parameter | §4.1 |
| One pod per viewer | D7 and §6.3 |
| 409 shown by the egret header | §5.1 |
| Anything learned about `mo.Html` scripts | §6.1 |
| The final block list | §6.2 |

Mark the spec's §9 S0 section **done**, with a link to the result document.

- [ ] **Step 3: Commit on the spec branch**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret-wt-marimo-spec
git add docs && git commit -m "docs: Data Apps S0 result — <one-line summary of the answers>"
```

- [ ] **Step 4: Report to the user and stop**

Summarise each answer in one line, which fallbacks triggered, and whether question 4 passed. If it failed, the spec's architecture must be revisited before any P1 plan is written. Do not start P1 without the user's go-ahead.
