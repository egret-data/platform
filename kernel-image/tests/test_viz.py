"""The kernel image ships the DA/DS viz + numeric stack. These tests are gated on
import availability — the local dev .venv may lack them (like sqlalchemy), but the
built kernel image installs them (Dockerfile), and the live e2e is the real check.
"""
import importlib.util

import pytest


def _have(mod):
    return importlib.util.find_spec(mod) is not None


VIZ_LIBS = ["plotly", "matplotlib", "seaborn", "numpy", "scipy", "sklearn"]


@pytest.mark.skipif(
    not all(_have(m) for m in VIZ_LIBS),
    reason="viz libs not installed in this venv (they are in the kernel image)",
)
def test_viz_libs_import():
    import matplotlib  # noqa: F401
    import numpy  # noqa: F401
    import plotly  # noqa: F401
    import scipy  # noqa: F401
    import seaborn  # noqa: F401
    import sklearn  # noqa: F401


@pytest.mark.skipif(not _have("plotly"), reason="plotly not installed")
def test_plotly_emits_mimetype_bundle():
    import plotly.express as px

    fig = px.bar(x=["a", "b"], y=[1, 2])
    bundle = fig._repr_mimebundle_()
    # Depending on the plotly version, _repr_mimebundle_ returns either the data
    # dict or a (data, metadata) tuple — normalize.
    data = bundle[0] if isinstance(bundle, tuple) else bundle
    assert "application/vnd.plotly.v1+json" in data


@pytest.mark.skipif(not _have("matplotlib"), reason="matplotlib not installed")
def test_matplotlib_inline_backend_configures():
    import matplotlib

    matplotlib.use("module://matplotlib_inline.backend_inline")
    import matplotlib.pyplot as plt

    fig, ax = plt.subplots()
    ax.plot([1, 2, 3])
    assert fig is not None
