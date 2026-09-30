import os
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

HERE = Path(__file__).parent.parent


def fake_server(path: str, behaviour: str):
    """Accept one client, take its fds, write through them, report PID/EXIT."""
    srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    srv.bind(path)
    srv.listen(1)
    seen = {"pid_sent": threading.Event()}

    def run():
        conn, _ = srv.accept()
        msg, fds, _, _ = socket.recv_fds(conn, 65536, 3)
        seen["env_has_marker"] = b"S0_MARKER" in msg
        stdin, stdout = os.fdopen(fds[0], "rb"), os.fdopen(fds[1], "wb")
        seen["stdin"] = stdin.read()
        stdout.write(b"KERNEL_READY\n")
        stdout.flush()
        child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"])
        conn.sendall(f"PID {child.pid}\n".encode())
        seen["pid_sent"].set()
        code = child.wait()
        seen["child_code"] = code
        conn.sendall(f"EXIT {7 if behaviour == 'exit7' else code}\n".encode())
        conn.close()
        srv.close()

    t = threading.Thread(target=run, daemon=True)
    t.start()
    return seen, t


def run_client(sock_path: str):
    env = {**os.environ, "S0_KERNEL_SOCKET": sock_path, "S0_MARKER": "1"}
    return subprocess.Popen(
        [sys.executable, str(HERE / "shim_client.py")],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, env=env,
    )


def test_fds_env_and_exit_code_are_relayed():
    with tempfile.TemporaryDirectory() as d:
        path = f"{d}/k.sock"
        seen, t = fake_server(path, "exit7")
        client = run_client(path)
        client.stdin.write(b'{"args": 1}')
        client.stdin.close()
        assert client.stdout.readline() == b"KERNEL_READY\n"
        assert seen["pid_sent"].wait(timeout=10)
        time.sleep(0.5)  # let the client read PID and install its signal handlers
        client.send_signal(signal.SIGTERM)  # must be forwarded to the fake kernel, not kill the client
        assert client.wait(timeout=10) == 7
        t.join(timeout=10)
        assert seen["stdin"] == b'{"args": 1}'
        assert seen["env_has_marker"]
        assert seen["child_code"] == -signal.SIGTERM  # SIGTERM reached the kernel pid


def test_shim_python_diverts_only_the_kernel_launch():
    shim = HERE / "shim" / "bin" / "python"
    env = {**os.environ, "S0_REAL_PYTHON": sys.executable}
    out = subprocess.run([str(shim), "-c", "print('passthrough')"], env=env,
                         capture_output=True, text=True, timeout=30)
    assert out.stdout.strip() == "passthrough"
    with tempfile.TemporaryDirectory() as d:
        path = f"{d}/k.sock"
        seen, t = fake_server(path, "exit7")
        env = {**env, "S0_KERNEL_SOCKET": path, "S0_MARKER": "1"}
        p = subprocess.Popen([str(shim), "-m", "marimo._ipc.launch_kernel"],
                             stdin=subprocess.PIPE, stdout=subprocess.PIPE, env=env)
        p.stdin.write(b"hi")
        p.stdin.close()
        assert p.stdout.readline() == b"KERNEL_READY\n"
        assert seen["pid_sent"].wait(timeout=10)
        time.sleep(0.5)
        p.send_signal(signal.SIGTERM)
        assert p.wait(timeout=10) == 7
        t.join(timeout=10)
        assert seen["stdin"] == b"hi"


def test_shim_python_answers_marimos_venv_check_only_when_asked():
    shim = HERE / "shim" / "bin" / "python"
    check = "import marimo, msgspec, zmq; print(marimo.__version__)"
    env = {**os.environ, "S0_REAL_PYTHON": sys.executable}
    real = subprocess.run([str(shim), "-c", check], env=env, capture_output=True, text=True, timeout=60)
    fast = subprocess.run([str(shim), "-c", check], env={**env, "S0_SHIM_SKIP_CHECK": "9.9.9"},
                          capture_output=True, text=True, timeout=60)
    other = subprocess.run([str(shim), "-c", "print('other')"], env={**env, "S0_SHIM_SKIP_CHECK": "9.9.9"},
                           capture_output=True, text=True, timeout=60)
    assert real.stdout.strip() == "0.25.0"  # default: the real interpreter answers
    assert fast.stdout.strip() == "9.9.9"
    assert other.stdout.strip() == "other"  # only that exact command is short-circuited
