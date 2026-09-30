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
