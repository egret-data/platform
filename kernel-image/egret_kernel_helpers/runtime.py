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


def load_runtime(env=None):
    """Return a Runtime from EGRET_NOTEBOOK_RUNTIME, or None if unset."""
    env = os.environ if env is None else env
    raw = env.get("EGRET_NOTEBOOK_RUNTIME")
    if not raw:
        return None
    return Runtime(json.loads(raw))
