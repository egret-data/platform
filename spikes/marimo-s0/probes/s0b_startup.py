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
