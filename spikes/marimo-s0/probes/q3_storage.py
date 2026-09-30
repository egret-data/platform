"""Q3: an edit saved in marimo lands in egret's draft; a stale save gets 409 - what does the author see?"""

import asyncio
import json
import pathlib
import re

import httpx
from playwright.async_api import async_playwright

from common import EGRET_LOCAL, events, open_app, reset_events

OUT = pathlib.Path(__file__).parent.parent / "results"
EDIT_LOG = pathlib.Path(__file__).parent.parent / ".work/logs/edit.log"
POD = {"authorization": "Bearer pod-kernel-token"}
MARKER = "s0_marker = 12345"


def draft() -> dict:
    return httpx.get(f"{EGRET_LOCAL}/api/apps/probe/draft", headers=POD).json()


def binds() -> list[int]:
    return [int(m) for m in re.findall(r"EgretStorage bound to probe rev (\d+)", EDIT_LOG.read_text())]


async def add_cell_and_save(page, frame, code: str) -> None:
    await frame.get_by_text("PY pid=").first.wait_for(timeout=90000)
    editor = frame.locator(".cm-content").last
    await editor.click()
    await page.keyboard.press("End")
    await page.keyboard.press("Enter")
    await page.keyboard.type(code)
    await page.keyboard.press("Control+s")
    await asyncio.sleep(3)


async def main() -> None:
    reset_events()
    before = draft()
    saves: list[dict] = []
    async with async_playwright() as p:
        browser = await p.chromium.launch()
        page, frame = await open_app(browser, "alice", "probe", "edit")

        async def on_resp(r):
            if "/api/kernel/save" in r.url and r.request.method == "POST":
                try:
                    body = (await r.text())[:500]
                except Exception as e:  # noqa: BLE001
                    body = f"<{e}>"
                saves.append({"url": r.url, "status": r.status, "body": body})

        page.on("response", lambda r: asyncio.ensure_future(on_resp(r)))

        await add_cell_and_save(page, frame, MARKER)
        await page.screenshot(path=str(OUT / "q3-after-save.png"), full_page=True)
        saved = draft()
        binds_before_put = binds()
        saves_first = len(saves)

        # someone else saves first (no page reload / navigation after this point)
        put = httpx.put(f"{EGRET_LOCAL}/api/apps/probe/draft", content=(saved["source"] + "\n# other editor\n").encode(),
                        headers={**POD, "if-match": str(saved["revision"])})
        after_put = draft()

        await add_cell_and_save(page, frame, "s0_second = 1")
        await page.screenshot(path=str(OUT / "q3-conflict.png"), full_page=True)
        visible = await frame.locator("body").inner_text()
        alerts = await frame.locator("[role=alert], [role=status], .toast, [data-sonner-toast], [data-radix-toast-viewport] *").all_inner_texts()
        final = draft()
        binds_after = binds()
        await browser.close()

    result = {
        "revision_before": before["revision"],
        "revision_after_first_save": saved["revision"],
        "marker_in_draft": MARKER in saved["source"],
        "external_put_status": put.status_code,
        "revision_after_external_put": after_put["revision"],
        "second_marker_in_final_draft": "s0_second = 1" in final["source"],
        "revision_final": final["revision"],
        "save_responses": saves,
        "save_responses_before_external_put": saves_first,
        "storage_binds": {
            "revisions_bound_before_external_put": binds_before_put,
            "revisions_bound_total": binds_after,
            "binds_after_external_put": len(binds_after) - len(binds_before_put),
            "note": "0 binds after the external PUT means no page load re-fetched the revision, so the second save used the stale revision",
        },
        "conflict_events": [e for e in events() if e["kind"] == "draft_conflict"],
        "toast_texts": alerts,
        "ui_mentions_conflict": any(w in (visible + " ".join(alerts)).lower() for w in ("reload", "someone else", "conflict", "409")),
        "ui_text_excerpt": visible[-2000:],
    }
    (OUT / "q3.json").write_text(json.dumps(result, indent=2))
    print(json.dumps({k: v for k, v in result.items() if k != "ui_text_excerpt"}, indent=2))


asyncio.run(main())
