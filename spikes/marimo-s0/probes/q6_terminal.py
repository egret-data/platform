"""Q6 step 4 (automated Playwright, not manual): the terminal must be unreachable through the proxy.
Tries the real UI path (open the terminal panel), then a direct WebSocket from the page. Also checks blocked HTTP routes on /e and /r."""

import asyncio
import json
import pathlib

from playwright.async_api import async_playwright

from common import open_app

OUT = pathlib.Path(__file__).parent.parent / "results"
PROXY = "http://apps.egret.localhost:8701"

WS_JS = """(url) => new Promise(res => {
  const w = new WebSocket(url); const t = setTimeout(() => res({state: 'timeout'}), 8000);
  w.onopen = () => { clearTimeout(t); res({state: 'open'}); w.close(); };
  w.onclose = e => { clearTimeout(t); res({state: 'closed', code: e.code, wasClean: e.wasClean}); };
  w.onerror = () => {};
})"""

FETCH_JS = """async (paths) => Object.fromEntries(await Promise.all(paths.map(async ([m, p]) => {
  const r = await fetch(p, {method: m, headers: m === 'POST' ? {'content-type': 'application/json'} : {}, body: m === 'POST' ? '{}' : undefined});
  return [m + ' ' + p, r.status];
})))"""

BLOCKED_SAMPLE = [("GET", "/api/packages/list"), ("POST", "/api/secrets/keys"), ("POST", "/api/files/list_files"),
                  ("POST", "/api/ai/chat"), ("POST", "/api/storage/list_entries"), ("POST", "/api/home/recent_files"),
                  ("POST", "/api/kernel/copy"), ("POST", "/api/kernel/rename"), ("GET", "/api/lsp/health"),
                  ("POST", "/api/export/auto_export/html"), ("GET", "/terminal/ws")]
ALLOWED_SAMPLE = [("GET", "/api/kernel/status"), ("GET", "/api/version"), ("GET", "/health")]


async def main() -> None:
    result: dict = {"method": "automated Playwright, headless Chromium (not manual)"}
    async with async_playwright() as p:
        browser = await p.chromium.launch()
        page, frame = await open_app(browser, "alice", "probe", "edit")
        await frame.get_by_text("PY pid=").first.wait_for(timeout=90000)
        ui = {"clicked": None}
        for sel in ['[data-testid="footer-panel-toggle"]', 'button:has-text("Terminal")', '[aria-label*="erminal" i]', '[data-testid*="terminal" i]']:
            loc = frame.locator(sel).first
            if await loc.count():
                try:
                    await loc.click(timeout=3000)
                    ui["clicked"] = sel
                    break
                except Exception as e:  # noqa: BLE001
                    ui.setdefault("errors", []).append(f"{sel}: {str(e)[:100]}")
        if not ui["clicked"]:
            await frame.locator("body").press("Control+`")  # marimo's terminal shortcut is not guaranteed; recorded as-is
            ui["clicked"] = "keyboard Control+`"
        await asyncio.sleep(3)
        result["ui_attempt"] = ui
        await page.screenshot(path=str(OUT / "q6-terminal.png"), full_page=True)
        app_frame = next(f for f in page.frames if f != page.main_frame)
        result["ws_terminal_e"] = await app_frame.evaluate(WS_JS, f"ws://apps.egret.localhost:8701/e/terminal/ws")
        result["ws_main_e"] = await app_frame.evaluate(WS_JS, f"ws://apps.egret.localhost:8701/e/ws?file=probe.py&session_id=s_q6probe")
        result["http_blocked_e"] = await app_frame.evaluate(FETCH_JS, [["GET", "/e" + p_] if m == "GET" else ["POST", "/e" + p_] for m, p_ in BLOCKED_SAMPLE])
        result["http_allowed_e"] = await app_frame.evaluate(FETCH_JS, [[m, "/e" + p_] for m, p_ in ALLOWED_SAMPLE])
        await browser.close()
        browser = await p.chromium.launch()
        page, frame = await open_app(browser, "alice", "probe", "run")
        await frame.get_by_text("PY pid=").first.wait_for(timeout=90000)
        app_frame = next(f for f in page.frames if f != page.main_frame)
        result["ws_terminal_r"] = await app_frame.evaluate(WS_JS, "ws://apps.egret.localhost:8701/r/terminal/ws")
        result["http_blocked_r"] = await app_frame.evaluate(FETCH_JS, [[m, "/r" + p_] for m, p_ in BLOCKED_SAMPLE])
        result["http_allowed_r"] = await app_frame.evaluate(FETCH_JS, [[m, "/r" + p_] for m, p_ in ALLOWED_SAMPLE])
        await browser.close()
    (OUT / "q6-terminal.json").write_text(json.dumps(result, indent=2))
    print(json.dumps(result, indent=2))


asyncio.run(main())
