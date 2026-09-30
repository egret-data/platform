"""Reload within the session TTL: does a reload reuse the kernel or start a new one? (S0b, automated Playwright)

1. open the app as alice, read the pid; 2. reload the host page (new ticket); 3. reload only the
iframe (navigate the iframe to its own URL: same apps_sid cookie, no new ticket). After each step:
the rendered pid (or the error text), seconds, and how many kernels the launcher started
(`worker_claimed` marks) and which egret events happened. Needs a process-per-viewer run server.
"""

import asyncio
import json
import pathlib
import sys
import time

from playwright.async_api import async_playwright

from common import EGRET_PUBLIC, events, reset_events

ROOT = pathlib.Path(__file__).parent.parent
LOG = ROOT / ".work" / "logs" / "timing.jsonl"
LABEL = sys.argv[1] if len(sys.argv) > 1 else "forkserver"


def claims() -> list[int]:
    return [m["pid"] for m in map(json.loads, filter(None, LOG.read_text().splitlines())) if m["event"] == "worker_claimed"]


def app_frame(page):
    return next(f for f in page.frames if f.parent_frame is not None)


async def read(frame, t0: float) -> dict:
    """First of: the PY line, or the egret_marimo error text. Frame here is a Frame (not a locator)."""
    py = frame.get_by_text("PY pid=").first
    err = frame.get_by_text("session expired").first
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        if await py.count():
            text = await py.inner_text()
            return {"pid": int(text.split("pid=")[1].split()[0]), "text": text, "seconds": round(time.monotonic() - t0, 2)}
        if await err.count():
            return {"pid": None, "text": (await err.inner_text())[:300], "seconds": round(time.monotonic() - t0, 2)}
        await asyncio.sleep(0.1)
    body = await frame.locator("body").inner_text()
    return {"pid": None, "text": "neither PY line nor error text within 60 s", "seconds": round(time.monotonic() - t0, 2),
            "body_excerpt": " ".join(body.split())[:600]}


async def step(name: str, action, page) -> dict:
    before_claims, before_events = len(claims()), len(events())
    t0 = time.monotonic()
    await action()
    frame = app_frame(page)
    result = await read(frame, t0)
    await asyncio.sleep(2)  # let the launcher's mark and egret's events land
    result.update({
        "step": name,
        "iframe_url": frame.url,
        "kernels_started": claims()[before_claims:],
        "egret_events": [{k: e.get(k) for k in ("kind", "user", "pid") if k in e} for e in events()[before_events:]],
    })
    return result


async def main() -> None:
    reset_events()
    async with async_playwright() as p:
        browser = await p.chromium.launch()
        ctx = await browser.new_context()
        page = await ctx.new_page()
        await page.goto(f"{EGRET_PUBLIC}/login?user=alice")
        steps = [await step("open", lambda: page.goto(f"{EGRET_PUBLIC}/apps/probe/run"), page)]
        steps.append(await step("host_reload", lambda: page.reload(), page))
        frame = app_frame(page)
        steps.append(await step("iframe_reload", lambda: frame.goto(frame.url), page))
        await browser.close()
    pids = [s["pid"] for s in steps]
    out = {
        "label": LABEL,
        "probe": "automated Playwright, one run, macOS laptop",
        "steps": steps,
        "pids": pids,
        "host_reload_reused_kernel": pids[1] is not None and pids[1] == pids[0],
        "iframe_reload_reused_kernel": pids[2] is not None and pids[2] == pids[1],
    }
    (ROOT / "results" / "s0b-reload.json").write_text(json.dumps(out, indent=2))
    print(json.dumps(out, indent=2))


asyncio.run(main())
