"""Egret notebook kernel helpers — auto-loaded into every kernel.

Exposes query(), catalog(), display(), plus `runtime` (parsed manifest) and
`me` (your username). Designed to import cleanly even with no manifest (a bare
kernel) — the helpers raise a clear error only when actually called.
"""
from .catalog import catalog
from .display import display
from .magic import load_ipython_extension, run_egret_sql
from .query import query
from .runtime import load_runtime

runtime = load_runtime()
me = runtime.username if runtime is not None else None

__all__ = ["query", "catalog", "display", "load_runtime", "runtime", "me",
           "load_ipython_extension", "run_egret_sql"]
