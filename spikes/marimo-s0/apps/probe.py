import marimo

__generated_with = "0.25.0"
app = marimo.App()


@app.cell
def _():
    import os

    import marimo as mo

    import egret_marimo as egret

    return egret, mo, os


@app.cell
def _(egret):
    db = egret.connect("trino_prod")
    return (db,)


@app.cell
def _(mo):
    n = mo.ui.slider(1, 10, value=1, label="n")
    n
    return (n,)


@app.cell
def _(egret, mo, n, os):
    py_df = egret.sql(f"SELECT {n.value} AS n", database="trino_prod")
    mo.md(
        f"PY pid={os.getpid()} viewer={py_df['viewer'][0]} sql={py_df['sql'][0]}"
    )
    return


@app.cell
def _(db, mo, n):
    sql_df = mo.sql(f"SELECT {n.value} AS via_sql_cell", engine=db)
    mo.md(f"SQLCELL viewer={sql_df['viewer'][0]}")
    return


if __name__ == "__main__":
    app.run()
