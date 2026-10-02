import marimo

__generated_with = "0.25.0"
app = marimo.App(width="medium")


@app.cell
def _():
    import marimo as mo
    import egret_marimo as egret
    return egret, mo


@app.cell
def _(egret):
    # Queries run with each viewer's own permissions.
    db = egret.connect("db")
    return (db,)


@app.cell
def _(mo):
    mo.md("# New data app")
    return


@app.cell
def _():
    import os
    return (os,)


@app.cell
def _(egret, mo, os):
    mo.md(f"pid={os.getpid()} viewer={egret.sql('SELECT 1', database='db')['viewer'][0]}")
    return


@app.cell
def _(mo):
    slider = mo.ui.slider(1, 10, value=1, label="n")
    slider
    return (slider,)


@app.cell
def _(egret, mo, os, slider):
    mo.md(
        f"slider={slider.value} pid={os.getpid()} "
        f"viewer={egret.sql(f'SELECT {slider.value}', database='db')['viewer'][0]}"
    )
    return


if __name__ == "__main__":
    app.run()
