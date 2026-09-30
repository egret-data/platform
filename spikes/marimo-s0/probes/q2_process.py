"""Q2: distinct kernel pids per viewer session, and time to first render, for one process mode."""

import asyncio
import json
import pathlib
import sys
import time

from playwright.async_api import async_playwright

from common import open_app, wait_text

MODE = sys.argv[1]  # thread | sandbox | ipc — must match how the run server was started
OUT = pathlib.Path(__file__).parent.parent / "results"


async def one(browser, user):
    t0 = time.monotonic()
    _, frame = await open_app(browser, user, "probe", "run")
    text = await wait_text(frame, "PY pid=", timeout=300000)
    return {"user": user, "pid": int(text.split("pid=")[1].split()[0]), "seconds": round(time.monotonic() - t0, 2)}


async def main() -> None:
    async with async_playwright() as p:
        browser = await p.chromium.launch()
        cold = await one(browser, "u0")
        warm = await asyncio.gather(*(one(browser, f"u{i}") for i in range(1, 4)))
        if len(sys.argv) > 2 and sys.argv[2] == "hold":
            # keep sessions open so rss can be sampled from outside
            (OUT / f"q2-{MODE}.hold").write_text("ready")
            for _ in range(60):
                if not (OUT / f"q2-{MODE}.hold").exists():
                    break
                await asyncio.sleep(1)
        await browser.close()
    sessions = [cold, *warm]
    result = {
        "mode": MODE,
        "sessions": sessions,
        "distinct_pids": len({s["pid"] for s in sessions}),
        "max_seconds": max(s["seconds"] for s in sessions),
    }
    (OUT / f"q2-{MODE}.json").write_text(json.dumps(result, indent=2))
    print(json.dumps(result, indent=2))


asyncio.run(main())
