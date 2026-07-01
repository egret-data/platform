# Auto-loaded by IPython at kernel start — makes the egret helpers global.
try:
    from egret_kernel_helpers import query, catalog, display, runtime, me, iceberg, trino  # noqa: F401
except Exception as _e:  # noqa: BLE001
    print(f"egret helpers not loaded: {_e}")

try:
    from egret_kernel_helpers import load_ipython_extension as _load_egret
    _load_egret(get_ipython())  # noqa: F821 — get_ipython is global in an IPython kernel
except Exception as _e:  # noqa: BLE001
    print(f"egret %%egret_sql magic not loaded: {_e}")
