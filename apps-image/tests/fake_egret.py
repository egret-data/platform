"""A fake egret: the pod API (spec 5.3) and the query gateway, in memory."""

import socket
import threading
import time

import uvicorn
from starlette.applications import Starlette
from starlette.requests import Request
from starlette.responses import JSONResponse, Response
from starlette.routing import Route

MAX_SOURCE = 1024 * 1024


class State:
    def __init__(self) -> None:
        self.reset()

    def reset(self) -> None:
        self.drafts: dict[str, dict] = {}
        self.versions: dict[tuple[str, str], dict] = {}
        self.pod_tokens: dict[str, dict] = {}
        self.session_tokens: dict[str, dict] = {}
        self.queries: list[dict] = []
        self.events: list[dict] = []


STATE = State()


def _bearer(request: Request) -> str:
    return request.headers.get("authorization", "").removeprefix("Bearer ")


def _pod_ok(request: Request) -> bool:
    return _bearer(request) in STATE.pod_tokens


async def draft(request: Request) -> Response:
    if not _pod_ok(request):
        return JSONResponse({"error": "unauthorized"}, status_code=401)
    app_id = request.path_params["app_id"]
    d = STATE.drafts.get(app_id)
    if d is None:
        return JSONResponse({"error": "not found"}, status_code=404)
    if request.method == "GET":
        return JSONResponse(d)
    sent = request.headers.get("if-match")
    if sent is None:
        return JSONResponse({"error": "If-Match required"}, status_code=428)
    try:
        source = (await request.json())["source"]
        if not isinstance(source, str):
            raise TypeError
    except (ValueError, KeyError, TypeError):
        return JSONResponse({"error": 'body must be JSON {"source": "..."}'}, status_code=400)
    if len(source.encode()) > MAX_SOURCE:
        return JSONResponse({"error": "source too large"}, status_code=413)
    if sent.strip('"') != str(d["revision"]):
        STATE.events.append({"kind": "draft_conflict", "app_id": app_id, "sent": sent})
        return JSONResponse(
            {"error": "stale revision", "current_revision": d["revision"]}, status_code=409
        )
    d["source"] = source
    d["revision"] += 1
    STATE.events.append({"kind": "draft_saved", "app_id": app_id, "if_match": sent})
    return JSONResponse({"revision": d["revision"]})


async def version(request: Request) -> Response:
    if not _pod_ok(request):
        return JSONResponse({"error": "unauthorized"}, status_code=401)
    key = (request.path_params["app_id"], request.path_params["version_id"])
    v = STATE.versions.get(key)
    if v is None:
        return JSONResponse({"error": "not found"}, status_code=404)
    return JSONResponse(v)


async def query(request: Request) -> Response:
    token = _bearer(request)
    info = STATE.session_tokens.get(token)
    if info is None:
        return JSONResponse({"error": "session expired"}, status_code=401)
    body = await request.json()
    if body.get("sql") == "BAD":
        return JSONResponse({"error": "syntax error near BAD"}, status_code=400)
    STATE.queries.append(
        {
            "token": token,
            "user": info["user"],
            "database": body.get("database"),
            "sql": body.get("sql"),
            "params": body.get("params"),
            "pid": body.get("pid"),
        }
    )
    return JSONResponse(
        {
            "columns": [{"name": "viewer"}, {"name": "sql"}],
            "rows": [[info["user"], body["sql"]]],
            "truncated": False,
            "row_count": 1,
        }
    )


app = Starlette(
    routes=[
        Route("/api/apps/pod/apps/{app_id}/draft", draft, methods=["GET", "PUT"]),
        Route("/api/apps/pod/apps/{app_id}/versions/{version_id}", version, methods=["GET"]),
        Route("/api/apps/query", query, methods=["POST"]),
    ]
)


def serve(port: int = 0) -> tuple[uvicorn.Server, int]:
    """Run the app in a uvicorn thread; returns (server, port). Stop with server.should_exit = True."""
    if port == 0:
        with socket.socket() as s:
            s.bind(("127.0.0.1", 0))
            port = s.getsockname()[1]
    server = uvicorn.Server(uvicorn.Config(app, host="127.0.0.1", port=port, log_level="warning"))
    threading.Thread(target=server.run, daemon=True).start()
    while not server.started:
        time.sleep(0.02)
    return server, port
