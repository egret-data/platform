"""End to end: the real launcher and marimo, driven by Chromium through a token-injecting proxy."""

from __future__ import annotations

import re
import time

import httpx
import pytest

from harness import APPS, MARIMO_TOKEN, POD_TOKEN, ROOT, assert_env_has_no_secrets, viewer_page, wait_until

pytestmark = pytest.mark.e2e

APP = "33333333-3333-4333-8333-333333333333"
VERSION = "v-1"
TOKENS = {"alice": "tok-alice", "bob": "tok-bob"}
PID_LINE = re.compile(r"^pid=(\d+) viewer=(\w+)$", re.M)


def _seed_viewers(egret) -> None:
    for user, token in TOKENS.items():
        egret.session_tokens[token] = {"user": user}


def _text(page) -> str:
    return page.locator("body").inner_text()


def _wait_text(page, pattern: re.Pattern, timeout: float = 60) -> re.Match:
    return wait_until(lambda: pattern.search(_text(page)), timeout, f"{pattern.pattern!r} on the page", 0.25)


def test_run_mode_one_kernel_process_and_token_per_viewer(egret, launch, proxy, browser):
    """D7 + §4.1: each viewer's session has its own kernel process and queries with its own token."""
    _seed_viewers(egret)
    egret.versions[(APP, VERSION)] = {"source": (APPS / "viewer.py").read_text(), "version_no": 1}
    t0 = time.monotonic()
    server = launch("run", f"/r/{VERSION}", EGRET_APP_ID=APP, EGRET_VERSION_ID=VERSION)
    t_ready = time.monotonic() - t0
    base = proxy(server.port, TOKENS)

    pids: dict[str, int] = {}
    with viewer_page(browser, base, "alice") as alice, viewer_page(browser, base, "bob") as bob:
        pages = {"alice": alice, "bob": bob}
        t1 = time.monotonic()
        for page in pages.values():
            page.goto(f"{base}/r/{VERSION}/")
        for user, page in pages.items():
            m = _wait_text(page, PID_LINE)
            pids[user] = int(m.group(1))
            assert m.group(2) == user, f"{user}'s page shows viewer {m.group(2)}"
        t_pages = time.monotonic() - t1

        assert pids["alice"] != pids["bob"], f"one kernel process for both viewers: {pids}"
        assert server.proc.pid not in pids.values(), f"kernel ran in the server process {server.proc.pid}"
        # The kernels were launched through the D14 shim venv, not a uv sandbox or a thread.
        assert f"Using configured venv: {ROOT / 'venv-shim' / 'bin' / 'python'}" in server.output()
        # Neither the server (re-exec'd with a clean env) nor a viewer's kernel can read the secrets.
        server_ps = assert_env_has_no_secrets(server.proc.pid, POD_TOKEN, MARIMO_TOKEN)
        assert "egret_marimo.launcher --internal run" in server_ps
        for pid in pids.values():
            assert_env_has_no_secrets(pid, POD_TOKEN, MARIMO_TOKEN)

        # A reactive re-run: move each viewer's slider and wait for the dependent cell.
        for user, page in pages.items():
            _wait_text(page, re.compile(rf"slider=1 pid={pids[user]} viewer={user}"))
            page.get_by_role("slider").first.focus()
            page.keyboard.press("ArrowRight")
            _wait_text(page, re.compile(rf"slider=2 pid={pids[user]} viewer={user}"))

    queries = list(egret.queries)
    for user, token in TOKENS.items():
        mine = [q for q in queries if q["token"] == token]
        assert {q["user"] for q in mine} == {user}
        assert {q["pid"] for q in mine} == {pids[user]}, f"{user}'s token used from another kernel: {mine}"
        assert any(q["sql"] == "SELECT 2" for q in mine), f"no re-run query for {user}: {mine}"
    # Every query came from one of the two viewer kernels, with that viewer's token.
    assert {(q["pid"], q["token"]) for q in queries} == {(pids[u], t) for u, t in TOKENS.items()}

    print(
        f"\n[e2e run] server pid={server.proc.pid} kernel pids={pids} "
        f"server ready {t_ready:.1f}s, both pages rendered {t_pages:.1f}s, queries={len(queries)}"
    )


EDITED = re.compile(r"edited-(\w+)")


def _edit_connect_cell(page, marker: str) -> None:
    """Append ``  # <marker>`` to the connection cell's last line and save with Ctrl/Cmd+S."""
    editor = page.locator(".cm-content", has_text='egret.connect("db")').first
    editor.click()
    page.keyboard.press("ControlOrMeta+End")
    page.keyboard.type(f"  # {marker}")
    page.keyboard.press("ControlOrMeta+s")


def test_edit_mode_save_conflict_reload_recover(egret, launch, proxy, browser):
    """§5.1/§5.3: saves land in egret; a stale save is refused; reload recovers the latest draft."""
    _seed_viewers(egret)
    template = (APPS / "template.py").read_text()
    egret.drafts[APP] = {"source": template, "revision": 1}
    t0 = time.monotonic()
    server = launch("edit", "/e/pod-1")
    t_ready = time.monotonic() - t0
    base = proxy(server.port, TOKENS)
    draft = egret.drafts[APP]
    server_ps = assert_env_has_no_secrets(server.proc.pid, POD_TOKEN, MARIMO_TOKEN)
    assert "egret_marimo.launcher --internal edit" in server_ps

    with viewer_page(browser, base, "alice") as page:
        page.goto(f"{base}/e/pod-1/?file={APP}.py")
        page.locator(".cm-content", has_text='egret.connect("db")').first.wait_for()

        # 1. A save goes to egret as revision 2.
        _edit_connect_cell(page, "edited-1")
        wait_until(lambda: draft["revision"] == 2, 30, "the draft to reach revision 2")
        assert "edited-1" in draft["source"]

        # 2. Someone else saves revision 3.
        external = template.replace('egret.connect("db")', 'egret.connect("db")  # external-3')
        r = httpx.put(
            f"{egret.base_url}/api/apps/pod/apps/{APP}/draft",
            json={"source": external},
            headers={"authorization": f"Bearer {POD_TOKEN}", "if-match": '"2"'},
        )
        assert r.json() == {"revision": 3}

        # 3. This editor's next save is refused, visibly, and egret keeps revision 3.
        _edit_connect_cell(page, "edited-stale")
        toast = page.locator("[role=status], li[data-state=open]", has_text="Failed to save").first
        toast.wait_for()
        toast_text = toast.inner_text()
        assert "saved by someone else" in toast_text and "reload" in toast_text, toast_text
        assert (draft["revision"], draft["source"]) == (3, external)
        assert any(e["kind"] == "draft_conflict" for e in egret.events)

        # 4. Reload: the conflicted session is closed, not resumed; the editor shows revision 3.
        page.reload()
        page.locator(".cm-content", has_text="external-3").first.wait_for()
        assert "edited-stale" not in _text(page)

        # 5. The next save succeeds as revision 4, on top of revision 3.
        _edit_connect_cell(page, "edited-4")
        wait_until(lambda: draft["revision"] == 4, 30, "the draft to reach revision 4")
        assert "external-3" in draft["source"] and "edited-4" in draft["source"]
        assert "edited-stale" not in draft["source"]

    saves = [e for e in egret.events if e["kind"] == "draft_saved"]
    print(
        f"\n[e2e edit] server pid={server.proc.pid} ready {t_ready:.1f}s; "
        f"saves If-Match={[e['if_match'] for e in saves]}; "
        f"conflicts={sum(e['kind'] == 'draft_conflict' for e in egret.events)}; toast={toast_text!r}"
    )
