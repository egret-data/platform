"""Q6 bypass probe: raw (non-normalised) request paths through the apps proxy. usage: q6_bypass.py <out.json>
Raw sockets so the path goes on the wire exactly as written (browsers and httpx would normalise it)."""

import base64
import json
import pathlib
import re
import socket
import sys

import httpx

OUT = pathlib.Path(__file__).parent.parent / "results" / sys.argv[1]
EGRET, APPS = ("127.0.0.1", 8700), ("127.0.0.1", 8701)


def cookie(mode: str) -> str:
    with httpx.Client(follow_redirects=False) as c:
        c.get(f"http://127.0.0.1:8700/login?user=alice")
        page = c.get(f"http://127.0.0.1:8700/apps/probe/{mode}").text
        t = re.search(r"_ticket\?t=([\w-]+)", page).group(1)
        r = httpx.get(f"http://127.0.0.1:8701/_ticket?t={t}")
        return r.cookies["apps_sid"]


def raw(head: str, ck: str, ws: bool) -> dict:
    req = f"{head} HTTP/1.1\r\nHost: apps.egret.localhost:8701\r\nCookie: apps_sid={ck}\r\nConnection: close\r\n"
    if ws:
        req += "Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: " + base64.b64encode(b"0123456789abcdef").decode() + "\r\n"
    else:
        req += "Content-Length: 0\r\n" if head.startswith("POST") else ""
    ws_after = ""
    s = socket.create_connection(APPS, timeout=3)
    s.sendall((req + "\r\n").encode())
    data = b""
    closed = False
    try:
        while len(data) < 4000:
            b = s.recv(2000)
            if not b:
                closed = True
                break
            data += b
            if not ws and b"\r\n\r\n" in data:
                break
    except socket.timeout:
        pass
    s.close()
    if ws:
        head, _, after = data.partition(b"\r\n\r\n")
        ws_after = ("server closed connection; " if closed else "connection held open; ") + f"{len(after)} bytes after handshake: " + after[:60].hex()
    first = data.split(b"\r\n", 1)[0].decode(errors="replace")
    return {"status_line": first, "body_head": data.split(b"\r\n\r\n", 1)[-1][:120].decode(errors="replace") if not ws else "",
            "ws_after_handshake": ws_after}


CASES = []
for pfx in ("e", "r"):
    for label, tail in [("plain", "{p}/terminal/ws"), ("dot-dot", "{p}/x/../terminal/ws"), ("dot", "{p}/./terminal/ws"),
                        ("double-slash", "{p}//terminal/ws"), ("upper-case", "{p}/Terminal/ws"),
                        ("encoded-dotdot", "{p}/%2e%2e/{q}/terminal/ws"), ("x-encoded-dotdot", "{p}/x/%2e%2e/terminal/ws"),
                        ("encoded-slash", "{p}/x/..%2fterminal/ws"), ("pct-letter", "{p}/%74erminal/ws"), ("encoded-dot-single", "{p}/%2e/terminal/ws")]:
        CASES.append(("ws", pfx, label, "GET", tail.format(p="/" + pfx, q=pfx)))
    for label, tail in [("plain", "{p}/api/packages/list"), ("dot-dot", "{p}/x/../api/packages/list"),
                        ("double-slash", "{p}//api/packages/list"), ("upper-case", "{p}/API/packages/list"),
                        ("encoded-dotdot", "{p}/x/%2e%2e/api/packages/list"), ("encoded-slash", "{p}/x/..%2fapi/packages/list"),
                        ("pct-letter", "{p}/%61pi/packages/list"), ("trailing-dot-seg", "{p}/api/packages/x/../list")]:
        CASES.append(("http", pfx, label, "GET", tail.format(p="/" + pfx)))
    CASES.append(("http", pfx, "dot-dot POST files", "POST", f"/{pfx}/x/../api/files/list_files"))
    CASES.append(("http", pfx, "plain POST files", "POST", f"/{pfx}/api/files/list_files"))

res = {"note": "raw socket, path as-is; 101/200 = reached marimo, 403/400/404 = stopped", "cases": []}
ck = {"e": cookie("edit"), "r": cookie("run")}
for kind, pfx, label, method, path in CASES:
    r = raw(f"{method} {path}", ck[pfx], kind == "ws")
    res["cases"].append({"kind": kind, "prefix": pfx, "variant": label, "request": f"{method} {path}", **r})
    print(f'{r["status_line"][:30]:30} {r["ws_after_handshake"][:30]:30} {r["body_head"][:40]!r:44} {method} {path}')
OUT.write_text(json.dumps(res, indent=2))
