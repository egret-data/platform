"""catalog() — a pyiceberg RestCatalog for the user's private namespace (read/write)."""
import os

from .runtime import load_runtime


def catalog(id=None, *, env=None):
    """Return a pyiceberg RestCatalog bound to the workspace warehouse with the
    user's 1-grant token. The user's private namespace is `notebook_{username}`
    (available as `.namespace` on the runtime catalog)."""
    rt = load_runtime(os.environ if env is None else env)
    if rt is None:
        raise RuntimeError("egret: no notebook runtime configured")
    c = rt.catalog
    if not c.url or not c.token:
        raise RuntimeError("egret: workspace catalog not configured (no warehouse/token)")
    from pyiceberg.catalog.rest import RestCatalog
    return RestCatalog("egret", uri=c.url, warehouse=c.warehouse, token=c.token)
