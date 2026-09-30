# Data Apps on marimo — replacing the notebook

**Status:** designed
**Date:** 2026-09-30
**Replaces:** the Jupyter notebook (`egret-notebook`, the Jupyter half of `egret-kernel`, `kernel-image/`,
`frontend/src/components/notebook/`)
**Delivered as:** one spike (S0) and six PRs (P1–P6), §9.

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
| D7 | **Run mode: one pod per app version, one process per viewer session.** Fallback if S0 fails: one pod per (app × viewer). Never threads. | Viewer sessions share no memory. |
| D8 | **Edit mode: one pod per (author × workspace), optimistic concurrency** (revision + 409). No locks, no real-time collaboration. | Every kernel runs as exactly one person. |
| D9 | **Fixed, curated Python image.** No runtime package install. | Egress can be closed to egret only. |
| D10 | **"Notebooks" becomes "Apps"** in the Explore menu; permissions `app:read`, `app:write`, `app:manage`. | `notebook:*` is removed. |
| D11 | **marimo's own UI in an iframe**, served by an egret reverse proxy. Not a fork of its frontend, not a custom UI over its protocol. | The iframe does not use egret's OKLCH design; theme (light/dark) is passed by query param. |

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
- **marimo's run mode puts sessions in threads** of one process (`marimo/_session/managers/kernel.py`).
  An author's code could then pass one viewer's rows to another through module state. D7 requires one
  process per session; marimo has a subprocess kernel manager over ZeroMQ (`managers/ipc.py`, used for
  sandboxes) — whether run mode can use it is S0 question 2.
- **marimo's auth is one shared token** (`marimo/_server/api/auth.py`), but it keeps an outer
  `scope["user"]`, and notebook code sees the request through `mo.app_meta().request` (headers, cookies,
  `user`, `meta`). That is the only per-session channel into a kernel.
- **marimo has a storage seam, but it is private.** `StorageInterface` (read/write/exists/rename,
  `marimo/_session/notebook/storage.py`) is accepted by `AppFileManager(storage=…)`
  (`file_manager.py:73`), but nothing public lets you supply one.
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
- The kernel reads it through `mo.app_meta().request.headers`. That this reaches the kernel of *the right
  session* is S0 question 1; fallback: pass it as a query parameter on session creation.
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
   `app:read` (run) or `app:write` (edit) on the token's app, and `query:run` on the database.
3. Resolve `database` **by name** within the token's workspace (404 if absent — same as denied).
4. Read-only check (§4.3); reject with 400 and the reason.
5. `QueryExecPort::execute_as` (or `execute_as_with_params`) as the token's user, with
   `max_rows = min(requested, configured cap)` and the configured timeout.
6. Audit: app, version (or draft), user, database, statement hash, row count, duration, outcome.

Response: `TabularResult` as JSON (columns, rows, `truncated`). Arrow is a later optimisation.

Errors keep the port's distinctions: `Forbidden` → 403 with the engine's message (the viewer lacks a
grant — expected under D3); `NotFound` → 404; `InvalidInput` → 400; `Internal` → 500 without engine
addresses; expired token → 401, which the helper turns into "session expired — reload the app". Each
session has a rate limit (configurable, default 10 queries/s, burst 20).

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
  recognises it as an engine, so it appears in the SQL cell's engine dropdown (S0 question 5).
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

- `read(path)` → `GET …/draft`, remembering the revision.
- `write(path, source)` → `PUT …/draft` with `If-Match`. A 409 raises, and marimo shows its own "save
  failed" message: "this draft was saved by someone else — reload". No extra channel needed.
- `exists`, `rename` → map to the app; rename is refused (names change in egret, not in marimo).
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

In run mode an author can emit arbitrary HTML and JavaScript (`mo.Html`, anywidget). If the iframe were
same-origin with egret, that script could read the parent document and call every egret API with the
viewer's cookie — including admin APIs. That is XSS into egret, and D3 makes the author untrusted with
respect to the viewer. Therefore:

- marimo is served from a **separate origin** (e.g. `apps.egret.example`; locally, a different port) in
  `<iframe sandbox="allow-scripts allow-same-origin allow-forms allow-downloads">`. `allow-same-origin`
  is relative to the *apps* origin, not egret's.
- egret's session cookie is never sent to the apps origin.
- To open an iframe, egret issues a **one-time ticket** (TTL 30 s, bound to user, app, mode, and version
  or draft). The apps origin redeems it at `/_ticket` for an apps-origin cookie (HttpOnly, Secure,
  SameSite=Lax, scoped to the same binding). The session token of §4.1 is minted from that redemption.
- This adds a host to ingress, Helm, kind and the OIDC configuration. S0 question 4 must prove it end to
  end; it has no fallback short of revisiting the architecture.

### 6.2 Proxy

An axum service in egret, listening on the apps host:

- Relays HTTP and WebSocket; strips hop-by-hop headers and every client `X-Egret-*` header.
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
| Kernels | one process per open notebook | one process per viewer session (D7) |
| Lifetime | reaper on idle, as today | stop after 15 min with no session; egret shows a loading screen on cold start |
| Limits | as today (250m/512Mi → 1000m/2Gi), configurable | configurable; max 20 sessions per pod, then 503 "app busy" |

- `KernelLauncher` (`crates/egret-kernel/src/launcher.rs`) and its k8s and subprocess implementations are
  generalised to launch both pod kinds; the reaper and existing pod hardening (non-root, drop ALL,
  seccomp RuntimeDefault) carry over.
- **NetworkPolicy:** ingress only from egret; egress only to egret's gateway and DNS.
- **marimo configuration in the image disables:** the terminal, AI assistant, package installation, file
  browser, secrets panel, and file create/rename. Anything config cannot disable, the proxy blocks by
  route.
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
  kind.
- The workspace feature module `notebooks` is replaced by `apps`, covering `apps`, `apps-runtime`.
- `notebook:read` / `notebook:write` are removed in P6.

## 9. Delivery

### S0 — spike (throwaway code, output is a written report)

| # | Question | Fallback if no |
|---|---|---|
| 1 | Does a proxy-injected header reach the right session's kernel through `mo.app_meta().request`? | Pass the token as a query parameter at session creation |
| 2 | Can run mode run each session in its own process (IPC manager)? | One pod per (app × viewer) |
| 3 | Does `EgretStorage` plug into `StorageInterface`, and does a 409 show in marimo's UI? | File-watching sync agent |
| 4 | Does marimo work behind the proxy with a base URL and WebSockets, on a separate origin, with ticket → cookie? | None — revisit the architecture |
| 5 | Does a SQL cell accept `egret_marimo`'s DB-API connection as an engine? | A custom marimo engine class |
| 6 | Can configuration disable terminal, AI, package install and file browser? | Block those routes at the proxy |

### PRs, in order

1. **P1 — Query gateway.** `app_sessions`, the read-only checker, `POST /api/apps/query`, audit, rate
   limit. Backend only; testable with a seeded token and no marimo.
2. **P2 — Storage and lifecycle.** `apps`, `app_drafts`, `app_versions` (+ trigger), draft API with
   revision/409, publish, restore, `app:*` permissions, the `apps` feature module.
3. **P3 — `egret-apps` image and `egret_marimo`.** Entrypoints, `EgretStorage`, DB-API, `egret.sql`,
   locked-down marimo configuration; pytest against a fake egret, including the `StorageInterface`
   contract test.
4. **P4 — Apps origin, proxy, edit mode.** Ticket → cookie, proxy, edit launcher, the Apps list and Edit
   page.
5. **P5 — Run mode.** Per-version launcher, publish/versions/restore UI, the viewer page.
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
- **JavaScript inside an app cannot read egret's cookies or call egret's API** — a browser test with an
  app that tries.
- **Two viewers with different Ranger grants see different rows from the same app**, on real Trino and
  real StarRocks on kind.
- An admin opening an app that runs `DELETE` gets a 400 and the table is unchanged.
- `UPDATE` and `DELETE` on `app_versions` fail.
- Two editors saving the same draft: the second gets 409, and marimo shows it.
- Live browser verification on kind for P4 and P5.

## 11. Done when

- An author builds an app with a SQL cell and a UI widget over Trino and over StarRocks, and publishes it.
- Two viewers see results under their own permissions.
- An admin opening a malicious app cannot write data or reach egret's API.
- The Jupyter notebook is gone.

## 12. Out of scope

Real-time collaboration; per-app dependencies (PEP 723 with an internal mirror — the natural next
step, and it does not change this architecture); embedding outside egret; public links; scheduled runs
or exports of apps; Arrow transport; autoscaling; converting old notebooks.
