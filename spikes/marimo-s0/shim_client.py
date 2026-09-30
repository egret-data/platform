"""Launcher shim: hand this process's stdio to a pre-started kernel (spike S0b)."""

import json
import os
import signal
import socket
import sys


def main() -> int:
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect(os.environ["S0_KERNEL_SOCKET"])
    env = json.dumps(dict(os.environ)).encode()
    if len(env) > 65536:
        print("shim: environment too large", file=sys.stderr)
        return 70
    socket.send_fds(sock, [env], [0, 1, 2])
    # our copies of stdin/stdout stay open until exit; the kernel owns the conversation now
    reader = sock.makefile("rb")
    first = reader.readline().decode().split()
    if len(first) != 2 or first[0] != "PID":
        print(f"shim: bad reply {first!r}", file=sys.stderr)
        return 71
    kernel_pid = int(first[1])

    def forward(signum, _frame):
        try:
            os.kill(kernel_pid, signum)
        except ProcessLookupError:
            pass

    for s in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
        signal.signal(s, forward)
    last = reader.readline().decode().split()
    return int(last[1]) if len(last) == 2 and last[0] == "EXIT" else 72


if __name__ == "__main__":
    sys.exit(main())
