"""display(obj) — render Daft DataFrames (capped); fall through to IPython otherwise."""


def display(obj, n=50):
    from IPython.display import display as _ipy
    try:
        import daft
        if isinstance(obj, daft.DataFrame):
            _ipy(obj.limit(n))
            return
    except Exception:  # noqa: BLE001 — daft missing / not a DataFrame → fall through
        pass
    _ipy(obj)
