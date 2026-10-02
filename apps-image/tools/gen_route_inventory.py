"""Regenerate `crates/egret-apps/src/proxy/route_inventory.json` from marimo's live routes.

    cd apps-image && .venv/bin/python tools/gen_route_inventory.py

The proxy's allow-list (`crates/egret-apps/src/proxy/policy.rs`) is a TABLE LOOKUP over that
file, and anything the table does not name is denied. So a marimo upgrade that adds a route
cannot widen what the proxy forwards; it can only make an editor feature stop working until
the route is classified here. `tests/test_route_inventory.py` fails when the live route set and
the committed file differ, which is how an upgrade finds out.

Classification comes from S0 (docs/en/plans/2026-09-30-marimo-s0-result.md, question 6), which
inventoried the 107 routes of marimo 0.25.0 and measured editing, saving, the 409 and run mode
with the 22 prefixes below blocked. What S0 did NOT measure is which routes the RUN page needs,
so the `run` surface is the minimum plausible set and is checked by recording traffic in P5.
"""

from __future__ import annotations

import json
import pathlib

from marimo._server.api.router import build_routes

ROOT = pathlib.Path(__file__).resolve().parents[2]
OUT = ROOT / "crates/egret-apps/src/proxy/route_inventory.json"

# prefix -> why it is blocked on both surfaces (S0 question 6).
DENY_PREFIXES = {
    "/terminal": "PTY shell as the pod user; never for authors",
    "/api/packages": "installs/removes packages in the shared pod environment",
    "/api/secrets": "reads/writes secret keys and .env files on the pod filesystem",
    "/api/files": "arbitrary file browse/read/create/move/delete/download in the workspace dir",
    "/api/ai": "AI chat/completion and MCP use pod-side provider keys; no egret policy over them",
    "/api/storage": "lists/downloads objects from storage backends configured on the pod",
    "/api/home": "workspace file listing, recent files, tutorial open (creates a file), other sessions' shutdown",
    "/api/kernel/copy": "creates a second notebook file on the pod, outside the draft store",
    "/api/kernel/rename": "renames the notebook file, detaching it from the draft binding",
    "/api/kernel/install_missing_packages": "installs packages from a cell's imports",
    "/api/kernel/save_user_config": "writes the shared marimo.toml for every editor on the pod",
    "/api/export/auto_export": "writes html/md/ipynb beside the notebook on the pod",
    "/api/export/requirements": "installs export dependencies",
    "/api/lsp": "starts language-server subprocesses",
    "/api/datasources": "pod-side DB connections and schema previews; data goes only through egret's gateway",
    "/api/cache": "reads/clears the pod's shared cache dir",
    "/api/environment": "leaks OS, binary and dependency versions",
    "/api/usage": "leaks host memory/CPU totals",
    "/api/sessions": "lists every session on the pod, including other viewers'",
    "/api/status": "server status and every user's connection counts",
    "/auth": "marimo's own login pages; the proxy injects the pod token",
    "/og": "serves notebook thumbnails from __marimo__/ on the pod",
}

BOTH = {
    ("GET", "/"): "the editor / run app shell",
    ("GET", "/@file/{filename_and_length:path}"): "virtual files from the kernel's registry (mo.image, mo.download outputs)",
    ("GET", "/public/{filepath:path}"): "files under <notebook dir>/public; the proxy pins X-Notebook-Id",
    ("GET", "/public-files-sw.js"): "service worker for /public",
    ("GET", "/health"): "liveness",
    ("GET", "/healthz"): "liveness",
    ("GET", "/api/version"): "version string",
    ("WEBSOCKET", "/ws"): "the live document/kernel channel",
    ("GET", "/sse"): "server-sent kernel messages",
    ("GET", "/{path:path}"): "the frontend bundle (static files)",
    ("WEBSOCKET", "/assets"): "the /assets static mount (listed as WEBSOCKET only because it has no method list)",
}

# Needed to RUN an app. Unmeasured: P5 records the run page's traffic and confirms.
RUN_KERNEL = {
    "instantiate", "set_ui_element_value", "set_model_value", "function_call",
    "takeover", "stdin", "interrupt", "status",
}
# Needed to EDIT; marimo's own `@requires("edit")` also refuses these in run mode.
EDIT_KERNEL = {
    "code_autocomplete", "delete", "execute", "focus_cell", "format", "pdb/breakpoints",
    "pdb/pm", "read_code", "restart_session", "run", "save", "save_app_config",
    "scratchpad/run", "set_cell_config", "shutdown",
}
EDIT_OTHER = {
    ("POST", "/api/document/transaction"): "edit the in-memory document",
    ("GET", "/api/documentation/snippets"): "static snippet text for the editor",
    ("POST", "/api/sql/validate"): "parses SQL with sqlglot; no connection made",
    ("WEBSOCKET", "/ws_sync"): "real-time collaboration channel; marimo RTC is locked off, kept open for edit only because S0 measured editing with it open",
    ("GET", "/api/export/availability"): "which export formats exist",
    ("POST", "/api/export/html"): "the notebook in another format, to the requester (shows code: edit only)",
    ("POST", "/api/export/ipynb"): "as above",
    ("POST", "/api/export/markdown"): "as above",
    ("POST", "/api/export/script"): "as above",
    ("POST", "/api/export/pdf"): "as above",
    ("POST", "/api/export/update_cell_outputs"): "as above",
}


def walk(routes, prefix=""):
    for r in routes:
        path = prefix + getattr(r, "path", "")
        children = getattr(r, "routes", None)
        if children:
            yield from walk(children, path)
        else:
            for m in sorted(getattr(r, "methods", None) or ["WEBSOCKET"]):
                if m != "HEAD":
                    yield m, path


def classify(method: str, path: str) -> tuple[str, str]:
    for prefix, why in DENY_PREFIXES.items():
        if path == prefix or path.startswith(prefix + "/"):
            return "deny", why
    if (method, path) in BOTH:
        return "both", BOTH[(method, path)]
    if (method, path) in EDIT_OTHER:
        return "edit", EDIT_OTHER[(method, path)]
    if path.startswith("/api/kernel/"):
        name = path[len("/api/kernel/"):]
        if name in RUN_KERNEL:
            return "run", "core run-page traffic (run surface unmeasured; confirmed in P5)"
        if name in EDIT_KERNEL:
            return "edit", "editing"
    return "deny", "not classified: a route marimo added since S0 stays blocked until someone reads it"


def main() -> None:
    entries = []
    for method, path in sorted(set(walk(build_routes(""))), key=lambda e: (e[1], e[0])):
        surface, why = classify(method, path)
        entries.append({"method": method, "path": path, "surface": surface, "why": why})
    OUT.write_text(json.dumps(entries, indent=2) + "\n")
    counts: dict[str, int] = {}
    for e in entries:
        counts[e["surface"]] = counts.get(e["surface"], 0) + 1
    print(f"wrote {OUT.relative_to(ROOT)}: {len(entries)} routes {counts}")


if __name__ == "__main__":
    main()
