import os
import signal
import subprocess
import sys
import tempfile
from pathlib import Path

from test_kernel_pool import HERE, alive, claimed_pid, cwd_of, wait_for


def ppid_of(pid: int) -> int:
    return int(subprocess.run(["ps", "-o", "ppid=", "-p", str(pid)], capture_output=True, text=True,
                              timeout=30).stdout.strip() or 0)


def test_forked_child_dies_with_its_sigkilled_client_and_runs_in_its_cwd():
    # Arrange: a light fork server, and a shim client whose kernel stays blocked reading
    # KernelArgs from stdin (we never close the client's stdin).
    with tempfile.TemporaryDirectory() as d:
        d = os.path.realpath(d)
        sock, log = f"{d}/k.sock", Path(d) / "timing.jsonl"
        client_cwd = Path(d) / "client-cwd"
        client_cwd.mkdir()
        env = {**os.environ, "S0_TIMING_LOG": str(log)}
        server = subprocess.Popen([sys.executable, str(HERE / "kernel_forkserver.py"), "--socket", sock,
                                   "--preload", "light"], stdout=subprocess.PIPE, env=env, text=True)
        try:
            ready = server.stdout.readline()
            assert ready.startswith("FORKSERVER_READY light"), ready
            assert "polars_loaded=False" in ready  # light really leaves polars out
            client = subprocess.Popen([sys.executable, str(HERE / "shim_client.py")],
                                      stdin=subprocess.PIPE, stdout=subprocess.PIPE, cwd=client_cwd,
                                      env={**env, "S0_KERNEL_SOCKET": sock})
            pid = wait_for(lambda: claimed_pid(log), timeout=30)
            assert pid, "no worker_claimed mark"
            assert pid not in (client.pid, server.pid)  # the kernel runs in a forked child, not the server
            assert ppid_of(pid) == server.pid
            assert cwd_of(pid) == str(client_cwd)  # the child adopted the launcher's cwd

            # Act: the client dies without a chance to forward anything.
            client.send_signal(signal.SIGKILL)
            client.wait(timeout=10)

            # Assert: the forked child (and so its kernel) is gone within 5 s — not even a zombie,
            # since the server ignores SIGCHLD — and the fork server survives.
            assert wait_for(lambda: not alive(pid), timeout=5), f"child {pid} outlived its client"
            assert server.poll() is None
        finally:
            server.terminate()
            server.wait(timeout=10)
