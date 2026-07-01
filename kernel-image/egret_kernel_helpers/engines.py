"""Dataframe-first engine helpers: iceberg(), trino() — direct Daft as-you."""
import datetime as _dt
import json
import os

_REFRESH_MARGIN = _dt.timedelta(seconds=30)


def _parse_expiry(s):
    # RFC3339; tolerate trailing "Z".
    return _dt.datetime.fromisoformat(s.replace("Z", "+00:00"))


def _now():
    return _dt.datetime.now(_dt.timezone.utc)


def _post(url, body, token):
    import urllib.error
    import urllib.request
    req = urllib.request.Request(
        url, data=body, method="POST",
        headers={"Authorization": "Bearer " + token, "Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            return json.loads(r.read().decode())
    except urllib.error.HTTPError as e:
        if e.code == 401:
            raise RuntimeError("egret: kernel token expired — restart the kernel") from None
        if e.code in (403, 404):
            raise RuntimeError("egret: engine not enabled for notebooks or not permitted") from None
        detail = e.read().decode()[:200]
        raise RuntimeError(f"egret: engine-token failed (HTTP {e.code}): {detail}") from None
    except Exception as e:  # noqa: BLE001 — surface a clear message, never silent-empty
        raise RuntimeError(f"egret: cannot reach the engine-token endpoint ({e})") from None


# Module-level cache: {engine: {jwt, expires_at, expires_at_raw, kind, host, port}}.
_CACHE: dict = {}


def _engine_token(engine, *, env=None, http=None, cache=None):
    """Fetch a fresh per-user JWT + engine connection coordinates from egret,
    caching until near expiry (the kernel is long-lived, the JWT is not)."""
    env = os.environ if env is None else env
    cache = _CACHE if cache is None else cache
    cached = cache.get(engine)
    if cached is not None and _parse_expiry(cached["expires_at_raw"]) - _REFRESH_MARGIN > _now():
        return cached
    api = env.get("EGRET_API_URL")
    token = env.get("EGRET_KERNEL_TOKEN")
    if not api or not token:
        raise RuntimeError(
            "egret: kernel not configured (missing EGRET_API_URL / EGRET_KERNEL_TOKEN)"
        )
    body = json.dumps({"engine": engine}).encode()
    resp = (http or _post)(api.rstrip("/") + "/api/kernel/engine-token", body, token)
    resp["expires_at_raw"] = resp["expires_at"]
    cache[engine] = resp
    return resp
