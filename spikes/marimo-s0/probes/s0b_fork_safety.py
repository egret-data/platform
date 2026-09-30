"""Fork safety of the fork server: one viewer per preload (full, light) through the whole app.

Needs fake egret + proxy up (servers.sh up-support) and no run server. For each preload it starts
the run server with KERNEL_LAUNCHER=forkserver, opens the app as one viewer (automated Playwright),
and records: whether `PY pid=` rendered, the polars path (egret.sql -> pl.DataFrame) data, the
fork server's thread count at fork time, and any line matching `fork` (case-insensitive) that was
appended to .work/logs/* during the run, except our own `forkserver` log lines and `forked` timing marks.
"""

import asyncio
import json
import os
import pathlib
import re
import subprocess
import time

from playwright.async_api import async_playwright

from common import open_app, wait_text

ROOT = pathlib.Path(__file__).parent.parent
LOGS = ROOT / ".work" / "logs"
SERVERS = str(ROOT / "probes" / "servers.sh")


def sizes() -> dict[pathlib.Path, int]:
    return {p: p.stat().st_size for p in LOGS.iterdir() if p.is_file()}


def appended_since(before: dict[pathlib.Path, int]) -> dict[str, str]:
    out = {}
    for p in LOGS.iterdir():
        if p.is_file():
            with open(p, "rb") as f:
                start = before.get(p, 0)
                f.seek(start if p.stat().st_size >= start else 0)  # `>` truncated it: read it all
                out[p.name] = f.read().decode(errors="replace")
    return out


def forkserver_pid() -> int | None:
    out = subprocess.run(["pgrep", "-f", "kernel_forkserver.py"], capture_output=True, text=True).stdout.split()
    for pid in map(int, out):  # the server is the one whose parent is not another fork server
        ppid = int(subprocess.run(["ps", "-o", "ppid=", "-p", str(pid)], capture_output=True, text=True).stdout.strip())
        if str(ppid) not in out:
            return pid
    return None


def os_threads(pid: int) -> int:
    lines = subprocess.run(["ps", "-M", "-p", str(pid)], capture_output=True, text=True).stdout.splitlines()
    return len(lines) - 1  # header


async def one_viewer(user: str) -> dict:
    async with async_playwright() as p:
        browser = await p.chromium.launch()
        t0 = time.monotonic()
        _, frame = await open_app(browser, user, "probe", "run")
        try:
            py = await wait_text(frame, "PY pid=", timeout=180000)
        except Exception as e:  # noqa: BLE001  record, do not crash the other preload
            await browser.close()
            return {"rendered": False, "error": repr(e)}
        seconds = round(time.monotonic() - t0, 2)
        try:
            sqlcell = await wait_text(frame, "SQLCELL", timeout=60000)
        except Exception as e:  # noqa: BLE001
            sqlcell = f"missing: {e!r}"
        await browser.close()
    return {"rendered": True, "seconds": seconds, "py_line": py, "sqlcell_line": sqlcell,
            "pid": int(py.split("pid=")[1].split()[0])}


def run_preload(preload: str) -> dict:
    subprocess.run([SERVERS, "stop-run"], capture_output=True, text=True)
    before = sizes()
    up = subprocess.run([SERVERS, "run", "ipc", f"S0_FORKSERVER_PRELOAD={preload}"], capture_output=True, text=True,
                        env={**os.environ, "KERNEL_LAUNCHER": "forkserver"})
    fs = forkserver_pid()
    threads_before = os_threads(fs) if fs else None
    user = f"fs{preload}"
    viewer = asyncio.run(one_viewer(user))
    threads_after = os_threads(fs) if fs else None
    appended = appended_since(before)
    ready = re.findall(r"FORKSERVER_READY.*", "".join(appended.values()))
    fork_lines = [f"{name}: {line}" for name, text in appended.items() for line in text.splitlines()
                  if "fork" in line.lower() and "forkserver" not in line.lower() and '"event": "forked"' not in line]
    expected_py = f"viewer={user} sql=SELECT 1 AS n"
    return {
        "preload": preload,
        "servers_run": up.stdout.strip(),
        "forkserver_ready_line": ready,
        "forkserver_pid": fs,
        "forkserver_os_threads_before_viewer": threads_before,
        "forkserver_os_threads_after_fork": threads_after,
        "viewer": viewer,
        "kernel_pid_differs_from_forkserver": viewer.get("pid") not in (None, fs),
        "polars_path_correct": viewer.get("rendered", False) and viewer["py_line"].endswith(expected_py),
        "sqlcell_correct": f"SQLCELL viewer={user}" in viewer.get("sqlcell_line", ""),
        "fork_warning_lines": fork_lines,
    }


def main() -> None:
    results = [run_preload(p) for p in ("full", "light")]
    subprocess.run([SERVERS, "stop-run"], capture_output=True, text=True)
    out = {"probe": "automated Playwright, one viewer per preload, macOS laptop", "runs": results}
    (ROOT / "results" / "s0b-fork-safety.json").write_text(json.dumps(out, indent=2))
    print(json.dumps(out, indent=2))


if __name__ == "__main__":
    main()
