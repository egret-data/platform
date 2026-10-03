"""D24: with one run pod per (viewer, version), thread mode is safe.

The original reason run-mode kernels needed separate processes (the P3 forced-IPC patch) was
to stop DIFFERENT PEOPLE's sessions from sharing a Python interpreter's globals/`os.environ`.
With egret's run pods now keyed by `(user_id, version_id)` (D24 — see
`docs/en/plans/2026-10-03-data-apps-p5-run-mode.md`), a pod never holds two different people's
kernels; two sessions in one pod are always the SAME viewer's own tabs. This test is the
measurement that justifies defaulting run pods to marimo's own thread-mode kernel (no patch):
two sessions of one viewer must still each see their own request's `x-egret-app-token`, which
is the one thing thread mode could plausibly get wrong (if it read a stale/wrong request).

Promoted into the e2e suite by Task 5c-2 once `run_kernel_mode=thread` is the real toggle
(Task 5c-1); until then this runs against whatever `launcher.py` currently does for run mode.
"""

from __future__ import annotations

import re

import pytest

from harness import APPS, viewer_page, wait_until

pytestmark = [
    pytest.mark.e2e,
    pytest.mark.skip(
        reason="D24 Task 5c-1 has not landed yet: egret_marimo.launcher still forces the IPC patch "
        "unconditionally in run mode, so this test (which asserts thread mode IS the default) would "
        "fail against today's actual behaviour. It PASSED as a manual spike with the patch call "
        "commented out by hand — see docs/en/plans/2026-10-03-data-apps-p5-startup-result.md. "
        "Task 5c-2 removes this skip once the real run_kernel_mode=thread default exists."
    ),
]

APP = "44444444-4444-4444-8444-444444444444"
VERSION = "v-1"
PID_LINE = re.compile(r"^pid=(\d+) viewer=(\w+)$", re.M)


def _text(page) -> str:
    return page.locator("body").inner_text()


def _wait_text(page, pattern: re.Pattern, timeout: float = 60):
    return wait_until(lambda: pattern.search(_text(page)), timeout, f"{pattern.pattern!r} on the page", 0.25)


def test_two_tabs_of_one_viewer_never_cross_tokens_in_thread_mode(egret, launch, proxy, browser):
    egret.session_tokens["tok-solo"] = {"user": "solo"}
    egret.versions[(APP, VERSION)] = {"source": (APPS / "viewer.py").read_text(), "version_no": 1}
    server = launch("run", f"/r/{VERSION}", EGRET_APP_ID=APP, EGRET_VERSION_ID=VERSION)
    base = proxy(server.port, {"solo": "tok-solo"})

    pids = []
    with viewer_page(browser, base, "solo") as a, viewer_page(browser, base, "solo") as b:
        for page in (a, b):
            page.goto(f"{base}/r/{VERSION}/")
        for page in (a, b):
            m = _wait_text(page, PID_LINE)
            assert m.group(2) == "solo", f"wrong viewer rendered: {m.group(2)}"
            pids.append(int(m.group(1)))

    # thread mode: both of this one viewer's sessions run IN the server process, not a child.
    assert pids[0] == pids[1] == server.proc.pid, (
        f"thread mode was not in effect (pids {pids}, server {server.proc.pid}) — "
        "the forced-IPC patch may still be active"
    )

    queries = list(egret.queries)
    assert queries, "no queries recorded — the cell never ran"
    assert {q["token"] for q in queries} == {"tok-solo"}, (
        f"a session used a token other than the one viewer's own: {queries}"
    )
