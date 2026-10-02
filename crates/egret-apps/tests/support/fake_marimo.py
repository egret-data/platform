"""A stand-in for `egret-apps edit`: what the subprocess launcher starts in tests.

Honours the launcher's environment contract (`EGRET_MARIMO_PORT`, `EGRET_BASE_URL`,
`EGRET_MARIMO_TOKEN`, `APPS_WORKDIR`), dumps the environment it was given to
`$APPS_WORKDIR/env.json`, and serves `GET <base>/health` behind the marimo token.

A launcher that fails or hangs is simulated with `python -c` one-liners in the tests, not with
modes here: the launcher clears the environment, so nothing could select a mode through it.
"""

import http.server
import json
import os

workdir = os.environ["APPS_WORKDIR"]
os.makedirs(workdir, exist_ok=True)
with open(os.path.join(workdir, "env.json"), "w") as f:
    json.dump(dict(os.environ), f)

port = int(os.environ["EGRET_MARIMO_PORT"])
base = os.environ["EGRET_BASE_URL"]
token = os.environ["EGRET_MARIMO_TOKEN"]


class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        ok = self.headers.get("authorization") == f"Bearer {token}" and self.path == f"{base}/health"
        self.send_response(200 if ok else 401)
        self.end_headers()
        self.wfile.write(b"ok" if ok else b"no")

    def log_message(self, *a):
        pass


http.server.ThreadingHTTPServer(("127.0.0.1", port), H).serve_forever()
