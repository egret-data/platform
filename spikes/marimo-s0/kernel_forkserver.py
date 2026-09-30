"""Fork server: preload once, fork a fresh child per viewer kernel (S0b variant B).

The server itself never runs viewer code and never reads the shim's env payload (which carries
the session's environment): it only accepts and forks. The forked child owns the connection and
runs `kernel_worker.serve_one` — the same code as variant A, incl. kill-on-disconnect and cwd —
serves exactly one kernel, and exits. The server stays single-threaded so fork() copies no
half-held locks; it notices its parent (the marimo server) going away by polling in select().
"""

import argparse
import importlib
import os
import select
import signal
import socket
import sys
import threading

PRELOAD = {
    # egret_marimo imports polars at module level, so `light` leaves it out too (the child imports
    # it when the app's first cell runs). `full` preloads both.
    "light": ["marimo._ipc.launch_kernel", "marimo._runtime.runtime", "httpx"],
    "full": ["marimo._ipc.launch_kernel", "marimo._runtime.runtime", "httpx", "egret_marimo", "polars"],
}


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--socket", required=True)
    ap.add_argument("--preload", choices=list(PRELOAD), default="light")
    a = ap.parse_args()
    for name in PRELOAD[a.preload]:
        importlib.import_module(name)
    import kernel_worker  # serve_one; its module-level imports are a subset of `light`
    import timing

    signal.signal(signal.SIGCHLD, signal.SIG_IGN)  # auto-reap children: no zombies
    if os.path.exists(a.socket):
        os.unlink(a.socket)
    srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    srv.bind(a.socket)
    os.chmod(a.socket, 0o600)
    srv.listen(16)
    parent = os.getppid()
    print(f"FORKSERVER_READY {a.preload} pid={os.getpid()} py_threads={threading.active_count()} "
          f"polars_loaded={'polars' in sys.modules}", flush=True)
    while True:
        readable, _, _ = select.select([srv], [], [], 1.0)
        if os.getppid() != parent:  # the marimo server went away; children watch their own clients
            os._exit(0)
        if not readable:
            continue
        conn, _ = srv.accept()
        pid = os.fork()
        if pid == 0:
            srv.close()
            signal.signal(signal.SIGCHLD, signal.SIG_DFL)  # the kernel's own subprocesses need wait()
            try:
                kernel_worker.serve_one(conn)  # exits the process itself
            finally:
                os._exit(70)  # an exception before serve_one's own exit must not return to the accept loop
        conn.close()
        timing.mark("forked", child=pid)


if __name__ == "__main__":
    main()
