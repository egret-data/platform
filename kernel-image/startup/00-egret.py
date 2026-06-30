# Auto-loaded by IPython at kernel start — makes the egret helpers global.
try:
    from egret_kernel_helpers import query, catalog, display, runtime, me  # noqa: F401
except Exception as _e:  # noqa: BLE001
    print(f"egret helpers not loaded: {_e}")
