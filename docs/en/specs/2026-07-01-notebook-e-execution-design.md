# Notebook E — Execution Wiring (design)

> Status: draft for review · 2026-07-01
> Foundation: D1a (kernel image + helpers + `POST /kernel/query`), D1b (launcher), D1c (WS channels proxy) — all merged; F2 (unified OIDC delegation) — merged, so governed engine SQL runs per-user. Bilingual: English canonical; VI reference at `docs/vi/specs/2026-07-01-notebook-e-execution-design.md`.

## 1. Goal

Make notebook cells actually run. Wire execution end-to-end on top of the per-(user×workspace) Python/Daft kernel (D1a/b/c): the browser drives the kernel, cells execute, outputs render, results persist. This is the final sub-spec of the Notebook epic (A→B→C→D→**E**).

## 2. Architecture (Option A — frontend Jupyter client over the D1c proxy)

The browser is a **Jupyter Messaging Protocol client** speaking to the kernel over the **single multiplexed WebSocket** already proxied by D1c (`GET …/notebooks/{nb}/kernel/channels`, messages carry a `channel` field). The backend proxy is no longer purely transparent — it **taps** the kernel→browser (iopub) direction to persist outputs (the recorder backstop). The frontend owns live rendering; the recorder owns durable persistence.

```
browser (Jupyter client)  ──execute_request──►  D1c WS proxy ──►  kernel (Daft + helpers)
        ▲  render live                              │ tap iopub (recorder)
        │  restore on load                          ▼
   NotebookRuntime manifest              last_output / execution_count (DB, server-side)
```

Why Option A (matches JupyterLab + sparklabx, both hand-roll the protocol over a raw WS; `@jupyterlab/services` is installed-but-unused in sparklabx): D1c was deliberately built as a verbatim channel proxy for exactly this. The frontend correlates iopub by `parent_header.msg_id`.

## 3. Components

### 3.1 Frontend kernel client (`frontend/src/lib/kernel/`)
- `KernelConnection`: opens the WS after `POST …/kernel/connect` returns ready; Jupyter framing (header/parent_header/metadata/content, `channel` shell|iopub|control, `session`, `msg_id` via `crypto.randomUUID()`, protocol `5.3`); reconnect + silent retry; auth via the session cookie (GET-upgrade passes CSRF, per D1c).
- `execute(cellId, code)`: sends `execute_request` (with `metadata.egret_cell_id = cellId` so the recorder can tag it), tracks the pending msg_id → cellId map.
- iopub routing: `status` (busy/idle → running↔done, idle is the reload-safe completion signal), `stream` (stdout/stderr), `execute_result` + `display_data` (MIME bundle), `error` (ename/evalue/traceback), `execute_reply` (execution_count).
- control/REST: `interrupt` + `restart` via backend passthrough (see 3.4).

### 3.2 SQL via a kernel cell-magic (`kernel-image/egret_kernel_helpers/magic.py`)
- `%%egret_sql --target daft|<engine>`: `daft` → `daft.sql(cell)` over the attached Iceberg catalog; `<engine>` → the D1a `query(target, cell)` helper (governed callback `POST /kernel/query`, which post-F2 presents the user's engine JWT to Trino/StarRocks — real per-user authz). Registered in the D1a startup shim `startup/00-egret.py`. Parsing via `magic_arguments` (escaping handled in Python — the FE sends raw SQL, never string-builds Python). → the kernel image gets a new version; extends the D1a pytest.

### 3.3 Cell model / API (`egret-notebook` + migration)
- Add `sql_target text NULL` to `notebook_cells` (default `'daft'`; only SQL cells use it). Extend the C1 cell PATCH to accept it.
- `last_output` (jsonb, already present) contract: an array of output items `{type, ...}` mirroring trimmed nbformat (`stream`/`execute_result`/`display_data`/`error`). `execution_count` + `last_execution_time_ms` already present. **Server-side size cap ~256 KB** enforced by the recorder (truncate + marker); no S3 spill in this spec (storage sub-spec B not built).

### 3.4 Backend recorder + controls (`egret-kernel`)
- **Recorder**: extend the `BackendWsProxy` tap to parse iopub messages, accumulate per `egret_cell_id` (from the originating `execute_request` metadata), and persist `last_output`/`execution_count`/`last_execution_time_ms` via a `NotebookOutputStore` seam (UPDATE `notebook_cells`). This makes outputs durable even if the browser tab closes mid-run (sparklabx "KernelRecorder" pattern). **The recorder is authoritative for output persistence; the FE does NOT PATCH outputs** (avoids the sparklabx dual-write race) — the FE only renders live + persists source/`sql_target`/structure (existing C1 path).
- `GET …/notebooks/{nb}/kernel/active-executions`: restore in-flight cells on reopen.
- `POST …/kernel/interrupt` + `POST …/kernel/restart`: bearer/session-authed passthroughs to the Kernel Gateway REST (the D1c channels WS is verbatim; interrupt/restart go over REST, like sparklabx). Mount CSRF-appropriately (session routes behind CSRF, per the F2-P1 split).

### 3.5 Frontend notebook UX (`NotebookPage` / `CellEditor`)
- Kernel status bar (idle/busy/spawning/disconnected); connect on open; restore outputs from `last_output` + `active-executions`.
- CellEditor: enable Run; per-cell state (idle/queued/running/done/error); SQL cells get a **target dropdown** (Daft | engines from the F1 manifest `engines`); markdown renders client-side (already done in C1).
- **Outputs renderer** (`CellOutputRenderer` + `ansiToHtml`, ported from sparklabx): MIME `text/html` (DOMPurify-sanitized — Daft DataFrame tables + `query()` results), `image/png`+`image/jpeg` (base64 `<img>`), `text/plain`, error tracebacks via a 16/256-colour ANSI→HTML converter; per-cell error boundary; render caches to avoid O(n²) on long streams. (No Spark-log noise-filtering — egret is Daft, not Spark.)
- Toolbar (full controls): Run cell, **Run all** (queue every `execute_request` to the kernel upfront + `stop_on_error` so a tab close doesn't halt the batch — sparklabx pattern), Run above/below, Interrupt, Restart, Clear outputs.

## 4. Data flow — run a cell
1. Run → ensure kernel (`/connect` → ready) + WS open.
2. SQL cell → FE prepends `%%egret_sql --target <t>\n`; code cell → raw source; markdown → not run.
3. FE sends `execute_request` (with `metadata.egret_cell_id`); cell = running.
4. iopub streams back through the D1c proxy → FE routes by `parent_header.msg_id` → renders; the recorder taps the same stream → accumulates + persists.
5. `status: idle` → cell done; recorder writes `last_output` (capped) + `execution_count` + elapsed. Errors → render traceback + stop run-all.

## 5. Error handling
Kernel not ready → spawning state + retry; WS drop → reconnect, in-flight marked unknown (restored via active-executions); execution error → render + stop run-all; output too large → truncate + marker; interrupt → cell interrupted; restart → fresh kernel (execution_count resets), FE clears running states.

## 6. Testing
- **Kernel pytest** (extends D1a): the `%%egret_sql` magic (daft + engine targets, arg parsing, escaping) with faked daft/`query`.
- **Backend** (`#[sqlx::test]` where the sqlx version allows, else the egret-kernel http harness): recorder parse+persist+cap (fake iopub msgs), active-executions, interrupt/restart (fake gateway), cell PATCH accepting `sql_target`.
- **Frontend** (Vitest): kernel client routing (parent_header correlation, MIME mapping), outputs renderer per MIME + ANSI, run-all upfront-queue/stop-on-error, restore-on-load.
- **Live smoke** (manual, documented — real Jupyter exec isn't in CI): boot the stack, run a Python/Daft cell, a SQL/Daft cell, a SQL/engine cell (governed per-user via F2), a plot (png), an error, and a tab-close-mid-run → reopen shows the output (proves the recorder).

## 7. Phasing (one spec, phased plan)
- **E-P1**: kernel `%%egret_sql` magic + migration `sql_target` + backend interrupt/restart + the `last_output` JSON contract & server-side cap.
- **E-P2**: backend recorder/tap + `NotebookOutputStore` + `active-executions` endpoint.
- **E-P3**: FE kernel client (protocol/routing/status) + outputs renderer (png/html-DOMPurify/jpeg/ANSI) + run single cell + restore-on-load.
- **E-P4**: full controls (run-all upfront-queue/above/below, interrupt, restart, clear) + SQL target dropdown + polish.

## 8. Out of scope
- S3 output spill (needs storage sub-spec B); rich MIME beyond png/html/plain (svg/json/markdown/vnd.*); collaborative editing/RTC; per-cell kernel selection (single Python kernel per the C1 reshape).

## 9. Self-review checklist
- Placeholders: none — the two integration risks (`%%egret_sql` in a shipped kernel image → rebuild; sqlx-version split in egret-kernel/egret-catalog-engine → use the crate's own test pattern) are flagged, not hidden.
- Consistency: Option A + recorder-owns-persistence applied throughout (§2/§3.4/§4); the FE never PATCHes outputs.
- Scope: execution only; governance is F2 (done), storage spill is B (deferred). Phased for independent delivery.
- Ambiguity: SQL target routing (§3.2) maps exactly to F2's governance boundary (daft=token-scoped Iceberg, engine=governed callback with the user's JWT).
