# Auto-loaded by IPython at kernel start — makes the egret helpers global.

# Silence Daft's per-stage execution progress bar (the "[00:00] In Memory Scan:
# … rows out" telemetry). It streams to the cell on every collect()/display() and
# buries the actual result; errors still surface as exceptions. Set before any
# daft import (the helper import below pulls daft) so daft reads it at init.
import os as _os  # noqa: E402
_os.environ.setdefault("DAFT_PROGRESS_BAR", "0")

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

# Interactive plotly → emit the application/vnd.plotly.v1+json MIME bundle so the
# egret frontend renders it with plotly.js. Without this, plotly defaults to a
# text/html renderer whose plotly.js <script> is stripped by the frontend's HTML
# sanitizer, breaking interactivity.
try:
    import plotly.io as _pio
    _pio.renderers.default = "plotly_mimetype"
except Exception as _e:  # noqa: BLE001
    print(f"plotly renderer not configured: {_e}")

# matplotlib/seaborn → inline backend so figures emit image/png (rendered by the
# notebook's <img>). ipykernel usually auto-inlines; set it explicitly to be safe
# under the Kernel Gateway.
try:
    import matplotlib
    matplotlib.use("module://matplotlib_inline.backend_inline")
except Exception as _e:  # noqa: BLE001
    print(f"matplotlib inline not configured: {_e}")
