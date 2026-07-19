"""%%egret_sql cell magic — run governed SQL and bind the result as a Daft DataFrame.

    %%egret_sql --target trino --into sales_df
    SELECT * FROM egret.sales WHERE region = 'US'

Runs the SQL via the existing governed helper (trino()/starrocks()), binds the
resulting Daft DataFrame to `sales_df` in the user namespace (so later Python
cells can reuse it), and displays it. The magic line is injected by the
frontend; users only ever see clean SQL.
"""
import re

_IDENT = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")


def run_egret_sql(sql, *, target, into, user_ns, engines, display_fn):
    """Testable core: validate, dispatch to the engine helper, bind, display."""
    if not _IDENT.match(into or ""):
        raise ValueError(f"egret: invalid --into variable name {into!r} (must be a Python identifier)")
    fn = engines.get(target)
    if fn is None:
        raise ValueError(f"egret: unknown --target {target!r} (expected one of {sorted(engines)})")
    df = fn(sql)
    user_ns[into] = df
    display_fn(df)
    return df


def register(ipython):
    """Register the %%egret_sql cell magic on a live IPython shell."""
    from IPython.core.magic import Magics, magics_class, cell_magic
    from IPython.core.magic_arguments import argument, magic_arguments, parse_argstring
    from .engines import trino, starrocks
    from .display import display

    engines = {"trino": trino, "starrocks": starrocks}

    @magics_class
    class EgretSqlMagics(Magics):
        @cell_magic
        @magic_arguments()
        @argument("--target", default="trino", help="engine: trino | starrocks")
        @argument("--into", default="df", help="variable to bind the result DataFrame to")
        def egret_sql(self, line, cell):
            args = parse_argstring(self.egret_sql, line)
            run_egret_sql(
                cell, target=args.target, into=args.into,
                user_ns=self.shell.user_ns, engines=engines, display_fn=display,
            )

    ipython.register_magics(EgretSqlMagics)
