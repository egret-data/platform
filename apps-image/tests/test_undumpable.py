"""The server must not be readable by the kernels it spawns (same uid, so file permissions alone
would let a cell read `/proc/<server>/environ` and `/proc/<server>/mem`, where the pod token and the
marimo token live after the launcher drops them from the environment).

`prctl(PR_SET_DUMPABLE, 0)` makes those files root-owned for everyone else of the same uid. The
kernels are fresh `exec`s, so they are dumpable again: this protects the server from them, NOT
one kernel from another. Kernels of different viewers sharing a run pod need separate uids or a
blocked ptrace (P5); an edit pod has a single author, whose cells already run arbitrary Python.
"""

import subprocess
import sys
from pathlib import Path

import pytest

PROBE = Path(__file__).parent / "support" / "undumpable_probe.py"
linux_only = pytest.mark.skipif(not sys.platform.startswith("linux"), reason="/proc and prctl are Linux")


def _probe(mode: str) -> dict[str, str]:
    out = subprocess.run([sys.executable, str(PROBE), mode], capture_output=True, text=True, check=True, timeout=60)
    return dict(line.split() for line in out.stdout.splitlines())


@linux_only
def test_without_the_protection_a_same_uid_child_reads_the_servers_environment_and_memory():
    # The control: if this ever stops being true the next test proves nothing.
    assert _probe("control") == {"environ": "READABLE", "mem": "READABLE"}


@linux_only
def test_after_make_undumpable_the_same_uid_child_is_denied():
    assert _probe("protect") == {"environ": "DENIED", "mem": "DENIED"}


def test_make_undumpable_is_a_harmless_no_op_off_linux(monkeypatch):
    from egret_marimo import launcher

    monkeypatch.setattr(sys, "platform", "darwin")
    assert launcher.make_undumpable() is False
