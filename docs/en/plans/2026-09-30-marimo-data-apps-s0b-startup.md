# Data Apps S0b — kernel start-up latency Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Find where a new viewer's ~13 s goes in per-session-process run mode, and measure whether pre-started kernels bring it under 2 s on a warm pod without giving up per-viewer process isolation.

**Architecture:** This continues the throwaway S0 spike (`spikes/marimo-s0/`, branch `spike/marimo-s0`).
- **Measure first.** Timestamped events along the path show where the time goes: page → ticket → WebSocket → kernel spawn → imports → first cell → gateway query → rendered.
- **Then try two ways to pre-start kernels.** Both plug in at the one point marimo already expects a launcher between itself and the kernel: it runs `<venv python> -m marimo._ipc.launch_kernel` and reads `KERNEL_READY` / `KERNEL_INFO <pid>` back.
  - We point marimo's configured venv at a *shim venv*, whose `python` hands the launch to a **pre-started, single-use worker**.
  - The hand-over passes the shim's stdin/stdout/stderr file descriptors over a Unix socket. The worker then speaks marimo's handshake directly.
  - The two variants are **(A) a warm pool** of fully started Python processes, and **(B) a fork server** that forks from a preloaded parent.

**Tech Stack:** as S0, plus Python's `socket.send_fds` / `recv_fds` (3.9+), `os.dup2`, `python -X importtime`.

## Global Constraints

- **Throwaway code.** Work on branch `spike/marimo-s0` in worktree `egret-wt-marimo-s0`, directory `spikes/marimo-s0/`. Never merge it. Only the result document is merged, on `docs/marimo-data-apps-spec`.
- Pin `marimo==0.25.0`. Read the installed copy under `.venv/lib/python3.12/site-packages/marimo/`.
- Hostnames are `egret.localhost` / `apps.egret.localhost`. Ports: fake egret 8700, proxy 8701, marimo edit 2718, marimo run 2719.
- Start and stop servers **only** through `probes/servers.sh`, which kills listeners only. **Never** run an unfiltered `lsof -ti :PORT | xargs kill`: it also kills the proxy through its client sockets.
- **Isolation must not regress.**
  - Every viewer session runs in its **own** OS process.
  - A worker or forked child serves **exactly one** kernel and then exits. It is never returned to a pool.
  - No process that has run a viewer's code, or held a viewer's token, is the parent of another viewer's kernel.
  - `q1_token.py` must still show each viewer on their own pid with their own identity.
- **Target:** a new viewer on a warm pod renders in **< 2 s** at p50 over 5 runs. Three concurrent new viewers render in **< 3 s**. Anything else is reported as measured, not rounded to the target.
- **Scope every timing:** runs per case, the machine, and load. Use "automated Playwright", never "manual", for browser probes.
- Every answer in the result document carries a number or an artefact path.

---

## File structure (all under `spikes/marimo-s0/`)

| File | Responsibility |
|---|---|
| `timing.py` | `mark(event, **kw)`: append one JSON line with `time.time()` and `os.getpid()` to `$S0_TIMING_LOG`. |
| `probes/s0b_breakdown.py` | Drive N viewers and merge browser-side and server-side marks into a per-session timeline. |
| `shim/python` | Shim-venv interpreter. Diverts `-m marimo._ipc.launch_kernel` to `shim_client.py`, execs the real Python otherwise. |
| `shim/pyvenv.cfg` | Makes the shim directory look like a venv to marimo, if marimo checks for one. |
| `shim_client.py` | Connects to the pool socket, sends fds 0/1/2 and its environment, then forwards signals and exits with the kernel's code. |
| `kernel_pool.py` | Variant A, the warm pool: keeps `S0_POOL_SIZE` warm workers and hands each accepted client to one. |
| `kernel_worker.py` | Variant A worker. Pre-imports, reports ready, receives fds + env, `dup2`s them, runs `launch_kernel.main()`, exits. |
| `kernel_forkserver.py` | Variant B, the fork server: preloads modules and forks a child per client. The child does what `kernel_worker.py` does after it receives the fds. |
| `probes/s0b_startup.py` | Time-to-render for 1 cold, 5 sequential and 3 concurrent new viewers → `results/s0b-<variant>.json`. |
| `probes/s0b_reload.py` | Reload within the session TTL: same kernel pid or a new one? |
| `tests/test_shim.py` | Unit tests: fd hand-over, signal forwarding, single use. |

`entrypoint.py` gains a `--kernel-launcher direct|pool|forkserver` flag, used only with `--process-mode ipc`.

---

### Task 1: Timing instrumentation and the 13 s breakdown

**Files:**
- Create: `spikes/marimo-s0/timing.py`, `spikes/marimo-s0/probes/s0b_breakdown.py`
- Modify: `spikes/marimo-s0/entrypoint.py` (marks around kernel launch), `spikes/marimo-s0/proxy.py` (marks at ticket redeem and WS open), `spikes/marimo-s0/egret_marimo/__init__.py` (marks before and after the gateway call), `spikes/marimo-s0/fake_egret.py` (a mark at query)
- Output: `results/s0b-breakdown-thread.json`, `results/s0b-breakdown-ipc.json`, `results/s0b-importtime.txt`

**Interfaces:**
- Produces: `timing.mark(event: str, **kw) -> None`. It is a no-op when `S0_TIMING_LOG` is unset. Each line is `{"t": float, "pid": int, "event": str, ...kw}`.
- Event names, used by later tasks: `ticket_redeemed`, `ws_open`, `kernel_spawn_begin`, `kernel_ready`, `kernel_first_cell`, `gateway_request`, `gateway_response`, `egret_query`.

- [ ] **Step 1: Write `timing.py`**

```python
"""Append-only timing marks for the S0b breakdown (spike only)."""

import json
import os
import time


def mark(event: str, **kw) -> None:
    path = os.environ.get("S0_TIMING_LOG")
    if not path:
        return
    line = json.dumps({"t": time.time(), "pid": os.getpid(), "event": event, **kw})
    with open(path, "a") as f:
        f.write(line + "\n")
```

- [ ] **Step 2: Add the marks**

Set `S0_TIMING_LOG=$PWD/.work/logs/timing.jsonl` for every process that `servers.sh` starts; kernels inherit it through the environment. Add these calls:

- `proxy.ticket`, after a successful redeem: `mark("ticket_redeemed", user=sess["user"])`
- `proxy.ws_relay`, right after `await ws.accept()`: `mark("ws_open", path=ws.url.path, user=sess["user"])`
- `egret_marimo._query`: `mark("gateway_request", sql=sql)` before the POST and `mark("gateway_response", sql=sql, status=r.status_code)` after it
- `fake_egret.query`: `mark("egret_query", user=info["user"], sql=body["sql"])` (use `from timing import mark`)
- `kernel_spawn_begin` / `kernel_ready`, via a patch installed by `entrypoint.py` in ipc mode. Wrap `IPCKernelManagerImpl.start_kernel`: mark `kernel_spawn_begin` before awaiting the original and `kernel_ready` after it returns.
- `kernel_first_cell`: add `import timing; timing.mark("kernel_first_cell")` as the first line of `apps/probe.py`'s first cell. Revert this edit at the end of Task 4, so later probe runs use the S0 notebook unchanged.

- [ ] **Step 3: Write `probes/s0b_breakdown.py`**

```python
"""Per-session timeline: browser marks + server/kernel marks from timing.jsonl."""

import asyncio
import json
import pathlib
import sys
import time

from playwright.async_api import async_playwright

from common import open_app, wait_text

ROOT = pathlib.Path(__file__).parent.parent
LOG = ROOT / ".work" / "logs" / "timing.jsonl"
MODE = sys.argv[1]  # thread | ipc — must match the running server
N = int(sys.argv[2]) if len(sys.argv) > 2 else 3


async def one(browser, user: str) -> dict:
    t0 = time.time()
    _, frame = await open_app(browser, user, "probe", "run")
    await wait_text(frame, "PY pid=", timeout=300000)
    return {"user": user, "nav_start": t0, "rendered": time.time()}


async def main() -> None:
    LOG.write_text("")
    async with async_playwright() as p:
        browser = await p.chromium.launch()
        sessions = []
        for i in range(N):  # sequential, so the marks do not interleave
            sessions.append(await one(browser, f"b{i}"))
        await browser.close()
    marks = [json.loads(line) for line in LOG.read_text().splitlines() if line]
    timelines = []
    for s in sessions:
        window = [m for m in marks if s["nav_start"] <= m["t"] <= s["rendered"]]
        timelines.append({
            "user": s["user"],
            "total_s": round(s["rendered"] - s["nav_start"], 3),
            "events": [{"event": m["event"], "at_s": round(m["t"] - s["nav_start"], 3),
                        "pid": m["pid"]} for m in sorted(window, key=lambda m: m["t"])],
        })
    out = ROOT / "results" / f"s0b-breakdown-{MODE}.json"
    out.write_text(json.dumps({"mode": MODE, "sessions": timelines}, indent=2))
    print(json.dumps(timelines, indent=2))


asyncio.run(main())
```

- [ ] **Step 4: Run the breakdown in thread mode, then in ipc mode**

For each mode:
1. `probes/servers.sh` up with the run server in that `--process-mode`.
2. Run `cd probes && timeout 900 uv run python s0b_breakdown.py <mode> 3`.

Expected: for each session, the gap each hop adds, read off the timeline. The ipc minus thread difference should sit between `kernel_spawn_begin` and `kernel_ready`, plus the kernel's first-cell import time. Record where it actually sits.

- [ ] **Step 5: Measure import cost inside a kernel-like process**

```bash
timeout 120 uv run python -X importtime -c "import marimo._ipc.launch_kernel, marimo._runtime.runtime, polars, httpx, egret_marimo, anywidget" 2> .work/importtime.raw
sort -t'|' -k2 -n -r .work/importtime.raw | head -30 > results/s0b-importtime.txt
```

Also time a bare interpreter start 5 times with `/usr/bin/time -p uv run python -c pass`. Record the median in the result.

- [ ] **Step 6: Commit**

```bash
git add spikes/marimo-s0 && git commit -m "spike(marimo-s0): S0b timing marks and start-up breakdown"
```

---

### Task 2: Shim venv and client, with unit tests

**Files:**
- Create: `spikes/marimo-s0/shim/python`, `spikes/marimo-s0/shim/pyvenv.cfg`, `spikes/marimo-s0/shim_client.py`
- Test: `spikes/marimo-s0/tests/test_shim.py`

**Interfaces:**
- Consumes: `$S0_KERNEL_SOCKET`, the path of the Unix socket served by Task 3's pool or Task 4's fork server; `$S0_REAL_PYTHON`, the absolute path of `.venv/bin/python`.
- Wire protocol, client to server, on one `SOCK_STREAM` connection:
  1. `send_fds(sock, [env_json_bytes], [0, 1, 2])`, where `env_json_bytes` is the UTF-8 JSON of `dict(os.environ)` and is at most 64 KiB.
  2. The server replies with one line, `PID <kernel pid>\n`.
  3. When the kernel exits, the server sends one more line, `EXIT <code>\n`, and closes the connection.
- `shim_client.py` forwards SIGINT, SIGTERM and SIGHUP to `<kernel pid>` and exits with `<code>`.

- [ ] **Step 1: Write the failing tests**

```python
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
```

- [ ] **Step 2: Run to see it fail**

Run: `uv run pytest tests/test_shim.py -v`
Expected: FAIL, because `shim_client.py` is missing and the client exits non-zero at once.

- [ ] **Step 3: Implement `shim_client.py`**

```python
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
```

- [ ] **Step 4: Implement the shim interpreter `shim/python`**

This file must be executable (`chmod +x`).

```sh
#!/bin/sh
# Shim-venv interpreter: divert marimo's kernel launch to a pre-started worker.
if [ "$1" = "-m" ] && [ "$2" = "marimo._ipc.launch_kernel" ]; then
  exec "$S0_REAL_PYTHON" "$(dirname "$0")/../shim_client.py"
fi
exec "$S0_REAL_PYTHON" "$@"
```

`shim/pyvenv.cfg`:

```text
home = /usr/bin
include-system-site-packages = false
version = 3.12
```

Next, read `site-packages/marimo/_session/_venv.py` `get_configured_venv_python` to find the path it expects for a configured venv. The usual layout is `<path>/bin/python`, so the shim layout must match it: put the script at `shim/bin/python`, not at `shim/python`, and move `pyvenv.cfg` alongside if marimo checks for it. `has_marimo_installed` runs `<python> -c "import marimo, msgspec, zmq; ..."`, which goes through the `exec "$S0_REAL_PYTHON" "$@"` branch. Record the final layout in the README.

- [ ] **Step 5: Run the tests until they pass**

Run: `uv run pytest tests -q`
Expected: all S0 tests plus `test_shim.py` pass.

- [ ] **Step 6: Commit**

```bash
git add spikes/marimo-s0 && git commit -m "spike(marimo-s0): S0b launcher shim and client"
```

---

### Task 3: Variant A, the warm pool of single-use workers, and measurement

**Files:**
- Create: `spikes/marimo-s0/kernel_pool.py`, `spikes/marimo-s0/kernel_worker.py`, `spikes/marimo-s0/probes/s0b_startup.py`
- Modify: `spikes/marimo-s0/entrypoint.py` (`--kernel-launcher pool`: start `kernel_pool.py` and point the ipc venv patch at `shim/`), `spikes/marimo-s0/probes/servers.sh` (pass the flag through)
- Output: `results/s0b-direct.json`, `results/s0b-pool.json`, `results/q1-pool.json`, `results/s0b-pool-rss.txt`

**Interfaces:**
- Consumes: the wire protocol from Task 2, and `timing.mark`.
- Produces: `kernel_pool.py --size N --socket PATH`. It prints `POOL_READY` once N workers are warm. Each worker prints `WORKER_WARM <pid>` to its own stderr log.

- [ ] **Step 1: Implement `kernel_worker.py`**

```python
"""A single-use, pre-started kernel process (S0b variant A)."""

import json
import os
import socket
import sys

import timing

# Pay the import cost before any viewer arrives.
import marimo._ipc.launch_kernel as launch_kernel  # noqa: E402
import marimo._runtime.runtime  # noqa: E402,F401
import polars  # noqa: E402,F401
import httpx  # noqa: E402,F401
import egret_marimo  # noqa: E402,F401


def serve_one(conn: socket.socket) -> None:
    msg, fds, _, _ = socket.recv_fds(conn, 65536, 3)
    env = json.loads(msg.decode())
    os.environ.clear()
    os.environ.update(env)
    for target, fd in zip((0, 1, 2), fds):
        os.dup2(fd, target)
        os.close(fd)
    sys.stdin = open(0, "r", closefd=False)
    sys.stdout = open(1, "w", closefd=False, buffering=1)
    sys.stderr = open(2, "w", closefd=False, buffering=1)
    conn.sendall(f"PID {os.getpid()}\n".encode())
    timing.mark("worker_claimed")
    code = 0
    try:
        launch_kernel.main()
    except SystemExit as e:
        code = int(e.code or 0)
    except BaseException:
        code = 1
        raise
    finally:
        try:
            conn.sendall(f"EXIT {code}\n".encode())
        finally:
            os._exit(code)


def main() -> None:
    channel = socket.socket(fileno=int(sys.argv[1]))  # inherited from the pool
    print(f"WORKER_WARM {os.getpid()}", file=sys.stderr, flush=True)
    timing.mark("worker_warm")
    conn_fd = socket.recv_fds(channel, 1, 1)[1][0]  # the pool passes the accepted client connection
    serve_one(socket.socket(fileno=conn_fd))


if __name__ == "__main__":
    main()
```

- [ ] **Step 2: Implement `kernel_pool.py`**

```python
"""Warm pool: N single-use workers; each accepted shim connection consumes one (S0b variant A)."""

import argparse
import os
import socket
import subprocess
import sys
import threading
from pathlib import Path

HERE = Path(__file__).parent


class Pool:
    def __init__(self, size: int) -> None:
        self.size = size
        self.idle: list[tuple[subprocess.Popen, socket.socket]] = []
        self.lock = threading.Condition()

    def spawn(self) -> None:
        parent, child = socket.socketpair()
        proc = subprocess.Popen(
            [sys.executable, str(HERE / "kernel_worker.py"), str(child.fileno())],
            pass_fds=[child.fileno()], stderr=open(HERE / ".work/logs/workers.log", "ab"),
        )
        child.close()
        # a worker is usable once it has finished importing; wait for its WORKER_WARM mark by
        # giving it the channel only when claimed — until then it blocks in recv_fds
        with self.lock:
            self.idle.append((proc, parent))
            self.lock.notify_all()

    def claim(self) -> tuple[subprocess.Popen, socket.socket]:
        with self.lock:
            while not self.idle:
                self.lock.wait()
            worker = self.idle.pop(0)
        threading.Thread(target=self.spawn, daemon=True).start()  # refill; never reuse
        return worker


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--size", type=int, default=2)
    ap.add_argument("--socket", required=True)
    a = ap.parse_args()
    pool = Pool(a.size)
    for _ in range(a.size):
        pool.spawn()
    if os.path.exists(a.socket):
        os.unlink(a.socket)
    srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    srv.bind(a.socket)
    os.chmod(a.socket, 0o600)
    srv.listen(16)
    print("POOL_READY", flush=True)
    while True:
        conn, _ = srv.accept()
        proc, channel = pool.claim()
        socket.send_fds(channel, [b"c"], [conn.fileno()])
        conn.close()
        channel.close()


if __name__ == "__main__":
    main()
```

A worker that has been handed out is never put back. The pool starts a replacement right away, and that replacement pays its own import cost in the background.

Known gap: a claimed worker may still be importing. `claim()` does not wait for `WORKER_WARM`, so a claim during warm-up is slower and does not break anything. The probe measures warm claims: it waits for N `worker_warm` marks before the first viewer. Record any cold-claim number separately.

- [ ] **Step 3: Wire `--kernel-launcher pool` into `entrypoint.py` and `servers.sh`**

In ipc mode, when `--kernel-launcher pool` is set:
- set `S0_REAL_PYTHON=sys.executable` and `S0_KERNEL_SOCKET=$PWD/.work/kernel.sock`;
- start `kernel_pool.py --size ${S0_POOL_SIZE:-3} --socket …` as a child process, and wait for its `POOL_READY` (bounded to 120 s);
- point the patched `_get_venv_config` at the shim directory, not at `sys.prefix`.

`direct` keeps the S0 behaviour.

- [ ] **Step 4: Write `probes/s0b_startup.py`**

```python
"""Time-to-render for 1 cold + 5 sequential + 3 concurrent new viewers, one launcher variant."""

import asyncio
import json
import pathlib
import statistics
import sys
import time

from playwright.async_api import async_playwright

from common import open_app, wait_text

VARIANT = sys.argv[1]  # direct | pool | forkserver — must match the running server
OUT = pathlib.Path(__file__).parent.parent / "results"


async def one(browser, user):
    t0 = time.monotonic()
    _, frame = await open_app(browser, user, "probe", "run")
    text = await wait_text(frame, "PY pid=", timeout=300000)
    return {"user": user, "pid": int(text.split("pid=")[1].split()[0]),
            "viewer_ok": f"viewer={user}" in text, "seconds": round(time.monotonic() - t0, 2)}


async def main() -> None:
    async with async_playwright() as p:
        browser = await p.chromium.launch()
        first = await one(browser, "c0")
        sequential = [await one(browser, f"s{i}") for i in range(5)]
        await asyncio.sleep(10)  # let a pool refill before the concurrent burst
        concurrent = await asyncio.gather(*(one(browser, f"k{i}") for i in range(3)))
        await browser.close()
    everyone = [first, *sequential, *concurrent]
    result = {
        "variant": VARIANT,
        "first": first,
        "sequential": sequential,
        "sequential_p50_s": statistics.median(s["seconds"] for s in sequential),
        "concurrent": concurrent,
        "concurrent_max_s": max(s["seconds"] for s in concurrent),
        "distinct_pids": len({s["pid"] for s in everyone}),
        "sessions": len(everyone),
        "every_viewer_ok": all(s["viewer_ok"] for s in everyone),
    }
    (OUT / f"s0b-{VARIANT}.json").write_text(json.dumps(result, indent=2))
    print(json.dumps({k: v for k, v in result.items() if k not in ("sequential", "concurrent")}, indent=2))


asyncio.run(main())
```

- [ ] **Step 5: Measure the direct baseline and the pool**

1. Direct: bring the stack up with `--process-mode ipc --kernel-launcher direct`, then run `cd probes && timeout 1200 uv run python s0b_startup.py direct`.
2. Pool: bring it up with `--kernel-launcher pool`. Wait until the timing log has `S0_POOL_SIZE` `worker_warm` marks, then run `timeout 1200 uv run python s0b_startup.py pool`.

Expected, for both:
- `distinct_pids == sessions`, which is 9;
- `every_viewer_ok: true`.

Also record:
- `sequential_p50_s` and `concurrent_max_s` against the targets (< 2 s and < 3 s);
- any error from the handshake.

A `false` or a timeout is a result, not something to retry away.

- [ ] **Step 6: Re-check identity and single use**

1. Run `q1_token.py` against the pool variant, and save the output as `results/q1-pool.json`. The committed `results/q1.json` must not be overwritten.
2. Get the pid of every kernel that served a viewer from the `worker_claimed` marks. Confirm each pid appears once, and that no pid is still alive after its viewer closes.
3. Capture `ps -o pid,ppid,rss,command` for the pool and its workers in `results/s0b-pool-rss.txt`, including the idle warm workers: their memory is the cost of the pool.

- [ ] **Step 7: Commit**

```bash
git add spikes/marimo-s0 && git commit -m "spike(marimo-s0): S0b variant A — warm pool of single-use kernels"
```

---

### Task 4: Variant B, the fork server, and measurement

**Files:**
- Create: `spikes/marimo-s0/kernel_forkserver.py`
- Modify: `spikes/marimo-s0/entrypoint.py` (`--kernel-launcher forkserver`), `spikes/marimo-s0/probes/servers.sh`
- Output: `results/s0b-forkserver.json`, `results/q1-forkserver.json`, `results/s0b-fork-safety.json`

**Interfaces:**
- Consumes: the Task 2 protocol, and `kernel_worker.serve_one(conn)` from Task 3 (reuse it, do not copy it).
- Produces: `kernel_forkserver.py --socket PATH --preload light|full`, which prints `FORKSERVER_READY`.

- [ ] **Step 1: Implement `kernel_forkserver.py`**

```python
"""Fork server: preload once, fork a fresh child per viewer kernel (S0b variant B)."""

import argparse
import importlib
import os
import signal
import socket

PRELOAD = {
    "light": ["marimo._ipc.launch_kernel", "marimo._runtime.runtime", "httpx", "egret_marimo"],
    "full": ["marimo._ipc.launch_kernel", "marimo._runtime.runtime", "httpx", "egret_marimo", "polars"],
}


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--socket", required=True)
    ap.add_argument("--preload", choices=list(PRELOAD), default="light")
    a = ap.parse_args()
    for name in PRELOAD[a.preload]:
        importlib.import_module(name)
    import kernel_worker  # serve_one; its module-level imports are the same set

    signal.signal(signal.SIGCHLD, signal.SIG_IGN)  # auto-reap children
    if os.path.exists(a.socket):
        os.unlink(a.socket)
    srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    srv.bind(a.socket)
    os.chmod(a.socket, 0o600)
    srv.listen(16)
    print(f"FORKSERVER_READY {a.preload}", flush=True)
    while True:
        conn, _ = srv.accept()
        if os.fork() == 0:
            srv.close()
            signal.signal(signal.SIGCHLD, signal.SIG_DFL)
            kernel_worker.serve_one(conn)  # never returns
        conn.close()


if __name__ == "__main__":
    main()
```

The fork server itself never runs viewer code and never holds a token: `serve_one` runs only in the forked child. This keeps the isolation constraint intact.

`kernel_worker.py` imports polars at module level. For the `light` preload, move that import into `main()` so that `light` truly leaves polars out. Keep the import there for variant A, since the worker is not a fork parent.

- [ ] **Step 2: Measure fork safety**

Create `probes/s0b_fork_safety.py`. It starts the fork server with `--preload full` and with `--preload light`. For each, it runs one kernel through the full app (`s0b_startup.py forkserver` with only the `first` viewer), and records:
- whether `PY pid=` rendered;
- any `fork()` warnings from polars or other libraries in `.work/logs/*`, captured with `grep -i "fork"`;
- whether the polars query path, `egret.sql → pl.DataFrame`, returned correct data.

Write the results to `results/s0b-fork-safety.json`.

Expected: `light` is safe. `full` may warn, because polars starts a thread pool when imported. Record what happens.

- [ ] **Step 3: Measure the fork server with the safe preload**

Bring the stack up with `--kernel-launcher forkserver`, using the preload Step 2 found safe. Then run `cd probes && timeout 1200 uv run python s0b_startup.py forkserver`, and run `q1_token.py` saving its output as `results/q1-forkserver.json`.

Expected: the same isolation checks as in Task 3 Step 5, plus the numbers.

- [ ] **Step 4: Measure reload within the session TTL**

Create `probes/s0b_reload.py`:
1. Open the app as alice and read the pid.
2. Reload the host page, which issues a new ticket, and read the pid again.
3. Reload the iframe only, `frame.page.reload()` on the iframe URL, and read the pid again.

Save the three pids and timings to `results/s0b-reload.json`. This shows whether a reload reuses the kernel, meaning `--session-ttl` helps, or starts a new one.

- [ ] **Step 5: Revert the `kernel_first_cell` edit in `apps/probe.py`, and commit**

```bash
git checkout -- spikes/marimo-s0/apps/probe.py  # or revert just the timing line if other edits exist
git add spikes/marimo-s0 && git commit -m "spike(marimo-s0): S0b variant B — fork server, fork safety, reload reuse"
```

---

### Task 5: Result document and spec update

**Files:**
- Create (on `docs/marimo-data-apps-spec`, worktree `egret-wt-marimo-spec`): `docs/en/plans/<run-date>-marimo-s0b-result.md`
- Modify: `docs/en/specs/2026-09-30-marimo-data-apps-design.md` and `docs/vi/specs/2026-09-30-marimo-data-apps-design.md`. The sections are D7, §6.3, §9 (add an S0b row) and §12 if anything moves out of scope.

- [ ] **Step 1: Write the result document**

```markdown
# Data Apps S0b — start-up latency result

**Run:** <date>, marimo 0.25.0, HeadlessChrome <ver>, macOS dev laptop, branch `spike/marimo-s0` @ <sha> (never merged)

## Where the ~13 s went
<table from s0b-breakdown-ipc.json vs -thread.json: per hop, seconds, and which hop the ipc delta lands in; import top-10 from s0b-importtime.txt; bare interpreter median>

## Launchers compared
| Variant | first (s) | sequential p50 (s) | 3 concurrent max (s) | distinct pids / sessions | identity ok | idle cost (MB) |
|---|---|---|---|---|---|---|
| direct (S0) | | | | | | 0 |
| pool (A, size N) | | | | | | |
| forkserver (B, preload …) | | | | | | |

Targets: < 2 s p50 new viewer on a warm pod; < 3 s for 3 concurrent — met / not met, per variant.

## Fork safety
## Reload reuse
## Findings
## What the spec must now say
```

- [ ] **Step 2: Update the spec (en, then vi)**

- D7 and §6.3: name the chosen launcher, with its measured numbers and idle cost, and say that marimo's `KERNEL_INFO` launcher contract is what it depends on. Point out that this contract is documented as public (`marimo._ipc.launch_kernel` is "a public API used by external consumers"), unlike the S0 patches.
- §6.3: revise the per-pod session cap and the cold-start UX (loading screen, pool size, pod idle timeout) using the measured numbers.
- §9: add an S0b row marked done, with a link to the result document.

- [ ] **Step 3: Commit on the spec branch, report to the user, and stop**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret-wt-marimo-spec
git add docs && git commit -m "docs: Data Apps S0b result — <one-line summary>"
```

Report each answer in one line, and say whether the targets were met. Do not start P1 without the user's go-ahead.
