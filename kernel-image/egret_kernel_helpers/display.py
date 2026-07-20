"""display(obj) — render Daft DataFrames (capped); fall through to IPython otherwise."""


def display(obj, n=50):
    from IPython.display import display as _ipy
    try:
        import daft
        if isinstance(obj, daft.DataFrame):
            # `.collect()` materializes the capped frame so its repr shows the
            # actual rows — a lazy `obj.limit(n)` renders only the schema plus
            # "(No data to display: Dataframe not materialized)".
            _ipy(obj.limit(n).collect())
            return
    except Exception:  # noqa: BLE001 — daft missing / not a DataFrame → fall through
        pass
    _ipy(obj)
