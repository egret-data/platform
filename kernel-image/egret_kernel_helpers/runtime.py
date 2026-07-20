"""Parse the EGRET_NOTEBOOK_RUNTIME manifest injected by egret (F1/D1b)."""
import json
import os


class Engine:
    def __init__(self, d):
        self.id = d["id"]
        self.name = d["name"]
        self.engine = d["engine"]
        self.enabled = d.get("enabled", True)


class Runtime:
    def __init__(self, d):
        self.username = d["username"]
        self.engines = [Engine(e) for e in d.get("engines", [])]
        # Catalog wiring (Sub-project C): present when the workspace has a
        # notebook warehouse configured. `catalog_jwt` is a short-lived
        # aud:"catalog" token scoped to `catalog_warehouse`; the kernel's
        # write_iceberg helper presents it to `catalog_uri`. Defaults keep the
        # class working against older manifests that omit these fields.
        self.catalog_jwt = d.get("catalog_jwt")
        self.catalog_uri = d.get("catalog_uri")
        self.catalog_warehouse = d.get("catalog_warehouse")
        self.namespace = d.get("namespace", [])


def load_runtime(env=None):
    """Return a Runtime from EGRET_NOTEBOOK_RUNTIME, or None if unset."""
    env = os.environ if env is None else env
    raw = env.get("EGRET_NOTEBOOK_RUNTIME")
    if not raw:
        return None
    return Runtime(json.loads(raw))
