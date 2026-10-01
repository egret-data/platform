"""A test stand-in for egret's apps proxy: injects the viewer's and marimo's tokens.

The viewer is chosen by the ``egret_test_user`` cookie. Every request to marimo
gets ``x-egret-app-token: <that viewer's session token>`` and
``Authorization: Bearer <marimo token>``. Modelled on the S0 spike's proxy.py,
without ticket redemption, route blocking or raw-path checks (P4 owns those).
"""

from __future__ import annotations

import asyncio
import contextlib

import httpx
import websockets
from starlette.applications import Starlette
from starlette.background import BackgroundTask
from starlette.requests import Request
from starlette.responses import PlainTextResponse, Response, StreamingResponse
from starlette.routing import Route, WebSocketRoute
from starlette.websockets import WebSocket, WebSocketDisconnect

COOKIE = "egret_test_user"
HOP_BY_HOP = {
    "connection", "keep-alive", "proxy-authenticate", "proxy-authorization", "te",
    "trailers", "transfer-encoding", "upgrade", "host", "content-length",
}
WS_HANDSHAKE = {
    "sec-websocket-key", "sec-websocket-version", "sec-websocket-extensions",
    "sec-websocket-accept", "sec-websocket-protocol",
}


def forward_headers(incoming, session_token: str, marimo_token: str) -> dict[str, str]:
    out = {
        k: v
        for k, v in ((k.lower(), v) for k, v in incoming.items())
        if k not in HOP_BY_HOP and k not in WS_HANDSHAKE
        and k not in ("cookie", "authorization") and not k.startswith("x-egret-")
    }
    out["authorization"] = f"Bearer {marimo_token}"
    out["x-egret-app-token"] = session_token
    return out


def make_app(upstream_port: int, tokens: dict[str, str], marimo_token: str) -> Starlette:
    """``tokens`` maps a viewer (cookie value) to its session token."""
    upstream = f"127.0.0.1:{upstream_port}"
    clients: list[httpx.AsyncClient] = []

    def client() -> httpx.AsyncClient:
        if not clients:  # created on the server's own event loop
            clients.append(httpx.AsyncClient(timeout=httpx.Timeout(60.0)))
        return clients[0]

    async def relay(request: Request) -> Response:
        token = tokens.get(request.cookies.get(COOKIE, ""))
        if token is None:
            return PlainTextResponse("no viewer", status_code=401)
        url = f"http://{upstream}{request.url.path}"
        if request.url.query:
            url += f"?{request.url.query}"
        c = client()
        r = await c.send(
            c.build_request(
                request.method, url,
                headers=forward_headers(request.headers, token, marimo_token),
                content=await request.body(),
            ),
            stream=True,
        )
        headers = {k: v for k, v in r.headers.items() if k.lower() not in HOP_BY_HOP}
        return StreamingResponse(
            r.aiter_raw(), status_code=r.status_code, headers=headers, background=BackgroundTask(r.aclose)
        )

    async def ws_relay(ws: WebSocket) -> None:
        token = tokens.get(ws.cookies.get(COOKIE, ""))
        if token is None:
            await ws.close(code=4401)
            return
        await ws.accept()
        url = f"ws://{upstream}{ws.url.path}" + (f"?{ws.url.query}" if ws.url.query else "")
        headers = forward_headers(ws.headers, token, marimo_token)
        headers.pop("origin", None)
        async with websockets.connect(url, additional_headers=headers, max_size=None, open_timeout=30) as up:

            async def client_to_upstream() -> None:
                while True:
                    msg = await ws.receive()
                    if msg["type"] == "websocket.disconnect":
                        return
                    await up.send(msg["text"] if msg.get("text") is not None else msg["bytes"])

            async def upstream_to_client() -> None:
                async for msg in up:
                    await (ws.send_text(msg) if isinstance(msg, str) else ws.send_bytes(msg))

            tasks = [asyncio.create_task(client_to_upstream()), asyncio.create_task(upstream_to_client())]
            await asyncio.wait(tasks, return_when=asyncio.FIRST_COMPLETED)
            for t in tasks:
                t.cancel()
        with contextlib.suppress(RuntimeError, WebSocketDisconnect):
            await ws.close()

    methods = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"]
    return Starlette(
        routes=[
            WebSocketRoute("/{path:path}", ws_relay),
            Route("/{path:path}", relay, methods=methods),
        ]
    )
