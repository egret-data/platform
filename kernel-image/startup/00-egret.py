# Auto-loaded by IPython at kernel start — makes the egret helpers global.
try:
    from egret_kernel_helpers import display, runtime, me, trino, starrocks, write_table, write_iceberg, read_iceberg  # noqa: F401
except Exception as _e:  # noqa: BLE001
    print(f"egret helpers not loaded: {_e}")

# Register the %%egret_sql cell magic (SQL cell → bound Daft DataFrame).
try:
    from egret_kernel_helpers.magic import register as _register_egret_sql
    _register_egret_sql(get_ipython())
except Exception as _e:  # noqa: BLE001
    print(f"egret sql magic not loaded: {_e}")
