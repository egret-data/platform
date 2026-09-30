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
