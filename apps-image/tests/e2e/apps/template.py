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


if __name__ == "__main__":
    app.run()
