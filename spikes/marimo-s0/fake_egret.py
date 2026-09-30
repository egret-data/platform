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
