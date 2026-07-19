# Notebook IDE Redesign — Design

- **Date:** 2026-07-19
- **Status:** Approved (brainstorm) → ready for implementation plan
- **Scope area:** `egret/frontend/src/**/notebook*`, `egret/frontend/src/lib/kernel/**`, `egret/crates/egret-notebook`, `egret/kernel-image`
- **Related:** notebook epic (A–E complete), F2 OIDC delegation, three-layer management model. Reference product: `sparklabx` notebook UX.

## 1. Problem & Goal

egret's notebook works end-to-end (cells run on a real Daft/Jupyter kernel, outputs persist, run controls exist) but the UI is minimal: a single-column page (`NotebookPage.tsx`, ~470 lines), a 38-line output renderer, no sidebar, no kernel-status indicator, no execution counters, and no first-class SQL cell. Next to Hex / Databricks / a polished sparklabx notebook it reads as "basic".

**Goal:** raise the notebook *editing experience* to an IDE-grade, Hex-like feel while keeping egret's architectural differentiators (governed 2-plane access, per-workspace Iceberg catalog, Ranger/OIDC delegation) unchanged. This is a **UX + thin-vertical-slice** effort, **not** a change to the execution model (still sequential, single Daft kernel — no reactive DAG, no cluster scaling).

### Non-goals (explicitly deferred)
- Reactive / DAG execution, real-time multiplayer, notebook version history.
- Resource-usage badge (needs a backend `/kernel/usage` endpoint; the SubprocessLauncher has no container metrics).
- Personal file browser (Spec B — no backend).
- Publish-to-BI from a notebook (separate spec; noted as future high-value).

## 2. Current state (verified 2026-07-19)

- **Kernel model is DataFrame-first.** `egret_kernel_helpers` exposes `trino(sql)` / `starrocks(sql)` → **Daft DataFrame** (via `daft.read_sql` over a fresh per-user JWT, governed), `write_table(df, …)`, and `display(obj)` (renders a Daft DataFrame capped at 50 rows). There is **no** `%%egret_sql` magic anymore (only stale `.pyc`); the old SQL-cell path was reverted.
- **`sql_target` column was added (migration 0046) then dropped (0047).** `notebook_cells` currently has no SQL columns. FE `CellType` still lists `"sql"` but nothing backs it; `CellEditor` renders every non-markdown cell as Python.
- **`KernelConnection`** exposes `KernelStatus` (`idle|busy|spawning|connected|disconnected`) but only surfaces per-execute `onStatus(busy|idle)` — there is **no** connection-level reactive status stream.
- **`useUpdateCell` / `useAddCell`** thread only `cell_type` + `source` (no SQL fields).
- **`CellOutputRenderer`** (egret, 38 lines) renders a subset; `ansiToHtml.ts` is already ported. sparklabx's 395-line renderer has an error boundary, memoized ANSI/split caches, HTML(DOMPurify)/png/jpeg/plain/error+traceback, and collapsible logs — but its noise filters are Spark/Scala-specific and its "Debug AI" button needs an AIContext egret lacks.
- egret design tokens are **OKLCH** (`src/index.css`); relevant semantic tokens exist: `--primary` (teal), `--success` (green), `--destructive`, `--muted-foreground`, `--card`, `--border`, both light + dark. **No new colors or fonts are introduced.**

## 3. Design language

- **Style:** "Soft UI Evolution" adapted to egret tokens — cards `bg-card border border-border rounded-lg shadow-sm`; running cell gets `ring-1 ring-primary/40`. Dense layout (dev-tool density): `gap-2/3`, toolbar `py-1`.
- **Color mapping (semantic, not raw hex):** connected/OK → `--success`; connecting/busy/restarting → `--primary` (busy uses a subtle pulse); error/dead → `--destructive`; disconnected → `--muted-foreground`. Status is **never color-only** — always paired with a text label + tooltip.
- **Type:** code + output use `font-mono`; Monaco handles in-editor font. No font import.
- **Motion:** 150–200ms on hover / collapse / sidebar toggle; wrap in `prefers-reduced-motion`.
- **A11y:** aria-labels on icon-only buttons, visible focus rings, headings sequential, output error has icon + text, DataFrame HTML sanitized with DOMPurify.

## 4. Architecture — component map

All under `egret/frontend/src`. New = ✚, changed = ✎.

```
routes/explore/NotebookPage.tsx            ✎ restructure into 3-region IDE shell
components/notebook/
  NotebookShell.tsx                        ✚ 3-region layout (rail | panel | main)
  NotebookHeader.tsx                       ✚ breadcrumb, name, public, kernel badge, run toolbar
  SidebarIconRail.tsx                      ✚ tabs: TOC · Data Sources (collapsible)
  panels/TableOfContentsPanel.tsx          ✚ headings/cells → scroll-to
  panels/DataSourcesPanel.tsx              ✚ engines + catalogs from runtime (read-only)
  KernelStatusBadge.tsx                    ✚ reactive dot + label + tooltip
  CellEditor.tsx                           ✎ gutter In[N], accent bar, sql target/into, md toggle
  CellOutputRenderer.tsx                   ✎ port (trimmed) from sparklabx
  ansiToHtml.ts                            (reuse)
  monaco/registerStatic.ts                 ✚ python + sql static completions
lib/kernel/
  KernelConnection.ts                      ✎ add status emitter + interrupt/restart status
  useKernelStatus.ts                       ✚ subscribe to connection status
  protocol.ts                              (reuse CellOutput union)
api/notebooks.ts                           ✎ sql_target + sql_into on Cell + mutations
```

### 4.1 IDE shell (`NotebookShell` + restructured `NotebookPage`)
Three regions:
1. **`SidebarIconRail`** (`w-12`, collapsible): two tabs — **Table of Contents**, **Data Sources**. Active tab highlighted (`bg-primary/10 text-primary`). Clicking the active tab collapses the panel. (We intentionally drop sparklabx's "Notebooks"/"My Files" rail items: notebook navigation already lives in the app sidebar, and there is no personal file backend.)
2. **Panel** (`w-64`, hidden when collapsed): renders the active panel.
3. **Main**: sticky `NotebookHeader` + scrollable cell list + add-cell affordances. Header is `sticky top-0 z-10` so run controls stay visible in long notebooks.

Layout collapses gracefully below `md`: rail becomes a top toggle, panel becomes an overlay sheet. No horizontal scroll on the page body; wide outputs scroll inside their own container.

### 4.2 `NotebookHeader`
Breadcrumb `← Notebooks` · name (rename-on-blur, existing logic) · Public `Switch` (existing) · **`KernelStatusBadge`** · run toolbar: **Run all · Interrupt · Restart · Clear outputs** (reuse the existing runner-registry logic in `NotebookPage`; Lucide icons: `Play`, `Square`, `RotateCcw`, `Eraser`). Toolbar buttons disable appropriately (Run-all while running; Interrupt only when busy).

### 4.3 `KernelStatusBadge` + `KernelConnection` status stream
`KernelConnection` gains a lightweight observable status:
- Internal `status: KernelStatus` + `listeners: Set<(s)=>void>`, `onStatusChange(cb): () => void`, private `setStatus(s)`.
- Emit transitions: `disconnected` → `connecting` (on `connect()` start) → `connected` (ws open + kernel ready) → `busy`/`idle` (from iopub status, already parsed) → `disconnected` (on `disconnect()`/close). `interrupt()`/`restart()` set transient `interrupting`/`restarting`; failure → `error`; kernel-dead close code → `dead`.
- Extend the `KernelStatus` union with `connecting | interrupting | restarting | error | dead`.

`useKernelStatus(kernel)` subscribes and returns the current status for the badge. Badge = colored dot (mapped tokens §3) + fixed-width label + `title` tooltip; `aria-live="polite"` on the label so screen readers hear transitions.

### 4.4 `CellEditor`
- **Left gutter:** `In [N]:` prompt (execution count from `cell.execution_count`), or a spinner while running; a thin **accent bar** colored by state (running → `--primary`, last run errored → `--destructive`, ok → `--success`, idle → `--border`).
- **Type select** gains **SQL** (`code | markdown | sql`). Monaco `language` follows type (`python` / `sql`); markdown uses the existing textarea + preview, now an **edit/preview toggle** (preview when the cell is not focused, matching sparklabx).
- **SQL cell chrome:** a **target dropdown** (options from `useNotebookRuntime().engines`, default = workspace default engine; "Daft (local)" is a stretch option) bound to `sql_target`, and a compact **"→ variable"** input bound to `sql_into` (default `df`). See §5.
- **Cell toolbar** (existing move/delete/run + run-above/below dropdown) becomes hover-revealed to reduce clutter; **Shift+Enter runs** the cell (Monaco keybinding + textarea handler).
- **Outputs** render via `CellOutputRenderer`.

### 4.5 `CellOutputRenderer` (trimmed port)
Port sparklabx's renderer, **minus** Spark/Scala noise filters and the AI "Debug" button:
- `CellOutputErrorBoundary` per cell (contain a bad-HTML render to one cell).
- Memoized caches: `WeakMap` ANSI-split + bounded ANSI→HTML cache (avoid O(n²) on streaming).
- Render: `stream` (stdout/stderr → `<pre>` ANSI), `result` MIME bundle — `text/html` (DOMPurify, `overflow-auto max-h-96`), `image/png`, `image/jpeg`, `text/plain`, JSON fallback — and `error` (icon + `ename: evalue`, traceback ANSI, styled with `--destructive`).
- **Collapsible logs:** a single generic "Logs (N lines)" disclosure that collapses `stderr`-classified lines (replaces the Spark/Scala/Delta-specific split). Keep it simple: stdout = user output; stderr = collapsible logs.

### 4.6 Panels
- **`TableOfContentsPanel`:** derive entries from cells — markdown headings (parse `#`/`##`) and a compact label for code/SQL cells (`In [N]` + first non-blank line). Click → `scrollIntoView` the cell (each `ManagedCell` exposes a ref via the existing registry or an `id={cell.id}` anchor). Indent by heading level.
- **`DataSourcesPanel`:** list `useNotebookRuntime().engines` (mark the default) and catalogs, read-only. Each row: engine icon + name + kind. A footer hint shows copy-paste syntax: `df = trino("SELECT … ")` / `df = starrocks("…")`. No "add connector" (egret configures these on the Notebook Settings page).

### 4.7 Monaco completions (`monaco/registerStatic.ts`)
Register static completion items once: **python** — `trino`, `starrocks`, `write_table`, `display`, `runtime`, `me`, plus common Daft ops; **sql** — standard keywords + the enabled engines' catalog/schema names if cheaply available (else keywords only). Adapt (don't copy wholesale) from sparklabx's `monaco/`.

## 5. SQL cell + SQL→DataFrame binding (thin vertical slice)

**UX:** a SQL cell holds clean SQL (no visible magic). It has a **target** engine and an **output variable** name. Running it binds the result to a **real Daft DataFrame** in the kernel namespace, so downstream Python cells reuse it (`sales_df.filter(...)`). This mirrors Databricks `--into` and Hex SQL cells and reuses egret's existing DataFrame-first helpers — **no new query engine code**.

### 5.1 Backend (`egret-notebook` + migration)
- **Migration `0048_notebook_cell_sql_binding.sql`** (append-only): re-add `sql_target text` and add `sql_into text`, both nullable, to `notebook_cells`. Touch `egret-metastore/src/lib.rs` to re-embed migrations (known staleness gotcha).
- Thread `sql_target` + `sql_into` through the cell model, store (create/update), and HTTP DTOs (add-cell + update-cell). Preserve existing IDOR/ownership guards; no behavior change for code/markdown cells (fields stay `NULL`).
- **Regenerate the API contract:** `cargo run -p egret-bin -- openapi > openapi.json` → `npm run gen:api`.

### 5.2 Kernel (`kernel-image`)
- Add `egret_kernel_helpers/magic.py` registering a cell magic **`%%egret_sql`**:
  - Parse `--target <engine>` (default: default engine) and `--into <var>` (default `df`; validate it is a valid Python identifier, else raise a clear error).
  - Run `trino(sql)` or `starrocks(sql)` (dispatch on target) → Daft DataFrame.
  - Assign into the user namespace: `get_ipython().user_ns[var] = df`.
  - `display(df)` so the cell shows the table.
  - Errors surface via the existing `RuntimeError` messages (clear, never silent-empty).
- Register the magic in `startup/00-egret.py`.
- pytest: parse flags, identifier validation, dispatch, namespace assignment (with a fake IPython + fake `trino`).

### 5.3 Frontend
- `Cell` type + `useAddCell`/`useUpdateCell`: add `sql_target?: string`, `sql_into?: string`.
- SQL `CellEditor`: target dropdown + "→ variable" input (persist via `updateCell`). To avoid silent overwrite when a notebook has several SQL cells, the FE seeds a **collision-free default** on cell creation: `df` for the first SQL cell, then `df_2`, `df_3`, … (scan existing `sql_into` values). The user can rename freely; a duplicate name shows an inline warning ("reuses variable `df` — will overwrite") rather than a hard error.
- On **Run** of a SQL cell, prepend a hidden first line
  `%%egret_sql --target <sql_target|default> --into <sql_into|df>`
  to the SQL source and send that to the kernel (the editor keeps showing clean SQL — same hide-the-magic technique used previously). Body is raw SQL, so no Python string-quoting hazard.
- Under the output, show a **binding chip**: `sales_df · Daft DataFrame` so users know the reusable variable name.

### 5.4 Stretch — Daft-local target
A `daft` target compiles to `daft.sql("<sql>")` over already-bound DataFrames (register bound vars into a Daft SQL catalog). Deferred; v1 targets are engines.

## 6. Data flow (unchanged execution model)
1. FE `KernelConnection.execute(cellId, source)` over the D1c WS proxy — for SQL cells, `source` is `%%egret_sql …\n<sql>`.
2. Kernel magic → `trino()/starrocks()` → per-user JWT → governed engine (Ranger/OIDC) → Daft DataFrame → bound to `user_ns[var]` → `display`.
3. iopub → recorder (backend E-P2) persists `last_output`; FE renders live via `CellOutputRenderer`. FE does **not** PATCH outputs (recorder owns persistence).

## 7. Testing
- **FE (Vitest):** `KernelStatusBadge` state→label/color mapping; `useKernelStatus` subscribe/unsubscribe; `CellOutputRenderer` for each MIME type + error + collapsible logs + error-boundary containment; SQL `CellEditor` target/into persistence + magic-prepend-on-run; TOC scroll targets; DataSources list from a mocked runtime.
- **Kernel (pytest):** `%%egret_sql` flag parsing, identifier validation, dispatch, namespace binding, display.
- **Backend:** `#[sqlx::test]` scratch-DB round-trip of `sql_target`/`sql_into` on create/update; NULL for code/markdown.
- **Live e2e (manual, not CI):** per the notebook live-e2e recipe — run a SQL cell (`SELECT … `), confirm the bound variable is usable in a following Python cell, confirm output table + In/Out counters + kernel badge transitions. Full local CI (fmt/clippy/`cargo test --workspace`, FE lint/typecheck/test/build) green before any merge.

## 8. Risks / gotchas
- **OpenAPI regen** must run or FE DTOs drift (`sql_target`/`sql_into`). Keep the hand-maintained `CellType` comment convention in `api/notebooks.ts`.
- **Migration staleness:** touch `egret-metastore/src/lib.rs`; migration is append-only (do not edit an applied one).
- **`sql_into` validation** on both kernel (identifier) and FE (inline error) to avoid injecting invalid Python.
- **Reduced-motion** must gate the busy pulse and sidebar/collapse transitions.
- **Sticky header + Monaco:** ensure Monaco's own scroll doesn't fight the page; fixed editor height with `scrollBeyondLastLine:false` (existing).

## 9. Suggested phasing (for the plan)
- **P1 (FE-only):** IDE shell + header + `KernelStatusBadge` (+ KernelConnection status stream) + `CellOutputRenderer` port + panels + Monaco completions. Shippable on its own.
- **P2 (vertical slice):** SQL cell + binding — migration 0048, `%%egret_sql` magic, DTO/regen, FE SQL cell + chip.
- **P3 (stretch):** Daft-local target.
