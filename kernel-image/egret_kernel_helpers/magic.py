"""`%%egret_sql --target daft|<engine>` cell magic.

daft   -> daft.sql(cell) over the attached Iceberg catalog.
<engine> -> the governed query() helper (egret callback -> your identity via F2).
The SQL body is passed as a Python value, never string-built, so no escaping issues.
"""
import argparse
import shlex

from .query import query


def _parse_target(line: str) -> str:
    p = argparse.ArgumentParser(prog="%%egret_sql", add_help=False)
    p.add_argument("--target", default="daft")
    args, _ = p.parse_known_args(shlex.split(line))
    return args.target


def run_egret_sql(line: str, cell: str):
    """Core (unit-testable): route the cell SQL to Daft or a governed engine."""
    target = _parse_target(line)
    if target == "daft":
        import daft
        return daft.sql(cell)
    return query(target, cell)


def load_ipython_extension(ipython):
    """Register the %%egret_sql cell magic on the running IPython kernel."""
    from IPython.core.magic import register_cell_magic

    @register_cell_magic("egret_sql")
    def _egret_sql(line, cell):
        return run_egret_sql(line, cell)
