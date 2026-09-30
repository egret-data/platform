# Data Apps S0 — result

**Run:** 2026-09-30 (one session, 16:44–20:03 +07), marimo 0.25.0, Playwright 1.52 / HeadlessChrome 136,
Python 3.12.11, macOS dev laptop. Branch `spike/marimo-s0` @ `03f7a9fd` (never merged); every artefact
path below is relative to `spikes/marimo-s0/` on that branch.
**Spec:** [`docs/en/specs/2026-09-30-marimo-data-apps-design.md`](../specs/2026-09-30-marimo-data-apps-design.md) §9
**Plan:** [`2026-09-30-marimo-data-apps-s0-spike.md`](2026-09-30-marimo-data-apps-s0-spike.md)

**How things were checked.** Every probe is an automated Playwright script (`probes/*.py`) driving
HeadlessChrome 136 against a fake egret (`fake_egret.py`, `:8700`, `egret.localhost`), the spike's
apps-origin proxy (`proxy.py`, `:8701`, `apps.egret.localhost`) and marimo servers on `:2718` (edit) and
`:2719` (run), all over plain http. Where a result rests on a screenshot, the screenshot was looked at by
the agent that ran the probe — written below as **agent-viewed screenshot**. No human inspected anything,
and nothing was checked by hand in a browser. Each question was measured **once**; every timing is a single
run on a macOS dev laptop under unmeasured load and is not a pod figure.

## Answers

| # | Question | Answer | Evidence | Design consequence |
|---|---|---|---|---|
| 1 | Header reaches the right kernel, incl. reactive re-runs | **Yes**, in thread and ipc modes. 2 viewers, first run then slider-driven reactive re-runs (slider values n=1..4; n=3 is missing for some viewer in each run — alice in `q1.json`, both in `q1-ipc.json`, bob in `q1-blocked.json` — keypress coalescing suspected): every alice page shows alice, every bob page bob; 14 gateway queries (thread), 12 (ipc), 0 rejected. | `results/q1.json` (thread, both sessions pid 544); `results/q1-ipc.json` (alice pid 13553, bob pid 13545); `results/q1-blocked.json` (same result with the final block list, 14 queries) | None. Query-parameter fallback not needed. |
| 2 | Process per run session | **thread:** 1 pid for 4 sessions, 5.24 s first render alone, 17.71 s max with 3 concurrent. **`--sandbox`:** 1 pid for 4 sessions (per server, not per session), 12.0 s max offline / 17.18 s online. **ipc (egret patch):** 4 pids for 4 sessions, 13.2 s cold, 32.9 s max with 3 concurrent; ≈ 80 MB RSS per kernel (79.9–81.3 MB). | `results/q2-thread.json`, `q2-sandbox.json`, `q2-sandbox-online.json`, `q2-ipc.json`, `q2-rss.txt`, `q2-sandbox-attempt1-error.txt` | D7 holds **only via egret's own patch** (force the IPC branch in `SessionImpl.create` + point `_get_venv_config` at the image interpreter). Pod-per-viewer fallback not triggered. Private API → pin + contract test. Per-pod cap (20) and cold-start UX must be re-set against these numbers. |
| 3 | Draft round-trip; what the author sees on 409 | Round-trip **works** (rev 1 → 2, marker in the draft). A stale save gets **409** and marimo shows a red **"Failed to save"** toast with egret's `detail` text; the save button stays unsaved; the edited buffer is kept. | `results/q3.json` (`save_responses` 200, 409, 409; `binds_after_external_put` 0); `results/q3-conflict.png` (agent-viewed screenshot); `results/q3-blocked.json` (same with the final block list) | Header-driven 409 **not needed**. Sync-agent fallback not triggered. New requirement: re-fetch on load must not reset a live editor's revision (see Findings). |
| 4 | Separate origin + ticket → cookie; author-JS reach | marimo edit + run **work** behind the proxy on `apps.egret.localhost` with `--base-url`, WebSockets and ticket → `apps_sid` cookie. Ticket single-use (replay **403**); no cookie → **401**; run cookie → `/e` **401**. Author JS **executes** (anywidget `_esm`); `mo.Html` `<script>` / `onerror` stay inert. From the apps origin JS cannot read the parent document (`SecurityError`), egret cookies (`""`) or egret responses (CORS `TypeError`); the header-bearing POST dies at preflight. **But** egret's SameSite=Lax cookies **ride** requests from the apps origin (egret logged `whoami` as `alice`), and a tossed `Domain=` `egret_csrf=evil` **reached egret** beside the real one. `any_mutate_succeeded=false`. **`__Host-` defence unmeasured.** | `results/q4.json`, `results/q4.png`; `results/q4-host-prefix.json` (`login_ok=false`, 0 cookies stored: Chromium refused `Secure` cookies over http); `results/smoke.png` | Architecture holds functionally — no revisit. §6.1 rewritten: same-site ambient authority is real → P4 requirements (GETs side-effect free, header-based CSRF, `__Host-` verified over HTTPS on kind). Open question to the user: a different registrable site for apps. |
| 5 | SQL cell accepts the DB-API connection | **Yes** — `mo.sql(..., engine=db)` ran through the gateway as the viewer. The engine dropdown lists `db` **only after** its defining cell has run (before: only "DuckDB (In-Memory)"). marimo's default SQL engine is DuckDB and errors without `duckdb` + `sqlglot`. | `results/q1.json` (`sql_cell_used_gateway: true`, `q5_dropdown`); `results/q5-dropdown.png`, `results/q5-dropdown-after-run.png` (automated Playwright capture; options also scraped into `q1.json`) | No custom engine class. Open design point for P3: ship duckdb in the image, or make the egret engine the SQL-cell default. |
| 6 | Block list | 107 routes inventoried; **22 prefixes** blocked (list below); edit, save, 409 and run unchanged with them. Terminal WS 403 on `/e` and `/r`. Dot-segment bypass found — of 40 raw variants: **3** HTTP GETs 200 (reached marimo), **16** WS forms 101 from the proxy but upstream-rejected (no shell), **2** POSTs reached marimo (401 from marimo), rest stopped or unattributed — and fixed (**0 of 40**: 28 × 403, 12 × 400). | `results/q6.json`, `q6-routes.json`, `q6-traffic.json` (260 distinct paths, 0 unmatched), `q6-terminal.json`, `q6-terminal.png`, `q6-bypass.json`, `q6-bypass-fixed.json`, `q1-blocked.json`, `q3-blocked.json` | §6.2 block list for P4; proxy checks and forwards one raw path; P4 should prefer an allow-list; strip/pin `X-Notebook-Id`. |

### Question 6 — the final block list

`/terminal`, `/api/packages`, `/api/secrets`, `/api/files`, `/api/ai`, `/api/storage`, `/api/home`,
`/api/kernel/copy`, `/api/kernel/rename`, `/api/kernel/install_missing_packages`,
`/api/kernel/save_user_config`, `/api/export/auto_export`, `/api/export/requirements`, `/api/lsp`,
`/api/datasources`, `/api/cache`, `/api/environment`, `/api/usage`, `/api/sessions`, `/api/status`, `/auth`,
`/og` — 22 prefixes covering 62 of the 107 routes (`results/q6.json` `S0_BLOCKED`, `block_list_reasons`).

The spike README's T7 line and `q6.json`'s `kernel_routes_that_create_or_move_files` still show the first
13-prefix list and call `save_user_config` "left open"; both predate the fix round. The 22-prefix list
above, `S0_BLOCKED` in `q6.json`, and `results/q6-terminal.json` (`POST /e/api/kernel/save_user_config`
→ 403) are the final state.

Requests the editor makes on its own that the list turns into 403, with no functional effect on editing
(`results/q3-blocked.json` run): `POST /api/packages/sandbox`, `GET /api/lsp/health`, `/api/usage`
(footer bars stop), `POST /api/datasources/discover` (datasource panel empty). The settings panel can no
longer persist changes (`save_user_config` 403).

## Findings

**Q1 — per-command request context holds across processes.** The proxy-injected header reached the
triggering session's kernel on the first run and on every slider-driven reactive re-run, in thread mode
(one pid, `results/q1.json`) and across the ipc subprocess boundary (`results/q1-ipc.json`). Scope: the
fake egret's gateway events carry user and pid but no session id, so "never crossed" rests on the page-text
booleans plus the per-user query log. marimo accepted the proxy's `Authorization: Bearer <pod token>` on
HTTP and on the WS upgrade (`marimo/_server/api/auth.py:77-86`), and `--base-url /r` needed no path
rewriting.

**Q2 — `--sandbox` is per server, not per session.** On a single-file `marimo run`, `--sandbox` goes to
`run_in_sandbox` (`marimo/_cli/cli.py:1249-1253`), which re-execs the whole server inside a uv env with
`--sandbox` stripped (`marimo/_cli/sandbox.py:461`); the inner server is thread mode, hence
`distinct_pids=1` (`results/q2-sandbox.json`). It also re-execs from `sys.argv[1:]` (an entrypoint must set
`sys.argv` first, else `error: unexpected argument '--mode'`) and the env holds only marimo, so every app
import must be a PEP 723 dependency (`results/q2-sandbox-attempt1-error.txt`:
`ModuleNotFoundError: No module named 'httpx'`). Offline only worked with a warm uv cache; cold-cache
offline is untested.

**Q2 — the IPC branch needs two patches to private API.** `Session` is a Protocol; the patch target is
`marimo._session.session.SessionImpl.create` (`session.py:91,99`), forced to `sandbox=True` so it selects
`IPCKernelManagerImpl` (`session.py:160`). In 0.25.0 that manager has no `_sandbox` switch: without a
configured venv it always builds a uv `NotebookSandbox` (`marimo/_session/managers/ipc.py:331-431`). The
spike patches `ipc._get_venv_config` (`ipc.py:65`) to return `path = sys.prefix`; the log then prints
`Using configured venv: …/.venv/bin/python` once per session and no uv env is built. Not done through
`marimo.toml`, because a configured venv would also silently turn `--sandbox` into the ipc variant
(`ipc.py:333`). All ipc kernels share the image's site-packages. Cost (`results/q2-ipc.json`,
`q2-rss.txt`): first session 13.24 s to first render against 5.24 s in thread mode; three concurrent
spawns 32.73–32.90 s against 16.62–17.71 s in thread mode (thread mode shows the same concurrency penalty,
so part of it is laptop contention); 79.9–81.3 MB RSS per kernel plus a 77 MB server and an 11.7 MB
multiprocessing resource tracker, against 103.8 MB for a whole thread-mode server with 4 sessions. macOS
RSS, single run.

**Q3 — the frontend does report a failed save.** The 409 from `EgretStorage.write` surfaces from
`POST /e/api/kernel/save` with egret's JSON `detail`, and marimo 0.25.0 renders it as a red toast "Failed to
save / This draft was saved by someone else — reload." (`results/q3-conflict.png`, agent-viewed
screenshot; `results/q3.json` `ui_mentions_conflict: true` is a keyword match over the iframe body text,
the toast-selector scrape returned `""`). The spec's §2 claim that the frontend only leaves the notebook
"unsaved" was wrong. One Ctrl+S produced two POSTs and two `draft_conflict` events (unexplained; autosave
suspected, not investigated); toast auto-dismiss timing was not measured.

**Q3 — hazard: re-fetch on load resets the revision.** `NotebookWorkspace.load` runs twice per editor open
— for the index page (`marimo/_server/api/endpoints/assets.py:387` → `session_manager.py:191`) and for the
session (`session_manager.py:353`) — and the spike's storage patch re-fetches the draft and rebinds its
revision on each. A reload, a second tab or an index fetch in an editor that holds an older revision would
therefore silently adopt the newer one, and its next stale save would **succeed** and overwrite the other
author. The probe ruled this out for its own measurement (`storage_binds.binds_after_external_put: 0`), so
the 409 above is genuine; the hazard is a P3 design requirement, not a measured failure.

**Q4 — same-site is not isolation from egret.** `apps.egret.localhost` and `egret.localhost` share the
registrable domain, so the browser treats a request from the apps origin to egret as same-site: the victim's
SameSite=Lax egret cookies were sent, and the fake egret logged `whoami` as `alice` with origin
`http://apps.egret.localhost:8701` (`results/q4.json` `egret_saw_whoami`). CORS only stopped the script
from reading the reply. The widget's `document.cookie = "egret_csrf=evil; domain=egret.localhost; path=/"` (`apps/xss.py:34`) was
accepted and delivered to egret beside the real value (`egret_saw_mutate[0].raw_cookie` holds two
`egret_csrf`). No mutate succeeded, only because the no-cors POST cannot carry `x-egret-csrf` and the
header-bearing variant failed the CORS preflight. Scope: the egret side is `fake_egret.py`, which models
egret's host-only, SameSite=Lax, double-submit cookies; it is not egret's code.

**Q4 — the `__Host-` defence could not run on http.** With `COOKIE_PREFIX=__Host-` the fake egret sets
`Secure` cookies; HeadlessChrome 136 stored none over `http://egret.localhost` (login 200 per the Task 6 report, `context.cookies()` `[]`;
`login_ok: false`,
host page "login first"), so no victim session formed and nothing was measured
(`results/q4-host-prefix.json`). Whether a `Domain=` toss can shadow `__Host-egret_csrf` is **unmeasured**.
The widget-side `__Host-egret_csrf=evil` toss not taking has two equally good explanations (the `Domain`
attribute is invalid for `__Host-`; Chromium refuses `Secure` cookies over http) and is not evidence.

**Q4 — author JS runs through anywidget, not `mo.Html`.** An anywidget `_esm` module executed
(`author_js_executed: true`); a `<script>` and an `<img onerror>` in `mo.Html` stayed "pending"
(`mo_html_script`, `mo_html_onerror`). The iframe used
`sandbox="allow-scripts allow-same-origin allow-forms allow-downloads"` (`fake_egret.py:68-70`).

**Q5 — engine discovery is by live variable.** `DBAPIEngine.is_compatible` duck-types
`cursor/commit/rollback/close` (`marimo/_sql/engines/dbapi.py:95-107`) and accepted the plain connection.
The dropdown is built from kernel variables (`marimo/_sql/get_engines.py:48`), so `db` appears only once
its cell has run. A SQL cell with no engine uses `DuckDBEngine(connection=None)` after requiring `duckdb`
and `sqlglot` (`marimo/_sql/sql.py:112-118`, deps at `sql.py:36-45`); the spike venv has neither, so a new
default SQL cell errored (`results/q1.json` `q5_dropdown.after_run_all.note`).

**Q6 — a deny-list is only as good as its path normalisation.** The first proxy matched
`request.url.path`, then httpx collapsed dot segments before forwarding, so check and forward diverged:
`GET /e/x/../api/packages/list`, `/e/x/%2e%2e/…` and `/e/x/..%2f…` returned **200** (package list), and 16
WebSocket forms of `/terminal/ws` (`x/../`, `./`, `//`, `Terminal`, `%2e%2e`, `x/%2e%2e`, `..%2f`, `%2e`, on
`/e` and `/r`) got **101 from the proxy**; the shell was not reached only because marimo refused the
un-collapsed upstream URL. In addition `POST /e/x/../api/files/list_files` and its `/r` twin passed the
check and reached marimo, which answered 401 (missing server token — a raw client lacks it; an author has
it from the served page). The `/r` dot-form `GET`s got 401 (one captured body is marimo's "Authorization
header required", the other two have none) and the `//` / `API` forms 404; those are not attributed.
So of 40 variants: 3 × 200 reaching marimo, 16 × 101 from the proxy with no shell, 2 POSTs reaching
marimo, the rest stopped or unattributed (`results/q6-bypass.json` and its `ws_note`). Fixed by
`forwarded_path()` (`proxy.py:31-40`): one value, the raw path the client sent, is both matched
(percent-decoded, case-insensitive) and forwarded; dot segments (also after one decode), `//`, encoded
`.` `/` `\` `%`, backslash and NUL are rejected (HTTP 400, WS 403). After: 0 of 40 variants pass — 28 × 403,
12 × 400 (`results/q6-bypass-fixed.json`). ASGI's `raw_path` is optional; the spike fails closed without it.
Browsers normalise paths themselves, so the Playwright terminal probe (`results/q6-terminal.json`) could
not see this; the raw-socket probe `probes/q6_bypass.py` did.

**Q6 — what the list cannot block.** `/api/kernel/save` is needed and accepts a `filename` (save-as),
confined only by marimo's inside-workspace check (`marimo/_server/api/endpoints/files.py:152-180`), so a
rogue save can create or overwrite files inside the pod's workspace directory. `/public/{path}` picks the
notebook directory from a client-supplied `X-Notebook-Id` header (`marimo/_server/api/endpoints/assets.py:712-750`);
the proxy must strip or pin it. `/api/kernel/execute` and the scratchpad run code with the same power as a
cell. `/@file/…` virtual files are session-global with unguessable names only. A marimo upgrade can add
routes: re-run `probes/q6_routes.py` on every upgrade.

**Config lookup.** `XDG_CONFIG_HOME` is the last place marimo looks: a `.marimo.toml` in the working
directory, any parent, or `~` wins first (`marimo/_config/utils.py:69-120`). The image must guarantee none
exists.

**Harness note (not a marimo or proxy defect).** The proxy vanished twice mid-task; the cause was the
harness's `lsof -ti :8700 | xargs kill -9`, which also matched the proxy's pooled client socket. An earlier
note that the proxy "did not survive a WebSocket session lifecycle" was withdrawn.

## Fallbacks

| Question | Fallback in the spec | Triggered? |
|---|---|---|
| 1 | Token by query parameter | No — `results/q1.json`, `q1-ipc.json` |
| 2 | One pod per (app × viewer) | No — ipc patch gives 4 pids for 4 sessions (`results/q2-ipc.json`); but `--sandbox` failed (`q2-sandbox.json`), so D7 rests on egret's patch |
| 3 | File-watching sync agent; 409 in the egret header | No — `results/q3.json`, `q3-conflict.png` |
| 4 | Revisit the architecture | No — functional pass (`results/q4.json`); security caveats move to P4 and one open question |
| 5 | Custom marimo engine class | No — `results/q1.json` `sql_cell_used_gateway: true` |

## What the spec now says

Applied in the same commit, `en` and `vi`:

- **D7, §2, §6.3** — `--sandbox` isolates per server; one process per session is egret's IPC patch on
  private API (pin + contract test); measured cost; the 20-session cap and cold-start UX must be re-set in
  P5.
- **§2, §5.1, §10** — marimo shows the failed save; the header-driven 409 fallback is dropped; re-fetch on
  load must not reset a live editor's revision.
- **§4.1** — S0 confirmed, not "S0 confirms".
- **§4.4** — dropdown after the defining cell ran; the DuckDB default is an open P3 design point.
- **§6.1** — author JS via anywidget; what crossed and what did not; `__Host-` unmeasured; P4 requirements;
  the different-registrable-site alternative as an open question.
- **§6.2** — the 22-prefix list, one raw path checked and forwarded, allow-list preferred, `X-Notebook-Id`,
  save-as.
- **§9** — S0 done, with a "What happened" column beside each fallback; P3, P4 and P5 items gain the work above. **§10** gains tests for the reload case, the path-normalisation variants and N sessions → N kernels, and splits the app-JS criterion into three measurable ones (cannot read egret's cookies or responses; cannot complete a state change — no side-effect GETs, header-based CSRF; a tossed `__Host-` cookie never reaches egret, over HTTPS on kind); **§11** drops "cannot reach egret's API", which S0 showed false for requests.

## Open questions for the user

1. **Serve apps from a different registrable site?** (e.g. `egret-apps.example` rather than
   `apps.egret.example`). It would stop egret's Lax cookies riding app requests and make cookie tossing
   impossible. Known cost: the iframe becomes cross-site, so its apps-origin cookie is a third-party cookie,
   which browsers block — the ticket → cookie scheme would need a replacement (not measured). Not decided.
2. **SQL-cell default engine** — ship `duckdb` + `sqlglot` in the image, or make the egret engine the
   default in `egret_marimo`? Left to P3 unless the user decides now.
