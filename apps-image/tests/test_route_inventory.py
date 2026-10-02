"""The proxy's allow-list must name every route marimo serves.

`crates/egret-apps/src/proxy/route_inventory.json` is what the proxy looks routes up in, and
a route absent from it is DENIED. So a marimo upgrade that adds a route cannot widen what the
proxy forwards — but it silently breaks an editor feature if the new route is one the editor
needs. This test is where an upgrade finds out: regenerate with
`tools/gen_route_inventory.py`, read every route it reports as unclassified, and classify it.
"""

import json
from pathlib import Path

from marimo._server.api.router import build_routes

INVENTORY = Path(__file__).resolve().parents[2] / "crates/egret-apps/src/proxy/route_inventory.json"


def _walk(routes, prefix=""):
    for r in routes:
        path = prefix + getattr(r, "path", "")
        children = getattr(r, "routes", None)
        if children:
            yield from _walk(children, path)
        else:
            for m in sorted(getattr(r, "methods", None) or ["WEBSOCKET"]):
                if m != "HEAD":
                    yield m, path


def test_the_committed_inventory_is_marimos_live_route_set():
    live = set(_walk(build_routes("")))
    committed = {(e["method"], e["path"]) for e in json.loads(INVENTORY.read_text())}
    assert live - committed == set(), (
        "marimo serves routes the proxy inventory does not name (they are denied until "
        f"classified): {sorted(live - committed)}"
    )
    assert committed - live == set(), f"inventory names routes marimo no longer has: {sorted(committed - live)}"


def test_nothing_marimo_added_is_left_unclassified_in_the_inventory():
    unclassified = [
        (e["method"], e["path"])
        for e in json.loads(INVENTORY.read_text())
        if e["surface"] == "deny" and e["why"].startswith("not classified")
    ]
    assert unclassified == []
