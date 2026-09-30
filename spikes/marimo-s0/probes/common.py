import httpx

EGRET_PUBLIC = "http://egret.localhost:8700"
EGRET_LOCAL = "http://127.0.0.1:8700"


async def open_app(browser, user: str, app_id: str, mode: str):
    ctx = await browser.new_context()
    page = await ctx.new_page()
    await page.goto(f"{EGRET_PUBLIC}/login?user={user}")
    await page.goto(f"{EGRET_PUBLIC}/apps/{app_id}/{mode}")
    return page, page.frame_locator("#app")


async def wait_text(frame, prefix: str, timeout: int = 90000) -> str:
    loc = frame.get_by_text(prefix).first
    await loc.wait_for(timeout=timeout)
    return await loc.inner_text()


def events() -> list[dict]:
    return httpx.get(f"{EGRET_LOCAL}/_events").json()


def reset_events() -> None:
    httpx.post(f"{EGRET_LOCAL}/_events/reset")
