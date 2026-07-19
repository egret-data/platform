"""%%egret_sql cell magic — run governed SQL and bind the result as a Daft DataFrame.

    %%egret_sql --target <engine-id|kind> --into sales_df
    SELECT * FROM egret.sales WHERE region = 'US'

`--target` is the engine id chosen in the cell's Target dropdown (or a bare kind
like 'trino' for a default connection). We resolve it against the runtime
manifest to pick the right helper and pass the engine id through so the server
resolves the exact connection. The result Daft DataFrame is bound to `--into`
(default `df`) in the user namespace and displayed. The magic line is injected
by the frontend; users only ever see clean SQL.
"""
import re

_IDENT = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")


def make_dispatch(engines, kind_fns):
    """Build a `dispatch(target, sql) -> DataFrame` closure.

    `engines` = the parsed runtime manifest engines (objects with `.id`,
    `.name`, `.engine` kind). `kind_fns` maps a kind ("trino"/"starrocks") to
    the helper that runs SQL on it. `target` may be an engine id, name, or a
    bare kind. A manifest match calls the kind's helper with `engine=<id>` so
    the server resolves the exact connection; otherwise `target` is treated as
    a bare kind (default connection) for a bare kernel.
    """
    by_key = {}
    for e in engines or []:
        by_key[e.id] = e
        by_key[e.name] = e

    def dispatch(target, sql):
        e = by_key.get(target)
        if e is not None:
            fn = kind_fns.get(e.engine)
            if fn is None:
                raise ValueError(f"egret: engine kind {e.engine!r} is not runnable from notebooks")
            return fn(sql, engine=e.id)
        fn = kind_fns.get(target)
        if fn is None:
            raise ValueError(
                f"egret: unknown --target {target!r} — pick a data source from the cell's Target dropdown"
            )
        return fn(sql, engine=target)

    return dispatch


def run_egret_sql(sql, *, target, into, dispatch, user_ns, display_fn):
    """Testable core: validate the variable name, dispatch, bind, display."""
    if not _IDENT.match(into or ""):
        raise ValueError(f"egret: invalid --into variable name {into!r} (must be a Python identifier)")
    df = dispatch(target, sql)
    user_ns[into] = df
    display_fn(df)
    return df


def register(ipython):
    """Register the %%egret_sql cell magic on a live IPython shell."""
    from IPython.core.magic import Magics, magics_class, cell_magic
    from IPython.core.magic_arguments import argument, magic_arguments, parse_argstring
    from .engines import trino, starrocks
    from .display import display
    from .runtime import load_runtime

    kind_fns = {"trino": trino, "starrocks": starrocks}

    @magics_class
    class EgretSqlMagics(Magics):
        @cell_magic
        @magic_arguments()
        @argument("--target", default="trino", help="engine id or kind (trino|starrocks)")
        @argument("--into", default="df", help="variable to bind the result DataFrame to")
        def egret_sql(self, line, cell):
            args = parse_argstring(self.egret_sql, line)
            rt = load_runtime()
            engines = rt.engines if rt is not None else []
            dispatch = make_dispatch(engines, kind_fns)
            run_egret_sql(
                cell, target=args.target, into=args.into,
                dispatch=dispatch, user_ns=self.shell.user_ns, display_fn=display,
            )

    ipython.register_magics(EgretSqlMagics)
