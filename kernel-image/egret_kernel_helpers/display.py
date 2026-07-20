"""display(obj) — render Daft DataFrames as a clean egret table; fall through to IPython otherwise."""

# MIME the egret frontend renders with its styled DataFrameTable component.
_TABLE_MIME = "application/vnd.egret.table+json"


def _jsonable(v):
    """Coerce a cell value to something JSON-serializable for the MIME bundle.
    Primitives pass through; everything else (dates, decimals, nested, bytes) is
    stringified so the frontend always receives valid JSON."""
    if v is None or isinstance(v, (bool, int, float, str)):
        return v
    return str(v)


def _table_bundle(df, n):
    """Build `{columns:[{name,type}], rows:[[...]], truncated}` from a Daft frame.
    Materializes only the first `n` rows (capped) — no full-table compute."""
    capped = df.limit(n).collect()
    pyd = capped.to_pydict()
    names = list(pyd.keys())
    try:
        dtypes = {f.name: str(f.dtype) for f in capped.schema()}
    except Exception:  # noqa: BLE001 — schema API drift → just omit dtypes
        dtypes = {}
    columns = [{"name": nm, "type": dtypes.get(nm, "")} for nm in names]
    cols = [pyd[nm] for nm in names]
    rows = [[_jsonable(v) for v in r] for r in zip(*cols)] if names else []
    return {"columns": columns, "rows": rows, "truncated": len(rows) >= n}


def display(obj, n=50):
    from IPython.display import display as _ipy
    try:
        import daft
        if isinstance(obj, daft.DataFrame):
            bundle = _table_bundle(obj, n)
            ncol, nrow = len(bundle["columns"]), len(bundle["rows"])
            fallback = f"<egret DataFrame table: {nrow} rows x {ncol} cols>"
            # raw=True: hand IPython the MIME bundle directly (the frontend renders
            # the table+json; the text/plain is a fallback for other frontends).
            _ipy({_TABLE_MIME: bundle, "text/plain": fallback}, raw=True)
            return
    except Exception:  # noqa: BLE001 — daft missing / not a DataFrame → fall through
        pass
    _ipy(obj)
