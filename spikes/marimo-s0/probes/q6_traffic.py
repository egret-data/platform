"""Q6: cross-check paths seen in proxy traffic against build_routes, then write results/q6.json."""

import json
import pathlib
import re

ROOT = pathlib.Path(__file__).parent.parent
routes = json.loads((ROOT / "results/q6-routes.json").read_text())
log = (ROOT / ".work/logs/proxy.log").read_text()
seen = {}
for m in re.finditer(r'"(GET|POST|HEAD|PUT|DELETE|OPTIONS) (/[er])(/[^ ?]*)[^ ]* HTTP/1.1" (\d+)|"WebSocket (/[er])(/[^ ?]*)', log):
    path = m.group(3) or m.group(6)
    kind = m.group(1) or "WEBSOCKET"
    seen.setdefault((kind, path), set()).add(m.group(4) or "ws")


def pattern(p):
    return re.compile("^" + re.sub(r"\{[^}]*:path\}", ".+", re.sub(r"\{[^}:]*\}", "[^/]+", p)) + "$")


exact = [r for r in routes if "{" not in r["path"]]
tmpl = [r for r in routes if "{" in r["path"]]
not_in_routes = []
matched_by_catchall = []
for (kind, path), statuses in sorted(seen.items()):
    hit = next((r for r in exact if r["path"] == path), None)
    if hit:
        continue
    t = next((r for r in tmpl if pattern(r["path"]).match(path)), None)
    (matched_by_catchall if t and t["path"] == "/{path:path}" else not_in_routes).append(
        {"method": kind, "path": path, "statuses": sorted(statuses), "matched": t["path"] if t else None}
    )
out = {"seen_distinct": len(seen), "not_matched_by_any_route": [x for x in not_in_routes if not x["matched"]],
       "matched_only_by_template": not_in_routes,
       "matched_only_by_static_catchall_count": len(matched_by_catchall),
       "catchall_examples": matched_by_catchall[:5]}
print(json.dumps(out, indent=2))
(ROOT / "results/q6-traffic.json").write_text(json.dumps(out, indent=2))
