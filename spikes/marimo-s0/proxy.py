"""The apps-origin proxy, spike version (spec §6.1, §6.2)."""

import asyncio
import os
import re
import secrets
from urllib.parse import unquote

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


_AMBIGUOUS = re.compile(r"%(2e|2f|5c|25|00)|\\", re.I)


def forwarded_path(scope) -> str | None:
    """The one path that is both checked and forwarded, taken from the bytes the client sent.
    Anything ambiguous (dot segments, empty segments, encoded dots/slashes/percent, backslash, NUL) is rejected (None), never rewritten."""
    raw = scope["raw_path"].decode("latin-1").split("?", 1)[0]
    if _AMBIGUOUS.search(raw) or "//" in raw or not raw.startswith("/"):
        return None
    decoded = unquote(raw)
    if any(seg in (".", "..") for seg in decoded.split("/")) or "\x00" in decoded or "\\" in decoded:
        return None
    return raw


def _blocked(prefix: str, path: str) -> bool:
    rest = unquote(path)[len(prefix) + 1:].lower()
    return any(rest.startswith(b.lower()) for b in BLOCKED)


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
    path = forwarded_path(request.scope)
    if path is None:
        return PlainTextResponse("ambiguous path rejected", status_code=400)
    if _blocked(prefix, path):
        return PlainTextResponse("blocked by egret", status_code=403)
    url = f"http://{UPSTREAMS[prefix]}{path}"
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
    path = forwarded_path(ws.scope) if sess else None
    if not sess or path is None or _blocked(prefix, path):
        await ws.close(code=4403)
        return
    await ws.accept()
    url = f"ws://{UPSTREAMS[prefix]}{path}" + (f"?{ws.url.query}" if ws.url.query else "")
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
