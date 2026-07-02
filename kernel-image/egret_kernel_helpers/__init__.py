"""Egret notebook kernel helpers — auto-loaded into every kernel.

Exposes trino()/starrocks()/write_table(), display(), plus `runtime`
(parsed manifest) and `me` (your username). Designed to import cleanly even
with no manifest (a bare kernel) — the helpers raise a clear error only when
actually called.
"""
from .display import display
from .engines import starrocks, trino, write_table  # noqa: F401
from .runtime import load_runtime

runtime = load_runtime()
me = runtime.username if runtime is not None else None

__all__ = ["display", "load_runtime", "runtime", "me", "trino", "starrocks", "write_table"]
