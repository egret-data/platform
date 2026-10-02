"""Stands in for the server and one of its kernels (a child of the same user).

    python undumpable_probe.py control    # the child CAN read the parent's /proc/<pid>/environ and mem
    python undumpable_probe.py protect    # after make_undumpable() it cannot

Prints one line per file: `environ READABLE|DENIED`, `mem READABLE|DENIED`.
"""

import subprocess
import sys

from egret_marimo import launcher

if sys.argv[1] == "protect":
    assert launcher.make_undumpable(), "prctl(PR_SET_DUMPABLE) failed"

CHILD = """
import os
ppid = os.getppid()
for name in ("environ", "mem"):
    # Opening is the permission check; reading `mem` at offset 0 fails with EIO whoever you are.
    try:
        open(f"/proc/{ppid}/{name}", "rb").close()
        print(name, "READABLE")
    except PermissionError:
        print(name, "DENIED")
"""
subprocess.run([sys.executable, "-c", CHILD], check=True)
