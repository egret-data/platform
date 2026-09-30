"""Q4: marimo works behind the proxy on its own origin; the ticket is single-use; author JS cannot reach egret.

Extends the brief's probe with: browser-console capture (to explain any `blocked:` outcome),
per-vector resilience (anywidget may not execute at all -- that is itself a key finding, recorded
with a screenshot), and a probe-level check that a run-mode apps-origin cookie cannot reach the
`/e` (edit) upstream even if the author JS never runs.
"""

import asyncio
import json
import os
import pathlib

from playwright.async_api import async_playwright

from common import events, open_app, reset_events

OUT = pathlib.Path(__file__).parent.parent / "results"
SUFFIX = "-host-prefix" if os.environ.get("COOKIE_PREFIX") == "__Host-" else ""
APPS_ORIGIN = "http://apps.egret.localhost:8701"


async def grab(frame, prefix: str, timeout_ms: int) -> str | None:
    """Return the first element text containing `prefix`, or None if it never appears."""
    try:
        loc = frame.get_by_text(prefix).first
        await loc.wait_for(timeout=timeout_ms)
        return await loc.inner_text()
    except Exception as e:  # noqa: BLE001 - a timeout means the vector did not render/execute
        return None


async def req_status(request_ctx, url: str) -> object:
    """GET status, resilient to `apps.egret.localhost` resolving to ::1 (proxy is IPv4-only).

    The IPv4 fallback carries no host cookie, so it is only used when the primary attempt could
    not connect at all -- which only happens in the login-failed run, where there is no session
    cookie to lose anyway.
    """
    try:
        r = await request_ctx.get(url, max_redirects=0)
        return r.status
    except Exception:  # noqa: BLE001
        try:
            r = await request_ctx.get(url.replace("apps.egret.localhost", "127.0.0.1"),
                                      max_redirects=0)
            return r.status
        except Exception as e:  # noqa: BLE001
            return f"error:{type(e).__name__}: {str(e).splitlines()[0][:120]}"


async def main() -> None:
    reset_events()
    console: list[dict] = []
    page_errors: list[str] = []
    async with async_playwright() as p:
        browser = await p.chromium.launch()
        page, frame = await open_app(browser, "alice", "xss", "run")
        page.on("console", lambda m: console.append(
            {"type": m.type, "text": m.text, "location": m.location}))
        page.on("pageerror", lambda e: page_errors.append(str(e)))

        # Detect login failure: with __Host- Secure cookies over http://*.localhost, Chromium may
        # refuse to store the session cookie, so the host page returns "login first" with no #app
        # iframe. Record that verbatim rather than crashing (it means P4 must redo this over HTTPS).
        login_ok = True
        login_note = None
        try:
            await page.locator("#app").wait_for(timeout=8000)
        except Exception:  # noqa: BLE001
            login_ok = False
            cookies = await page.context.cookies()
            try:
                body_txt = await page.locator("body").inner_text()
            except Exception:  # noqa: BLE001
                body_txt = ""
            login_note = {
                "reason": "no #app iframe on the egret host page",
                "host_page_body": body_txt[:300],
                "stored_cookies": [c["name"] for c in cookies],
                "interpretation": (
                    "Chromium refused to store the __Host- Secure cookie over "
                    "http://egret.localhost, so login produced no session; this check "
                    "must be repeated over HTTPS on kind in P4."
                ),
            }

        xss = html_script = onerror = None
        ticket = None
        replay_status: object = None
        if login_ok:
            # anywidget is the primary arbitrary-JS vector. Give it a bounded wait; if it never
            # renders, that (author JS does NOT execute) is itself the finding.
            xss = await grab(frame, "XSSRESULT", 60000)
            html_script = await grab(frame, "HTMLSCRIPT", 15000)
            onerror = await grab(frame, "ONERROR", 15000)

        await page.screenshot(path=str(OUT / f"q4{SUFFIX}.png"), full_page=True)
        try:
            frame_body = await frame.locator("body").inner_text()
        except Exception:  # noqa: BLE001
            frame_body = ""

        if login_ok:
            iframe_src = await page.locator("#app").get_attribute("src")
            ticket = iframe_src.split("t=")[1]
            # single-use ticket: it was already redeemed when the iframe loaded -> replay fails.
            replay_status = await req_status(page.context.request, f"{APPS_ORIGIN}/_ticket?t={ticket}")

        # a brand-new context has no apps_sid cookie -> the relay must refuse.
        no_cookie_ctx = await browser.new_context()
        apps_without_cookie_status = await req_status(no_cookie_ctx.request, f"{APPS_ORIGIN}/r/")

        # Probe-level version of the "run cookie must not reach edit" check: the browser context
        # holds the httponly apps_sid cookie (a RUN session). Hitting /e with it must be refused,
        # independently of whether the author JS executed.
        edit_status_code = await req_status(page.context.request, f"{APPS_ORIGIN}/e/api/status")
        edit_root_code = await req_status(page.context.request, f"{APPS_ORIGIN}/e/")

        await browser.close()

    evs = events()
    author_js_executed = xss is not None
    result = {
        "login_ok": login_ok,
        "login_note": login_note,
        "author_js_executed": author_js_executed,
        "author_js": json.loads(xss.removeprefix("XSSRESULT ")) if author_js_executed else None,
        "mo_html_script": html_script,
        "mo_html_onerror": onerror,
        "ticket_replay_status": replay_status,
        "apps_without_cookie_status": apps_without_cookie_status,
        "edit_from_run": {
            "note": "run-mode apps_sid cookie hitting the /e (edit) upstream via the proxy; must be 401/403",
            "api_status": edit_status_code,
            "root": edit_root_code,
        },
        "egret_saw_whoami": [e for e in evs if e["kind"] == "whoami"],
        "egret_saw_mutate": [e for e in evs if e["kind"] == "mutate"],
        "any_mutate_succeeded": any(e["kind"] == "mutate" and e["ok"] for e in evs),
        "console": console,
        "page_errors": page_errors,
        "frame_body_excerpt": frame_body[:2000],
    }
    (OUT / f"q4{SUFFIX}.json").write_text(json.dumps(result, indent=2))
    print(json.dumps({k: v for k, v in result.items()
                      if k not in ("frame_body_excerpt",)}, indent=2))


asyncio.run(main())
