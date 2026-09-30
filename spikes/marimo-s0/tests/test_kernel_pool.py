import json
import os
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).parent.parent


def alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    return True


def wait_for(predicate, timeout: float, step: float = 0.1):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(step)
    return None


def claimed_pid(log: Path):
    if not log.exists():
        return None
    for line in log.read_text().splitlines():
        mark = json.loads(line)
        if mark["event"] == "worker_claimed":
            return mark["pid"]
    return None


def cwd_of(pid: int) -> str:
    out = subprocess.run(["lsof", "-a", "-p", str(pid), "-d", "cwd", "-Fn"],
                         capture_output=True, text=True, timeout=30).stdout
    return next(line[1:] for line in out.splitlines() if line.startswith("n"))


def test_claimed_worker_dies_with_its_sigkilled_client_and_runs_in_its_cwd():
    # Arrange: a pool of one warm worker, and a shim client whose kernel stays blocked
    # reading KernelArgs from stdin (we never close the client's stdin).
    with tempfile.TemporaryDirectory() as d:
        d = os.path.realpath(d)
        sock, log = f"{d}/k.sock", Path(d) / "timing.jsonl"
        client_cwd = Path(d) / "client-cwd"
        client_cwd.mkdir()
        env = {**os.environ, "S0_TIMING_LOG": str(log)}
        pool = subprocess.Popen([sys.executable, str(HERE / "kernel_pool.py"), "--size", "1", "--socket", sock],
                                stdout=subprocess.PIPE, env=env, text=True)
        try:
            assert pool.stdout.readline().strip() == "POOL_READY"
            client = subprocess.Popen([sys.executable, str(HERE / "shim_client.py")],
                                      stdin=subprocess.PIPE, stdout=subprocess.PIPE, cwd=client_cwd,
                                      env={**env, "S0_KERNEL_SOCKET": sock})
            pid = wait_for(lambda: claimed_pid(log), timeout=30)
            assert pid, "no worker_claimed mark"
            assert pid != client.pid
            assert cwd_of(pid) == str(client_cwd)  # the worker adopted the launcher's cwd

            # Act: the client dies without a chance to forward anything.
            client.send_signal(signal.SIGKILL)
            client.wait(timeout=10)

            # Assert: the claimed worker (and so its kernel) is gone within 5 s.
            assert wait_for(lambda: not alive(pid), timeout=5), f"worker {pid} outlived its client"
            assert pool.poll() is None  # the pool itself survives
        finally:
            pool.terminate()
            pool.wait(timeout=10)
