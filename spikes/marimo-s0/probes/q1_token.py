"""Q1: the header reaches the right session's kernel, including reactive re-runs. Q5: SQL cell uses the engine."""

import asyncio
import json
import pathlib
import sys

from playwright.async_api import async_playwright

from common import events, open_app, reset_events, wait_text

OUT = pathlib.Path(__file__).parent.parent / "results"
TAG = sys.argv[1] if len(sys.argv) > 1 else ""  # e.g. forkserver -> results/q1-forkserver.json, no q5.png


async def bump_slider(frame) -> None:
    slider = frame.get_by_role("slider").first
    await slider.focus()
    for _ in range(3):
        await slider.press("ArrowRight")


async def main() -> None:
    reset_events()
    async with async_playwright() as p:
        browser = await p.chromium.launch()
        (pa, fa), (pb, fb) = await asyncio.gather(
            open_app(browser, "alice", "probe", "run"), open_app(browser, "bob", "probe", "run")
        )
        first_a, first_b = await asyncio.gather(wait_text(fa, "PY pid="), wait_text(fb, "PY pid="))
        await asyncio.gather(bump_slider(fa), bump_slider(fb))
        await asyncio.sleep(3)
        after_a, after_b = await asyncio.gather(wait_text(fa, "PY pid="), wait_text(fb, "PY pid="))
        sqlcell_a = await wait_text(fa, "SQLCELL")
        if not TAG:
            await pa.screenshot(path=str(OUT / "q5.png"), full_page=True)
        await browser.close()

    queries = [e for e in events() if e["kind"] == "query"]
    result = {
        "alice": {"first": first_a, "after_slider": after_a, "sqlcell": sqlcell_a},
        "bob": {"first": first_b, "after_slider": after_b},
        "gateway_queries": queries,
        "every_alice_page_shows_alice": all("viewer=alice" in s for s in (first_a, after_a)),
        "every_bob_page_shows_bob": all("viewer=bob" in s for s in (first_b, after_b)),
        "sql_cell_used_gateway": "viewer=alice" in sqlcell_a,
        "rejected_queries": [e for e in events() if e["kind"] == "query_rejected"],
    }
    (OUT / (f"q1-{TAG}.json" if TAG else "q1.json")).write_text(json.dumps(result, indent=2))
    print(json.dumps({k: v for k, v in result.items() if k != "gateway_queries"}, indent=2))


asyncio.run(main())
