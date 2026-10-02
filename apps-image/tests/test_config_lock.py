"""What a notebook's own PEP 723 header can and cannot change in the pod's marimo configuration.

An author controls the notebook source, so the header is attacker-controlled input to marimo's
config layers. Measured against marimo 0.25.0 (this test pins it, so an upgrade that changes the
picture fails here first):

* marimo ITSELF ignores `tool.marimo.server`, `tool.marimo.ai`, `runtime.auto_instantiate` and
  `display.custom_css` from script metadata "for security reasons";
* the `SecurityConfigManager` layer, which marimo merges LAST, is where egret_marimo locks `ai`,
  `experimental.rtc_v2` and `sharing`;
* what remains settable from a header — `runtime.dotenv`, `runtime.pythonpath`,
  `runtime.on_cell_change`, `runtime.auto_reload`, `package_management`, `formatting` — gives the
  author nothing a cell does not already have: a cell is arbitrary Python running as the pod's
  user, so it can read any file that user can read and extend `sys.path` itself. Those keys are
  therefore NOT locked; the boundary against hostile cell code is the pod (no engine credential,
  network policy, read-only root, tokens kept out of the kernel's reach), not this file.
"""

import pathlib

import pytest

from egret_marimo import patches

HOSTILE_HEADER = """# /// script
# requires-python = ">=3.12"
# [tool.marimo.runtime]
# dotenv = ["/etc/passwd"]
# pythonpath = ["/tmp/planted"]
# auto_instantiate = true
# [tool.marimo.server]
# follow_symlink = true
# [tool.marimo.ai]
# enabled = true
# [tool.marimo.experimental]
# rtc_v2 = true
# [tool.marimo.display]
# custom_css = ["/etc/hosts"]
# [tool.marimo.sharing]
# html = true
# wasm = true
# ///
import marimo
app = marimo.App()
"""


@pytest.fixture
def effective(tmp_path: pathlib.Path):
    nb = tmp_path / "nb.py"
    nb.write_text(HOSTILE_HEADER)
    from marimo._config.manager import SecurityConfigManager, get_default_config_manager

    original = SecurityConfigManager.get_config
    patches.install_config_lock()
    try:
        yield get_default_config_manager(current_path=str(nb)).get_config(hide_secrets=False)
    finally:
        SecurityConfigManager.get_config = original
        if hasattr(SecurityConfigManager, "_egret_original_get_config"):
            del SecurityConfigManager._egret_original_get_config


def test_a_header_cannot_turn_on_ai_or_rtc_or_sharing(effective):
    assert effective["ai"]["enabled"] is False
    assert effective["experimental"]["rtc_v2"] is False
    from marimo._config.config import SharingConfig

    for key in SharingConfig.__annotations__:
        assert effective["sharing"][key] is False, f"sharing.{key} is on"


def test_marimo_itself_ignores_the_server_and_instantiate_keys_in_a_header(effective):
    assert effective["server"]["follow_symlink"] is False
    assert effective["runtime"]["auto_instantiate"] is False
    assert "custom_css" not in effective.get("display", {}) or effective["display"]["custom_css"] in ([], None)


def test_the_keys_left_to_the_author_are_exactly_the_ones_that_grant_nothing_new(effective):
    # Documented, not locked: see the module docstring. If marimo starts honouring more from a
    # header, the assertions above fail; if this one fails, read what changed before accepting it.
    assert effective["runtime"]["dotenv"] == ["/etc/passwd"]
    assert effective["runtime"]["pythonpath"] == ["/tmp/planted"]
