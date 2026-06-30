"""query(engine, sql) — run governed SQL via the egret callback, as your identity."""
import json
import os

from .runtime import load_runtime


def query(engine, sql, max_rows=10000, *, env=None, http=None):
    """Run SQL on a workspace engine through egret (Trino/StarRocks + Ranger),
    returning a Daft DataFrame. `engine` is an engine id or name from the manifest."""
    env = os.environ if env is None else env
    api = env.get("EGRET_API_URL")
    token = env.get("EGRET_KERNEL_TOKEN")
    if not api or not token:
        raise RuntimeError(
            "egret: kernel not configured (missing EGRET_API_URL / EGRET_KERNEL_TOKEN)"
        )
    # Resolve engine name → id when the manifest is present (server re-validates).
    engine_ref = engine
    rt = load_runtime(env)
    if rt is not None:
        match = next((e for e in rt.engines if engine in (e.id, e.name)), None)
        if match is not None:
            engine_ref = match.id
    body = json.dumps({"engine": engine_ref, "sql": sql, "max_rows": max_rows}).encode()
    resp = (http or _post)(api.rstrip("/") + "/api/kernel/query", body, token)
    return _to_daft(resp)


def _to_daft(resp):
    import daft
    cols = [c["name"] for c in resp["columns"]]
    data = {c: [row[i] for row in resp["rows"]] for i, c in enumerate(cols)}
    return daft.from_pydict(data)


def _post(url, body, token):
    import urllib.error
    import urllib.request
    req = urllib.request.Request(
        url, data=body, method="POST",
        headers={"Authorization": "Bearer " + token, "Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            return json.loads(r.read().decode())
    except urllib.error.HTTPError as e:
        if e.code == 401:
            raise RuntimeError("egret: kernel token expired — restart the kernel") from None
        if e.code == 403:
            raise RuntimeError("egret: engine not enabled for notebooks or not permitted") from None
        detail = e.read().decode()[:200]
        raise RuntimeError(f"egret: query failed (HTTP {e.code}): {detail}") from None
    except Exception as e:  # noqa: BLE001 — surface a clear message, never silent-empty
        raise RuntimeError(f"egret: cannot reach the query endpoint ({e})") from None
