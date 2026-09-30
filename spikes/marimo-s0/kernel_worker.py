"""A single-use, pre-started kernel process (S0b variant A)."""

import json
import os
import select
import signal
import socket
import sys
import threading
import traceback

import timing

# Pay the import cost before any viewer arrives.
import marimo._ipc.launch_kernel as launch_kernel  # noqa: E402
import marimo._runtime.runtime  # noqa: E402,F401
import httpx  # noqa: E402,F401

# polars (and egret_marimo, which imports polars) are imported in main(), not here: the fork server
# (variant B) imports this module for serve_one and must be able to leave polars out (--preload light).

# The shim client adds this key to the env payload; the worker pops it and chdirs there, so the
# kernel runs in the launcher's cwd exactly as `python -m marimo._ipc.launch_kernel` would.
CWD_KEY = "__S0_SHIM_CWD__"


def _die_with_client(conn: socket.socket) -> None:
    """Kill-on-disconnect: once claimed, this process lives only as long as its shim client.

    The client never sends anything after the env, so any return from recv (EOF, or an error
    when the peer was SIGKILLed) means the client is gone and so is marimo's handle on us.
    """
    try:
        while conn.recv(4096):
            pass
    except OSError:
        pass
    # After marimo's kernel bootstrap calls os.setsid() we lead our own group, so user-code
    # subprocesses die with us. Before that we share the pool's group — never killpg it.
    try:
        if os.getpgid(0) == os.getpid():
            os.killpg(os.getpid(), signal.SIGKILL)
    finally:
        os._exit(137)


def serve_one(conn: socket.socket) -> None:
    msg, fds, _, _ = socket.recv_fds(conn, 65536, 3)
    if len(fds) != 3:
        print(f"worker {os.getpid()}: expected 3 fds, got {len(fds)} with {len(msg)} bytes", file=sys.stderr, flush=True)
        os._exit(70)
    env = json.loads(msg.decode())
    cwd = env.pop(CWD_KEY, None)
    os.environ.clear()
    os.environ.update(env)
    if cwd:
        os.chdir(cwd)
    for target, fd in zip((0, 1, 2), fds):
        os.dup2(fd, target)
        os.close(fd)
    sys.stdin = open(0, "r", closefd=False)
    sys.stdout = open(1, "w", closefd=False, buffering=1)
    sys.stderr = open(2, "w", closefd=False, buffering=1)
    conn.sendall(f"PID {os.getpid()}\n".encode())
    threading.Thread(target=_die_with_client, args=(conn,), daemon=True).start()
    timing.mark("worker_claimed")
    code = 0
    try:
        launch_kernel.main()
    except SystemExit as e:
        code = e.code if isinstance(e.code, int) else (0 if e.code is None else 1)
    except BaseException:
        traceback.print_exc()  # os._exit below would otherwise swallow it
        code = 1
    finally:
        try:
            conn.sendall(f"EXIT {code}\n".encode())
        finally:
            os._exit(code)


def main() -> None:
    import polars  # noqa: F401  variant A: still paid before WORKER_WARM, as before
    import egret_marimo  # noqa: F401

    channel = socket.socket(fileno=int(sys.argv[1]))  # inherited from the pool
    listener = socket.socket(fileno=int(sys.argv[2]))  # the pool's listening socket, inherited at spawn
    print(f"WORKER_WARM {os.getpid()}", file=sys.stderr, flush=True)
    timing.mark("worker_warm")
    try:
        channel.sendall(b"W")  # the pool lists us as idle only after this byte
        armed = channel.recv(1) == b"g"
    except OSError:
        armed = False
    if not armed:  # the pool went away before arming us
        os._exit(0)
    # armed: we are the only worker that accepts right now; if the pool dies meanwhile (channel
    # EOF), exit rather than wait forever on a listener that other workers still hold open
    readable, _, _ = select.select([listener, channel], [], [])
    if channel in readable:
        os._exit(0)
    conn, _ = listener.accept()
    listener.close()  # the kernel (and its children) must not keep the pool's listener
    channel.sendall(b"A")  # the pool may arm the next worker
    channel.close()
    serve_one(conn)


if __name__ == "__main__":
    main()
