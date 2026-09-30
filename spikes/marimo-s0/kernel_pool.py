"""Warm pool: N single-use workers; each shim connection consumes one (S0b variant A).

The pool never accepts. Every worker inherits the listening socket at spawn (plain fork
inheritance) and the pool "arms" exactly one warm worker at a time: the armed worker accepts one
connection, closes its copy of the listener, reports b"A", and the pool arms the next. The brief's
design (pool accepts, passes the connection to the worker with SCM_RIGHTS) lost 1 in 4 to 1 in 3
hand-offs on macOS under marimo: the worker's first recv on the passed socket returned 0 bytes and
the shim saw EOF (README, S0b T3). Here no socket is ever in flight; only the client's stdio pipes.
"""

import argparse
import os
import socket
import subprocess
import sys
import threading
import time
from pathlib import Path

HERE = Path(__file__).parent


class Pool:
    def __init__(self, size: int, listener: socket.socket, log_path: Path) -> None:
        self.size = size
        self.listener = listener
        self.log_path = log_path
        self.idle: list[tuple[subprocess.Popen, socket.socket]] = []
        self.everyone: list[subprocess.Popen] = []
        self.lock = threading.Condition()

    def spawn(self) -> None:
        parent, child = socket.socketpair()
        fds = [child.fileno(), self.listener.fileno()]
        with open(self.log_path, "ab") as log:
            proc = subprocess.Popen(
                [sys.executable, str(HERE / "kernel_worker.py"), *map(str, fds)], pass_fds=fds, stderr=log,
            )
        child.close()
        with self.lock:
            self.everyone.append(proc)
        # Listed as idle only once warm (the worker sends b"W" after its imports), so a claim
        # never picks a worker that is still importing while a warm one exists.
        if parent.recv(1) != b"W":
            print(f"pool: worker {proc.pid} died before warm (rc={proc.wait()})", file=sys.stderr, flush=True)
            parent.close()
            return
        with self.lock:
            self.idle.append((proc, parent))
            self.lock.notify_all()

    def wait_warm(self, n: int) -> None:
        with self.lock:
            while len(self.idle) < n:
                self.lock.wait()

    def claim(self) -> tuple[subprocess.Popen, socket.socket]:
        with self.lock:
            while not self.idle:
                self.lock.wait()
            worker = self.idle.pop(0)
        threading.Thread(target=self.spawn, daemon=True).start()  # refill; never reuse
        threading.Thread(target=self.reap, args=(worker[0],), daemon=True).start()
        return worker

    def reap(self, proc: subprocess.Popen) -> None:
        rc = proc.wait()  # no zombies: a claimed pid must really be gone once its kernel exits
        print(f"pool: claimed worker {proc.pid} exited rc={rc}", file=sys.stderr, flush=True)

    def kill_all(self) -> None:
        with self.lock:
            for proc in self.everyone:
                if proc.poll() is None:
                    proc.kill()


def exit_with_parent(pool: Pool) -> None:
    """The pool is a child of the marimo server; when the server goes, so do the idle workers."""
    parent = os.getppid()
    while os.getppid() == parent:
        time.sleep(1)
    pool.kill_all()  # claimed workers also watch the server pid via marimo's parent poller
    os._exit(0)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--size", type=int, default=2)
    ap.add_argument("--socket", required=True)
    a = ap.parse_args()
    log_path = HERE / ".work" / "logs" / "workers.log"
    log_path.parent.mkdir(parents=True, exist_ok=True)
    if os.path.exists(a.socket):
        os.unlink(a.socket)
    srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    srv.bind(a.socket)
    os.chmod(a.socket, 0o600)
    srv.listen(16)  # connections wait in the backlog until the armed worker accepts
    pool = Pool(a.size, srv, log_path)
    threading.Thread(target=exit_with_parent, args=(pool,), daemon=True).start()
    for _ in range(a.size):
        threading.Thread(target=pool.spawn, daemon=True).start()
    pool.wait_warm(a.size)
    print("POOL_READY", flush=True)
    while True:
        proc, channel = pool.claim()
        channel.sendall(b"g")  # armed: this worker, and only it, accepts the next connection
        if channel.recv(1) != b"A":
            # died while armed; a pending client stays in the backlog for the next armed worker
            print(f"pool: armed worker {proc.pid} died before accepting", file=sys.stderr, flush=True)
        channel.close()


if __name__ == "__main__":
    main()
