# Data Apps on marimo — replacing the notebook

**Status:** designed; S0 and S0b spikes done (§9)
**Date:** 2026-09-30
**Replaces:** the Jupyter notebook (`egret-notebook`, the Jupyter half of `egret-kernel`, `kernel-image/`,
`frontend/src/components/notebook/`)
**Delivered as:** two spikes (S0, S0b) and six PRs (P1–P6), §9.

The notebook changes purpose. It is no longer where a user does ad-hoc analysis — SQL Lab is that. It
becomes where an author builds a **highly custom dashboard in Python** (a *Data App*), in the style of
marimo or Streamlit, and publishes it for others to use. Metabase-style dashboards remain the no-code
option; Data Apps are the code-first one.

The runtime is [marimo](https://github.com/marimo-team/marimo) (Apache-2.0, 0.25.0 at time of writing):
its edit mode is the authoring surface, its run mode is the published app.

## 1. Decisions taken with the user

| # | Decision | Consequence |
|---|---|---|
| D1 | **Replace, no conversion.** Existing notebooks are dropped. | No importer; P6 deletes the old feature outright. |
| D2 | **Data access only through SQL** on the workspace's engines (Trino, StarRocks). No Iceberg read/write, no catalog token, no Daft. | The kernel image loses `pyiceberg`, `daft`, `write_table`, the `aud:catalog` JWT. |
| D3 | **An app runs with the viewer's permissions**, never the author's. | The app's code is untrusted with respect to the viewer; see §3, §6. |
| D4 | **Viewers are signed-in egret users** — workspace members and share-guests. No external embedding, no public links. | Every viewer has an identity to delegate. |
| D5 | **Draft + immutable published versions.** Latest version is live; restore = publish an old source again. | Append-only `app_versions`. |
| D6 | **SQL from an app is read-only.** | Closes the confused-deputy hole (an author's `DELETE` running as an admin viewer). |
| D7 | **Run mode: one pod per app version, one process per viewer session.** Never threads. S0 showed marimo's `--sandbox` does not give this (it isolates per server); egret's entrypoint forces marimo's IPC kernel branch instead (§2, §6.3). S0b: the per-viewer kernel can come from a pre-started process (a warm pool or a fork server) behind the interpreter marimo is configured to launch; which one is decided in P5 on Linux (§6.3). Fallback if the patch stops working on an upgrade: one pod per (app × viewer). | Viewer sessions share no memory. The entrypoint patches private marimo API: exact version pin + contract test. A pre-started launcher's kernel end runs `marimo._ipc.launch_kernel`, whose module path marimo documents as public; its server end still needs the private patches. |
| D8 | **Edit mode: one pod per (author × workspace), optimistic concurrency** (revision + 409). No locks, no real-time collaboration. | Every kernel runs as exactly one person. |
| D9 | **Fixed, curated Python image.** No runtime package install. | Egress can be closed to egret only. |
| D10 | **"Notebooks" becomes "Apps"** in the Explore menu; permissions `app:read`, `app:write`, `app:manage`. | `notebook:*` is removed. |
| D11 | **marimo's own UI in an iframe**, served by an egret reverse proxy. Not a fork of its frontend, not a custom UI over its protocol. | The iframe does not use egret's OKLCH design; theme (light/dark) is passed by query param. |
| D12 | **The apps origin is a sibling subdomain** (`apps.<egret host>`), same-site with egret (2026-10-01). | P4 must ship `__Host-` cookies (verified over HTTPS), side-effect-free GETs, header-based CSRF, no CORS for the apps origin (§6.1). |
| D13 | **SQL cells: ship DuckDB, egret engine is the default** (2026-10-01). | DuckDB for SQL over fetched DataFrames, in-pod only; a new SQL cell targets the gateway (§4.4). |
| D14 | **marimo's per-launch `has_marimo_installed` check is answered by the launcher shim** (2026-10-01). | A hack against private marimo behaviour: pinned marimo, a contract test in the image's CI that fails when the command changes; no upstream change pursued for now (§6.3). |
| D15 | **P5 start-up acceptance is relative to thread mode** (≤ 0.5 s median, ≤ 1 s for 3 concurrent, Linux, ≥ 5 runs); "< 2 s" is a separate, non-blocking experience goal (2026-10-01). | The ≈ 1.75 s page-load floor is outside the kernel launcher's reach (S0b). |
| D16 | **The pool shutdown hang is covered by a P5 acceptance test**, not a separate spike (2026-10-01). | Applies to whichever launcher P5 picks; k8s restarts make this order routine. |

## 2. What the code says

Read before designing; each of these shaped the design.

- **Execution-as-a-user already exists.** `QueryExecPort::execute_as(ws, database_id, principal, sql,
  limits)` (`crates/egret-query-api/src/port.rs:158`) runs SQL under a principal's delegated identity so
  Ranger evaluates *that* user, with `Limits { max_rows, timeout }` and a `truncated` flag. The Data API
  already serves through it. The gateway (§4) is a new caller of this port, not a new execution path.
- **The port is fail-closed on workspace scoping** (`database_id` not in `ws` → `NotFound`) but leaves
  the *permission* check to the caller. The gateway must evaluate `query:run` itself.
- **The current kernel holds engine JWTs.** `EGRET_NOTEBOOK_RUNTIME` carries `engine_jwt` and
  `catalog_jwt` into the pod (`crates/egret-notebook/src/runtime.rs:77`). Acceptable when the only code in
  the kernel is the user's own; not acceptable once an author's code runs for a viewer (D3). The new pods
  hold **no engine credential at all**.
- **There is no SQL parser in the workspace.** `sqlparser` is a new dependency (§4.3).
- **marimo's `create_asgi_app` is run-mode only** (`marimo/_server/asgi.py:354`, "This only works for
  application that are in Run mode"). Edit mode is served only by the `marimo edit` server, so it cannot
  be mounted in egret; it must run in a pod.
- **marimo's run mode puts sessions in threads** of one process (`marimo/_session/managers/kernel.py`;
  the choice is `marimo/_session/session.py:136-195`). An author's code could then pass one viewer's rows
  to another through module state. D7 requires one process per session. **marimo has no built-in path
  to that in run mode** (S0 question 2): on a single-file `marimo run`, `--sandbox` re-execs the *whole
  server* inside one uv environment (`marimo/_cli/cli.py:1249-1253`, `_cli/sandbox.py:461`), and the
  inner server is thread mode again — 4 sessions, 1 pid. The ZeroMQ subprocess manager
  (`managers/ipc.py`) is reachable only by patching two private functions from the egret entrypoint:
  `SessionImpl.create` (`marimo/_session/session.py:91,99`) forced onto the IPC branch
  (`session.py:160`), and `ipc._get_venv_config` (`ipc.py:65`) pointed at the image's interpreter,
  because otherwise the IPC manager builds a uv environment per session (`ipc.py:331-431`). With both,
  S0 measured 4 sessions → 4 kernel processes. (`isolate_apps` is not it: it isolates per *file*, not per
  session.)
- **marimo's auth is one shared token** (`marimo/_server/api/auth.py`), but it keeps an outer
  `scope["user"]`, and notebook code sees the request through `mo.app_meta().request` (headers, cookies,
  `user`, `meta`). **The request is refreshed per kernel command**, not captured once: it is a
  contextvar set around each command (`marimo/_runtime/kernel_request_handlers.py`), taken from the
  WebSocket handshake for the first run (`ws_session_connector.py:262`) and from every later
  `/run`, `/set_ui_element_value`, `/instantiate` call (`endpoints/execution.py`). Headers beginning
  `marimo` / `x-marimo` are dropped; `X-Egret-*` passes. That is the per-session channel into a kernel.
- **marimo has a storage seam, but it is private and covers only writes.** `StorageInterface`
  (`marimo/_session/notebook/storage.py:15`) is accepted by `AppFileManager(storage=…)`
  (`file_manager.py:69`), which the server builds in one place (`NotebookWorkspace.load`,
  `marimo/_server/workspace/_base.py:87`). But loading reads the file from disk directly
  (`load.load_app(path)`, `file_manager.py:285`), and only the final write of a save goes through
  `storage.write` (`file_manager.py:266`). A save that raises becomes an HTTP error from
  `POST /api/kernel/save`, and marimo's frontend **does** report it: S0 question 3 saw a red "Failed to
  save" toast carrying the error's `detail` text, the save button left unsaved and the edited buffer
  kept. (An earlier reading of `frontend/src/core/saving/save-component.tsx:111` concluded the frontend
  was silent; the measurement contradicts it.)
- **marimo's configuration cannot switch off** the terminal, package installation, the file browser or
  secrets; only AI has a switch (`[ai] enabled`). There is no `MARIMO_CONFIG` variable — config is found
  via `XDG_CONFIG_HOME` or the working directory (`marimo/_config/utils.py`).
- **marimo does no Origin, Host or CSRF check** and sets no `X-Frame-Options`/CSP on app pages; it
  requires a `Marimo-Server-Token` header on POSTs (skew protection), which a proxy must pass through.
- **egret's CSRF is double-submit on host-only cookies** `egret_session` / `egret_csrf`, SameSite=Lax,
  no `__Host-` prefix (`crates/egret-auth/src/cookie.rs`, `csrf.rs`), and egret has no CORS layer. A
  sibling subdomain can *toss* a `Domain=`-scoped cookie at egret, and requests it sends to egret carry
  egret's Lax cookies — S0 measured both (§6.1).
- **marimo SQL cells find engines from notebook variables** (`marimo/_sql/get_engines.py:35`), including
  any DB-API 2.0 connection. An object the helper package creates is enough for a SQL cell to offer it.
- **The notebook gate and permissions to retire:** the `notebooks` feature module
  (`crates/egret-workspace/src/features.rs:227`, prefixes `notebooks`, `notebook-runtime`,
  `notebook-settings`); `notebook:read` / `notebook:write` (`crates/egret-core/src/rbac.rs:394`). There
  is no `notebook:manage`; share management borrows `notebook:write` today.
- **The workflow `egret.python` task references a notebook** (`crates/egret-workflow/src/spec/registry.rs:90`)
  but has no handler. P6 must decide its fate (§9).

## 3. Architecture

```text
Browser — egret origin (e.g. egret.example)
  └─ /explore/apps/{id}        egret header  +  <iframe src="https://apps.egret.example/r/{version}/?ticket=…">
Browser — apps origin (apps.egret.example), inside the sandboxed iframe
  └─ marimo UI (edit or run)
         │  HTTP + WebSocket
egret server
  ├─ App store (Postgres)   apps · app_drafts(revision) · app_versions(append-only) · app_sessions
  ├─ App proxy (apps host)  ticket→cookie · authz · mint session token · inject headers · HTTP/WS relay
  ├─ App launcher (k8s)     edit pod per (author × ws) · run pod per app version · reaper
  └─ Query gateway          POST /api/apps/query — session token → execute_as(viewer), read-only
         ▲
marimo pod — image `egret-apps`, egress ONLY to egret
  ├─ edit: egret entrypoint → marimo edit server, EgretStorage, kernel process per notebook
  ├─ run:  egret entrypoint → marimo run server, kernel process per viewer session
  └─ egret_marimo (Python): DB-API connection + egret.sql() → gateway with the session token
```

Two invariants hold everywhere:

1. **No pod holds a credential to an engine.** All data access goes through the gateway with a token
   bound to (user, app, version-or-draft, mode), read-only, short-lived.
2. **The `.py` source lives only in Postgres.** A pod's disk is a disposable copy.

## 4. Query gateway and session tokens

### 4.1 Session token

An opaque random token; only its hash is stored, in `analytics.app_sessions`:

| Column | Meaning |
|---|---|
| `token_hash` | PK |
| `user_id`, `workspace_id`, `app_id` | who, where, which app |
| `version_id` nullable | the pinned published version (run); `NULL` = the draft (edit) |
| `mode` | `edit` \| `run` |
| `login_sid` | egret login session; back-channel logout revokes by it |
| `created_at`, `last_used_at`, `expires_at` | idle expiry 30 min, absolute 8 h |
| `revoked_at` | set on WebSocket close, logout, or explicit revoke |

- **The proxy mints it** when a marimo session is opened (§6.2), and injects it as `X-Egret-App-Token`
  on every request it forwards to the pod. Any `X-Egret-*` header arriving from the client is removed
  first. The browser never sees the token.
- The kernel reads it through `mo.app_meta().request.headers`, which marimo refreshes on every kernel
  command (§2), so the token a cell sees is the one on the request that triggered it. S0 question 1
  confirmed this through the proxy, including for cells re-run reactively by a UI element, with sessions
  in threads and with sessions in separate IPC kernel processes (two viewers, 14 and 12 gateway queries,
  none crossed).
- A token is not a permission. **Every gateway call re-evaluates authorization** (§4.2); removing a
  viewer's `app:read` stops their next query, not their next session.

### 4.2 `POST /api/apps/query`

Authenticated by `Authorization: Bearer <session token>`; exempt from CSRF (no cookie is involved).

```json
{ "database": "trino_prod", "sql": "SELECT …", "params": [], "max_rows": 10000 }
```

Steps, in order, each failing closed:

1. Look the token up by hash; reject if absent, revoked or expired (401). Touch `last_used_at`.
2. Authorize the token's user in the token's workspace: the workspace has the `apps` feature enabled
   (the path carries no `/ws/{ws}`, so the feature gate cannot infer it — the handler checks it),
   `app:read` on the token's app in run mode; `app:write` **and** `query:run` in edit mode. Run mode
   does not require `query:run`: `ws_viewer` lacks it, the viewer cannot write SQL (the token never
   reaches the browser; only the author's code sends queries), and Ranger still decides per viewer.
3. Resolve `database` **by name** within the token's workspace (404 if absent — same as denied).
4. Read-only check (§4.3); reject with 400 and the reason.
5. `QueryExecPort::execute_as` (or `execute_as_with_params`) as the token's user, with
   `max_rows = min(requested, configured cap)` and the configured timeout.
6. Audit: app, version (or draft), user, database, statement hash, row count, duration, outcome.

Response: `TabularResult` as JSON (columns, rows, `truncated`). Arrow is a later optimisation.

Errors keep the port's distinctions: `Forbidden` → 403 with the engine's message (the viewer lacks a
grant — expected under D3); `NotFound` → 404; `InvalidInput` → 400; `Internal` → 500 without engine
addresses; expired token → 401, which the helper turns into "session expired — reload the app". Each
session has a rate limit (configurable; as built in P1: 20 calls per 2 s window, and a call over the limit
holds the token for one further window).

### 4.3 Read-only check

Parse with `sqlparser` using the dialect closest to the engine (Trino → `GenericDialect`, StarRocks →
`MySqlDialect`). Accept only when the input is **exactly one** statement of kind `Query`, `Explain`,
`ShowTables`/`ShowColumns`/`ShowSchemas`/`ShowCatalogs`/`ShowCreate`, or `Describe`, and the `Query`
contains no data-modifying CTE. **Anything the parser cannot parse is rejected** — a query the parser
does not understand is not known to be read-only. Engine-specific syntax that fails to parse is a known
cost; the error says so, so the author can rewrite.

This is defence in addition to Ranger, not instead of it. Where an engine offers a read-only session or
transaction mode, P1 measures whether the delegated connection can use it and records the result.

### 4.4 The Python side — `egret_marimo`

Installed in the `egret-apps` image:

- `egret.connect("trino_prod")` → a **DB-API 2.0** connection whose cursor calls the gateway. marimo
  recognises it as an engine (S0 question 5: `mo.sql(…, engine=db)` ran through the gateway as the
  viewer). The SQL cell's engine dropdown is built from live kernel variables, so the connection appears
  there **only after the cell that defines it has run**; before that the dropdown offers only "DuckDB
  (In-Memory)".
- **marimo's default SQL engine is DuckDB**, and a SQL cell with no engine errors unless `duckdb` and
  `sqlglot` are installed (`marimo/_sql/sql.py:112-118`). **Decided with the user (2026-10-01, D13): both.** The
  image ships `duckdb` + `sqlglot`, so authors can run SQL over DataFrames already fetched (in-pod only;
  egress is closed), **and** `egret_marimo` makes the egret engine the default for a new SQL cell, so it
  targets the gateway under the viewer's identity. Unmeasured: whether marimo lets a library set the
  default SQL engine; P3 finds out, and if it cannot, the new-app template pre-creates the connection
  cell so the egret engine is listed and selected from the start.
- `egret.sql("SELECT …", database="trino_prod")` → a polars DataFrame, for Python code.
- `egret.databases()` → the names the viewer may query, for pickers.
- Databases are referenced **by name**, so an app carries no internal UUIDs.

Consequence of D3, stated so nobody reports it as a bug: **the same app shows different results to
viewers with different grants**, and a cell can fail with 403 for one viewer and succeed for another.

## 5. Storage and lifecycle

P2 adds these tables; P6 drops the notebook's tables (from migrations 0037–0043, 0090, 0091) with no
data migration (D1):

| Table | Columns |
|---|---|
| `analytics.apps` | `id`, `workspace_id`, `name`, `description`, `folder_id`, `owner_user_id`, `created_at`, `updated_at`, `deleted_at` |
| `analytics.app_drafts` | `app_id` PK/FK, `source text` (≤ 1 MiB, CHECK), `revision bigint`, `updated_by`, `updated_at` |
| `analytics.app_versions` | `id`, `app_id`, `version_no` (UNIQUE per app), `source`, `from_revision`, `published_by`, `published_at` |
| `analytics.app_sessions` | §4.1 |

- **`app_versions` is append-only by construction**: a trigger raises on `UPDATE` and `DELETE`. Deleting
  an app is a soft delete on `apps`; versions remain.
- **Draft API.** `GET /api/ws/{ws}/apps/{id}/draft` returns `source` and `revision` (ETag).
  `PUT` requires `If-Match: <revision>`; on mismatch → **409** with the current revision; on success the
  revision increments in the same statement (`UPDATE … WHERE revision = $n RETURNING`).
- **Publish** copies the draft at its current revision into `version_no + 1`. The egret header shows
  "Publish draft rev N" so the author knows exactly what is published. There is no validation at publish
  in v1; a broken version is undone by restore.
- **Restore** does not move a pointer: it publishes the old version's source as a new version. History
  stays linear, and "latest version is live" (D5) stays true.
- Folders, trash and bookmarks carry over from the notebook's generic implementations.

### 5.1 Edit mode: `EgretStorage`

The pod does not run `marimo edit` directly. It runs an egret entrypoint that starts the marimo edit
server with an `EgretStorage` implementing marimo's `StorageInterface`:

- **On open**, the entrypoint's patched `NotebookWorkspace.load` fetches the draft (`GET …/draft`),
  writes it to the pod's disk, remembers the revision, and only then builds the `AppFileManager` —
  because marimo loads from disk, not through the storage.
- **Re-fetch on load must not reset the revision a live editor holds.** `NotebookWorkspace.load` runs
  twice per editor open (index page and session, `marimo/_server/api/endpoints/assets.py:387`,
  `session_manager.py:191,353`) and again on reload, a second tab or an index fetch. The S0 patch
  re-fetched the draft and rebound the revision on every call; an editor holding revision *n* would then
  silently adopt *n + 1* and its next stale save would succeed, overwriting the other author. P3 binds
  the revision to the editing session (or refuses to rebind while a session holds an older one) and
  tests exactly this sequence.
- `write(path, source)` → `PUT …/draft` with `If-Match`, then writes the local copy. A 409 raises with
  the message as the error `detail`.
- `exists`, `rename` → map to the app; rename is refused (names change in egret, not in marimo).
- **The 409 is visible in marimo itself** (S0 question 3): the author sees a red "Failed to save" toast
  with egret's text ("This draft was saved by someone else — reload."), the save button stays unsaved,
  and the edited buffer is kept. No egret-header fallback is needed for the conflict. The header's
  draft-revision poll stays only if "Publish draft rev N" needs it.
- The storage authenticates as the pod's author (the per-pod kernel token that exists today) and egret
  checks `app:write` on every call.

`StorageInterface` is **private** to marimo, so: the image pins an exact marimo version; the image's CI
has a contract test (save, reload, 409) against it; if an upgrade breaks it, the documented fallback is a
file-watching agent in the pod that syncs to the draft API. The fallback is written down, not built.

Rejected: intercepting `/api/kernel/save` at the proxy. That request carries cells, not the `.py` file,
so egret would have to regenerate marimo's file format itself — a copy that drifts on every upgrade.

### 5.2 Run mode

A run pod serves exactly one published version. At start it fetches that version's source once, with a
pod-scoped token that can read only that version, writes it to local disk, and never writes back. A new
version gets a new pod; sessions opened before the publish keep running on the old version until they
close, and the old pod stops when idle.

## 6. Runtime, proxy and the security boundary

### 6.1 marimo must not share egret's origin

In run mode an author's JavaScript executes in the viewer's browser. S0 question 4 measured the vector:
an **anywidget** `_esm` module runs arbitrary JS (by design — widgets ship JS), while `mo.Html` is
sanitized (a `<script>` and an `<img onerror>` both stayed inert). If the iframe were same-origin with
egret, that script could read the parent document and call every egret API with the viewer's cookie —
including admin APIs. That is XSS into egret, and D3 makes the author untrusted with respect to the
viewer. Therefore:

- marimo is served from a **separate origin** (e.g. `apps.egret.example`; locally `apps.egret.localhost`
  beside `egret.localhost` — **not** a different port, because cookies are not isolated by port) in
  `<iframe sandbox="allow-scripts allow-same-origin allow-forms allow-downloads">`. `allow-same-origin`
  is relative to the *apps* origin, not egret's.
- egret's cookies are host-only, so by design the apps origin never holds them. S0 corroborates this
  only partly: `document.cookie` on the apps origin was empty of egret cookies, a check that cannot see
  HttpOnly cookies.
- To open an iframe, egret issues a **one-time ticket** (TTL 30 s, bound to user, app, mode, and version
  or draft). The apps origin redeems it at `/_ticket` for an apps-origin cookie (HttpOnly, Secure,
  SameSite=Lax, scoped to the same binding). The session token of §4.1 is minted from that redemption.
  S0 measured the ticket as single-use (replay → 403), a request with no apps cookie → 401, and a run
  session's cookie → 401 on the edit upstream. (The 30 s TTL was not measured.)
- **What S0 measured the author's JS can and cannot do** (automated Playwright, HeadlessChrome 136, one
  run, http, against a fake egret that models egret's cookies and double-submit CSRF): it cannot read the
  egret parent document (`SecurityError`), egret's cookies, or any egret response (no CORS allowance →
  `TypeError`); a POST carrying `X-Egret-CSRF` dies at the CORS preflight; a run session cannot reach the
  editor. **But two things did reach egret:**
  - **egret's SameSite=Lax cookies ride requests from the apps origin.** A `fetch` from the widget to
    egret's `whoami` arrived with the viewer's session cookie and the fake egret executed it as that user;
    CORS only kept the reply from the script. Same-site requests carry Lax cookies, so this is browser
    behaviour, not a quirk of the fake.
  - **a tossed cookie reached egret.** The widget set `egret_csrf=evil; domain=<parent domain>`, and
    egret's next request carried two `egret_csrf` values, the real one and `evil`. No state change
    succeeded only because the no-cors POST could not add the CSRF header.
- **Consequence, and P4 requirements.** Any egret `GET` (or other request that needs no custom header)
  with a side effect, and any CSRF check that compares cookie *values* without a header the attacker
  cannot set, is exploitable from inside an app. P4 therefore:
  - audits that egret's `GET` routes have no side effects, and keeps it that way (a test that fails when a
    state-changing route accepts `GET`);
  - keeps CSRF **header-based** (the `X-Egret-CSRF` header must be present and match), never value-only;
  - renames egret's cookies to `__Host-egret_session` / `__Host-egret_csrf` (the `__Host-` prefix forbids
    a `Domain` attribute, so a tossed cookie should not be able to use that name) — **this defence is
    unmeasured**: over http, Chromium refused to store the `Secure` `__Host-` cookies at all, so no victim
    session formed. P4 verifies it over HTTPS on kind (expected: a tossed `__Host-egret_csrf` never
    reaches egret);
  - keeps egret without any CORS allowance for the apps origin (a cross-origin request carrying
    `X-Egret-CSRF` needs a preflight, which then fails — measured in S0).
- **The apps origin is a sibling subdomain, i.e. same-site with egret.** The reason given was that a
  cross-site iframe's cookies are third-party cookies, which browsers block. S0 showed what same-site
  costs (above). **Decided with the user (2026-10-01, D12): keep the sibling subdomain** and pay for it
  with the P4 hardening above (`__Host-` cookies verified over HTTPS, side-effect-free GETs, header-based
  CSRF, no CORS allowance). Rejected: a different registrable site (e.g. `egret-apps.example`) — it would
  stop Lax cookies riding and make tossing impossible, but the apps-origin cookie becomes a blocked
  third-party cookie and ticket → cookie needs a replacement; S0 did not measure it. Not tried: serving
  the iframe from egret's own host with `sandbox` lacking `allow-same-origin` (opaque origin) — marimo
  would lose cookies and storage; a spike would be needed.
- This adds a host to ingress, Helm, kind and the OIDC configuration. S0 question 4 showed marimo edit
  and run working end to end behind the proxy on a separate origin with `--base-url`, WebSockets and
  ticket → cookie, so the architecture stands.

### 6.2 Proxy

An axum service in egret, listening on the apps host:

- Relays HTTP and WebSocket; strips hop-by-hop headers and every client `X-Egret-*` header; passes
  `Marimo-Server-Token` through (skew protection).
- **Blocks by route** what marimo cannot switch off (§2). S0 question 6 inventoried marimo 0.25.0's 107
  routes and blocked these 22 prefixes, under both `/e` and `/r`, with editing, saving, the 409 and run
  mode unchanged:
  `/terminal`, `/api/packages`, `/api/secrets`, `/api/files`, `/api/ai`, `/api/storage`, `/api/home`,
  `/api/kernel/copy`, `/api/kernel/rename`, `/api/kernel/install_missing_packages`,
  `/api/kernel/save_user_config`, `/api/export/auto_export`, `/api/export/requirements`, `/api/lsp`,
  `/api/datasources`, `/api/cache`, `/api/environment`, `/api/usage`, `/api/sessions`, `/api/status`,
  `/auth`, `/og`. The reason for each is in the S0 result. The editor itself calls
  `/api/packages/sandbox`, `/api/lsp/health`, `/api/usage` and `/api/datasources/discover` and gets 403
  with no effect on editing; the settings panel can no longer persist changes.
- **Checks and forwards the same path.** S0's first proxy matched a normalised path and forwarded one
  that httpx normalised again. Of 40 raw-socket variants: 3 HTTP `GET`s (`/e/x/../api/packages/list`
  and its `%2e%2e` / `..%2f` forms) returned 200 with the package list; 16 WebSocket forms of
  `/terminal/ws` got 101 from the proxy but marimo rejected the un-collapsed upstream URL, so no shell;
  2 `POST …/x/../api/files/list_files` passed the check and reached marimo (401 from marimo); the `/r`
  dot-form `GET` 401s and the `//` / `API` 404s are not attributed. The proxy takes the raw path the client
  sent, **rejects** it if it has a `.` or `..` segment (also after one percent-decode), `//`, an encoded
  `.` `/` `\` `%` or NUL, or a backslash, and otherwise matches it case-insensitively (percent-decoded)
  and forwards that same string (fixed: 0 of 40 pass — 28 × 403, 12 × 400).
- **A deny-list is fragile**: every marimo upgrade can add routes. P4 should prefer an **allow-list** of
  the routes the editor and run app need (the S0 result classifies all 107), and re-run the route
  inventory on every marimo upgrade.
- What the list cannot close: `/api/kernel/save` is needed and accepts a `filename` (save-as), confined
  only by marimo's inside-workspace check (`marimo/_server/api/endpoints/files.py:152-180`), so the pod's
  workspace directory must hold nothing but the draft; `/public/{path}` picks the notebook directory from
  a client-supplied `X-Notebook-Id` header (`endpoints/assets.py:712-750`) — the proxy strips it or pins
  it to the session's app; `/api/kernel/execute` and the scratchpad run code with a cell's power.
- Injects the pod's marimo token (marimo `--token`; never shown to a user) and the session token.
- **Edit:** `/e/{pod}/…` reaches only the caller's own edit pod; the app named by `?file=` requires
  `app:write` (checked again by `EgretStorage`).
- **Run:** `/r/{version}/…` requires `app:read` on the version's app.
- Revokes the session token when the session's WebSocket closes.

### 6.3 Pods

| | Edit | Run |
|---|---|---|
| Key | (author, workspace) | app version |
| Server | egret entrypoint → `marimo edit`, `EgretStorage` | egret entrypoint → `marimo run`, `include_code=False` |
| Kernels | one process per open notebook | one process per viewer session (D7), via egret's IPC patch; started from a pre-started launcher (pool or fork server, chosen in P5) |
| Lifetime | reaper on idle, as today | stop after 15 min with no session; egret shows a loading screen on cold start and on every new session until the app first renders |
| Limits | as today (250m/512Mi → 1000m/2Gi), configurable | configurable; max sessions per pod, then 503 "app busy" — the cap (20 was the pre-S0 guess) and the memory limit are re-set in P5 from the budget below, measured on kind |

- **Run-mode process per session is egret's patch, not a marimo feature** (§2). The entrypoint forces
  `SessionImpl.create` onto the IPC branch and points `ipc._get_venv_config` at the image's
  interpreter; kernels share the image's site-packages. Both are **private marimo API**, so, as with
  `EgretStorage`: the image pins an exact marimo version and its CI has a contract test (N sessions → N
  distinct kernel pids, and each session's header reaches its own kernel). If an upgrade breaks it and
  it cannot be re-patched, the fallback is one pod per (app × viewer).
- **Measured cost** (S0 and S0b; automated Playwright, single run per case, macOS dev laptop under heavy
  unrelated load (1-minute load 4.5 to 26), not a pod; S0b result:
  [`2026-10-01-marimo-s0b-result.md`](../plans/2026-10-01-marimo-s0b-result.md)):
  - **Memory.** ≈ 80 MB RSS per active IPC kernel (S0 79.9–81.3 MB; S0b claimed pool worker ~81 MB)
    plus a 77 MB server (S0). Pre-starting adds idle cost: a warm pool worker is 64.5–70.3 MB and a pool
    of size N holds N+1 of them; a fork server's parent is 48.5 MB, and its children's unique memory is
    unmeasured (macOS RSS counts shared copy-on-write pages). So the per-pod budget is server + launcher
    idle + sessions × ~81 MB; at 20 sessions the kernels alone are ≈ 1.6 GB before any app data.
  - **Time.** S0's 13.2 s first IPC render was **not reproduced** (S0b: 4.82 s warm, 7.35 s with an empty
    `.pyc` cache; the rest of the gap is unexplained). In a warm session ≈ 1.75 s goes to the browser
    loading egret's page and marimo's frontend, in every mode — a floor no launcher changes. On top of it
    the direct IPC launch adds ~2.3 s (T1, load ~4.5); in S0b round 4 (load 5.4 → 22.4) sequential p50
    was thread 2.12 s, direct 6.09, pool 3.54, fork server 3.03. No mode, not even thread, met < 2 s
    sequential or < 3 s for three concurrent viewers on this laptop; the concurrent figure there was
    dominated by page load (5.7–11.8 s ticket → WebSocket in every mode) and ranks nothing.
  - **The single biggest removable cost** is marimo's `has_marimo_installed` check: before **every**
    kernel launch from a configured, non-writable interpreter (egret's `_get_venv_config` patch makes it
    one) marimo runs `<python> -c "import marimo, msgspec, zmq; …"` (`marimo/_session/_venv.py:128`), a
    fresh interpreter importing marimo. Spawn → worker claimed took 1.0 to 1.8 s in the pool run, mostly
    this check (the span includes the shim start). Skipping it cut pool spawn → ready from 1.03–1.27 s to
    0.11–0.23 s (one outlier 1.48 s). **Decided with the user (2026-10-01, D14): option (a)** — the
    launcher shim answers the check, guarded by a contract test in the image's CI. Options considered: (a) the launcher shim answers that exact
    command (what S0b measured — a hack against private behaviour, needs a contract test that fails when
    the command changes); (b) an upstream marimo change (e.g. cache the result per interpreter and
    version); or (c) run the check once at image build or pod start and have the entrypoint patch return
    it (another private patch, same contract-test rule).
- **The launcher.** S0b built two pre-started launchers behind a shim at the configured interpreter path:
  a **warm pool** (N single-use workers that have already imported marimo, polars, httpx and
  `egret_marimo`; workers inherit the listening socket and one is armed at a time, because passing the
  accepted socket with `send_fds` lost sessions on macOS for an unproven reason) and a **fork server** (a
  single-threaded parent with the same imports that forks one child per kernel). Both run marimo's own
  `marimo._ipc.launch_kernel.main()`, whose module path marimo's docstring declares "a public API used by
  external consumers … Changing this path is a BREAKING CHANGE" (`marimo/_ipc/launch_kernel.py`); the
  stdin `KernelArgs` payload and the `KERNEL_READY` / `KERNEL_INFO` lines are not covered by that
  sentence, so the P3 contract test covers them too. Both cut direct's sequential-p50 excess over thread
  by more than half (the pool in rounds 3 and 4; the fork server in round 4, its only round) and kept
  isolation (distinct pids, single use, kernel killed when its client disconnects, none alive after the
  session TTL, identity correct). **S0b cannot choose between them**: in round 4 their sequential-p50 gap
  was ~0.5 s and flipped sign with the check skipped (pool 3.54 vs fork server 3.03 s; with the skip 2.43
  vs 2.95 s), single runs at load 5–22. They differ elsewhere: the pool's idle memory is (N+1) × ~70 MB
  and it hung on shutdown when its kernels were killed first (open, marimo side uninvestigated); the fork
  server is safe only while its parent never *uses* polars (only imports it), which nothing enforces. **P5
  chooses** after measuring on a quiet Linux node on kind: all variants interleaved, ≥ 5 runs each,
  per-phase marks, PSS/USS per idle worker and per forked child, the shutdown behaviour of both, with the
  `has_marimo_installed` fix P3/P5 adopted.
- **Session-start UX.** Every new viewer session waits for a kernel, not only a cold pod, and **a reload
  never reuses a kernel** (S0b: a host-page reload got a new pid; an iframe-only reload through the
  spike proxy stayed blank because the old WebSocket's close had revoked the token its cookie still
  mapped to — P4 re-tickets or rejects that case clearly). The loading screen covers the whole of it;
  P5 sets the pool size from the expected burst of new viewers (S0b has no number for a viewer who
  arrives while no worker is warm: every measured claim found one), and S0b gives no reason to change
  the pod idle timeout (15 min).

- `KernelLauncher` (`crates/egret-kernel/src/launcher.rs`) and its k8s and subprocess implementations are
  generalised to launch both pod kinds; the reaper and existing pod hardening (non-root, drop ALL,
  seccomp RuntimeDefault) carry over.
- **NetworkPolicy:** ingress only from egret; egress only to egret's gateway and DNS.
- **marimo configuration** (via `XDG_CONFIG_HOME` in the image) sets `[ai] enabled = false`; everything
  else is blocked at the proxy (§6.2). Package installation would fail anyway, egress being closed.
  `XDG_CONFIG_HOME` is the *last* place marimo looks — a `.marimo.toml` in the working directory, any
  parent, or `~` wins first (`marimo/_config/utils.py:69-120`) — so the image guarantees none exists.
- **Viewers do not see code** (`include_code=False`); people with `app:write` see it in edit mode.
- No autoscaling in v1.

## 7. Frontend

- The Explore menu entry "Notebooks" becomes **"Apps"**. The list shows each app's status (draft only /
  published vN) and actions **Open** (run) and **Edit** (edit, needs `app:write`).
- Both are full-page egret routes: the egret header (name, status, Publish, Versions, Share) above the
  iframe. The Versions panel lists versions and offers restore.
- Theme is passed to marimo as a query parameter.
- The notebook frontend (`NotebookPage.tsx`, `components/notebook/`, `lib/kernel/`, ≈ 4.9k LOC plus
  tests) is deleted in P6.

## 8. Permissions and gating

- New permissions: `app:read` (open the published app), `app:write` (edit the draft, publish, restore),
  `app:manage` (manage shares). Sharing uses the generic object-shares system with `app` as the object
  kind. **P1 ships only `app:read` and `app:write`**; `app:manage` arrives with shares in P2, because the
  catalog-drift test refuses a declared permission that nothing enforces.
- The workspace feature module `notebooks` is replaced by `apps`, covering `apps`, `apps-runtime`.
- `notebook:read` / `notebook:write` are removed in P6.

## 9. Delivery

### S0 — spike (throwaway code, output is a written report) — **done**

Result: [`docs/en/plans/2026-09-30-marimo-s0-result.md`](../plans/2026-09-30-marimo-s0-result.md)
(2026-09-30, marimo 0.25.0, HeadlessChrome 136, branch `spike/marimo-s0` @ `03f7a9fd`, never merged).
No fallback triggered; the architecture stands. Two open questions for the user are listed there; the apps-origin one is decided (D12, subdomain).

| # | Question | Fallback if no | What happened |
|---|---|---|---|
| 1 | Does a proxy-injected header reach the right session's kernel through `mo.app_meta().request`, including on reactive re-runs? | Pass the token as a query parameter at session creation | **Yes**, thread and IPC modes. Not triggered (§4.1). |
| 2 | Can run mode run each session in its own process — `--sandbox` offline, or the IPC branch forced by the entrypoint — and at what start-up cost? | One pod per (app × viewer) | `--sandbox`: **no** (1 pid for 4 sessions). Forced IPC: **yes** (4 pids), ≈ 80 MB per kernel, 13.2 s for the first session and 32.9 s for three concurrent, on a laptop. Not triggered; D7 rests on egret's patch (§2, §6.3). |
| 3 | Does the patched load + `EgretStorage` round-trip a draft, and what does the author see on 409? | File-watching sync agent; 409 shown by the egret header | **Yes**; marimo shows a "Failed to save" toast with egret's text. Neither triggered (§5.1). New requirement: re-fetch on load must not reset a live editor's revision. |
| 4 | Does marimo work behind the proxy with a base URL and WebSockets, on a separate origin, with ticket → cookie? | None — revisit the architecture | **Yes**. Not triggered. Author JS runs (anywidget); egret's Lax cookies ride its requests and a tossed cookie reaches egret; `__Host-` unmeasured on http → P4 requirements and one open question (§6.1). |
| 5 | Does a SQL cell accept `egret_marimo`'s DB-API connection as an engine? | A custom marimo engine class | **Yes**; listed in the dropdown only after its cell ran. Not triggered. DuckDB default decided: ship DuckDB, egret engine default (D13, §4.4). |
| 6 | Which routes must the proxy block (terminal, packages, secrets, file browser) without breaking edit and save? | — (this is a measurement, the proxy blocks regardless) | 22 prefixes; a dot-segment bypass found and fixed; allow-list preferred (§6.2). |

### S0b — kernel start-up latency spike — **done**

Result: [`docs/en/plans/2026-10-01-marimo-s0b-result.md`](../plans/2026-10-01-marimo-s0b-result.md)
(2026-09-30 to 2026-10-01, marimo 0.25.0, HeadlessChrome 136, branch `spike/marimo-s0` @ `14cc33f1`,
never merged; automated Playwright, single run per case, macOS dev laptop under heavy load).

| # | Question | What happened |
|---|---|---|
| S0b | Where does a new viewer's start-up time go in the forced IPC mode, and do pre-started kernels (warm pool, fork server) help? | ≈ 1.75 s is page + frontend load in every mode (a floor); marimo's per-launch `has_marimo_installed` check is most of a 1.0–1.8 s spawn → claimed span and is removable; S0's 13.2 s not reproduced (4.82 s). Round 4 seq p50: thread 2.12, direct 6.09, pool 3.54, fork server 3.03 s; < 2 s / < 3 s concurrent met by no mode, thread included. Pool vs fork server not separated; P5 chooses on Linux (§6.3). Isolation held in every variant. |

### PRs, in order

1. **P1 — Query gateway.** `app_sessions` (its `login_sid` cascades from egret's `sessions`, so logout
   revokes it by construction), the read-only checker, `POST /api/apps/query`, audit, rate limit — plus
   what the gateway must authorize against: the `analytics.apps` identity table, the `app:*`
   permissions and the `apps` feature module. Backend only; testable with a seeded token and no marimo.
   Plan: `docs/en/plans/2026-10-01-data-apps-p1-query-gateway.md`.
2. **P2 — Storage and lifecycle.** `app_drafts`, `app_versions` (+ trigger, and the FK from
   `app_sessions.version_id`, composite with `app_id` so a version belongs to its app), draft API with
   revision/409, publish, restore, app CRUD and shares with `app:manage`; the gateway checks that a run
   token's version still exists and is published; `apps.folder_id` FK (used for PDP ancestry); the
   frontend `ALL_FEATURE_KEYS` gains `apps`.
3. **P3 — `egret-apps` image and `egret_marimo`.** Entrypoints, `EgretStorage`, DB-API, `egret.sql`,
   locked-down marimo configuration, the run-mode IPC patch and the fix for marimo's per-launch
   `has_marimo_installed` check (D14: the launcher shim answers it, with a contract test that fails when
   marimo changes the command); pytest against
   a fake egret, including the
   `StorageInterface` contract test (with the reload-does-not-rebind case, §5.1) and the IPC contract
   test (§6.3). Decides the SQL-cell default engine (§4.4).
4. **P4 — Apps origin, proxy, edit mode.** Ticket → cookie, proxy (route allow-list or the S0 block
   list, one raw path checked and forwarded, `X-Notebook-Id` stripped or pinned), `__Host-` cookie rename
   verified over HTTPS on kind, the egret-side audit that `GET` routes have no side effects and CSRF stays
   header-based (§6.1), edit launcher, the Apps list and Edit page. From the P1 final review: token
   revocation by id / login session (not only by plaintext) and a sweeper for expired rows; decide whether
   an expired-but-undeleted egret login session must stop its app tokens; an `impersonator` column on
   `app_sessions` before any token is minted under impersonation; check `session_idle_secs <=
   session_absolute_secs` at boot; audit only the call that starts a rate-limit lockout, not every 429.
5. **P5 — Run mode.** Per-version launcher, publish/versions/restore UI, the viewer page; the kernel
   launcher (warm pool or fork server), the per-pod session cap, memory limit and session-start UX set
   from measurements on kind (§6.3). **Start-up targets (D15):** acceptance = a new viewer on a warm pod
   is at most **0.5 s** slower than same-round thread mode at the median, and three concurrent new
   viewers at most **1 s** slower at the max, measured on a Linux node on kind with ≥ 5 interleaved runs
   per variant. "< 2 s to first render" is tracked as a separate experience goal (frontend assets,
   caching) and does not block P5. **Shutdown (D16):** an acceptance test for whichever launcher P5
   picks — the pod shuts down within its grace period when kernels have already died (S0b saw the pool
   hang in exactly this order; the fork server was not tried).
6. **P6 — Remove the notebook.** `egret-notebook`, the Jupyter half of `egret-kernel`, `kernel-image/`,
   the notebook frontend, the notebook tables and `notebook:*`. The workflow task `egret.python`, which
   references a notebook and has no handler, is re-pointed to an app version or marked unsupported —
   decided in P6's plan.

## 10. Testing

The security properties are the acceptance criteria. Each has a test, and each test is shown to fail
when the code it guards is broken on purpose.

- A client-supplied `X-Egret-App-Token` (and any `X-Egret-*`) never reaches a pod.
- The gateway rejects `DELETE`, `INSERT`, `GRANT`, `SELECT 1; DROP TABLE t`, a data-modifying CTE,
  statements hidden behind comments, and anything unparseable.
- A token stops working after logout, after `app:read` is removed, and after its WebSocket closes.
- **JavaScript inside an app cannot read egret's cookies or egret's responses** — a browser test with an
  app that tries. (Its requests *do* reach egret as the viewer, §6.1; that is why the next two exist.)
- **JavaScript inside an app cannot complete any state change on egret** — tested by the audit that no
  egret `GET` has a side effect and that every state-changing route requires the `X-Egret-CSRF` header
  (header-based CSRF), plus a browser test that tries a no-cors POST and a header-bearing POST.
- **A tossed `__Host-` cookie never reaches egret** — the app sets a `Domain=`-scoped
  `__Host-egret_csrf` and then POSTs; tested over HTTPS on kind so the `__Host-` cookies exist (S0 could
  not, §6.1).
- **Two viewers with different Ranger grants see different rows from the same app**, on real Trino and
  real StarRocks on kind.
- An admin opening an app that runs `DELETE` gets a 400 and the table is unchanged.
- `UPDATE` and `DELETE` on `app_versions` fail.
- Two editors saving the same draft: the second gets 409, and marimo shows it — also when the second
  editor reloaded or opened a second tab in between (§5.1).
- The proxy rejects dot-segment, `//` and percent-encoded forms of a blocked route (S0's 40 raw-socket
  variants), on HTTP and WebSocket.
- N run sessions of one app run in N kernel processes.
- Live browser verification on kind for P4 and P5.

## 11. Done when

- An author builds an app with a SQL cell and a UI widget over Trino and over StarRocks, and publishes it.
- Two viewers see results under their own permissions.
- An admin opening a malicious app cannot write data, read egret's cookies or responses, or complete any
  state change on egret.
- The Jupyter notebook is gone.

## 12. Out of scope

Real-time collaboration; per-app dependencies (PEP 723 with an internal mirror — the natural next
step, and it does not change this architecture); embedding outside egret; public links; scheduled runs
or exports of apps; Arrow transport; autoscaling; converting old notebooks.
