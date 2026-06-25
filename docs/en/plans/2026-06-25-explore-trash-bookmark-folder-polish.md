# Explore Trash + Bookmark + Folder Polish — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Bring Charts and Datasets up to the Dashboards trash+bookmark capability (Move to Trash, Trash view, Restore, Delete permanently, bookmark star + filter), and add two folder UX fixes (indent items under folders; a "+ New folder" button).

**Architecture:** Backend first — migration adds `deleted_at` to charts/datasets; ChartRepo/DatasetRepo + their routes mirror the existing `DashboardRepo` / `db_*` handlers; the `bookmarks` table is already polymorphic. Then frontend mirrors `dashboards.ts` / `Dashboards.tsx` / `DashboardActionsMenu` for the two pages, plus the folder polish across all three.

**Tech Stack:** Rust (axum, sqlx), React 18 + TS, Vitest, @tanstack/react-query, shadcn.

**Spec:** `docs/en/specs/2026-06-25-explore-trash-bookmark-folder-polish-design.md`

**Conventions:**
- Same branch `feat/explore-parity-folders` (already checked out; this is a Project B addendum). Do NOT switch branches.
- Backend from `egret/`; frontend from `egret/frontend/`. `git add` with `crates/`/`frontend/`/`migrations/`/`docs/` prefixes.
- After the migration, **touch `crates/egret-metastore/src/lib.rs`** to re-embed.
- Backend sweep per commit: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p egret-bi`.
- Frontend sweep per commit: `npm run typecheck && npm run lint && npm run build && npx vitest run`.
- Dev Postgres up at `postgres://egret:egret@localhost:5432/egret`.
- End commit messages with: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

**Reference implementation to mirror (read these):** `crates/egret-bi/src/dashboard.rs` (`set_trashed`, `list_by_workspace_for`, `add_bookmark`/`remove_bookmark`, `deleted_at` in `COLS`); `crates/egret-bi/src/routes.rs` (`db_list` with `ListQuery{trashed}`, `db_trash`, `db_restore`, `db_bookmark_add`, `db_bookmark_remove`, their `.route(...)` registrations ~lines 1062-1071); `crates/egret-bi/src/model.rs` (`DashboardSummaryDto` + `dashboard_summary_dto(d, count, bm)`); `frontend/src/api/dashboards.ts` (`useDashboards({trashed})`, `trash`/`restore`/`addBookmark`/`removeBookmark`); `frontend/src/routes/explore/Dashboards.tsx` (★ Bookmarks + Trash toggles, star, trashed wiring); `frontend/src/components/chart/dashboard/DashboardActionsMenu.tsx` (the `trashed` branch).

---

## Task 1: Migration — `deleted_at` on charts + datasets

**Files:** Create `migrations/0022_charts_datasets_soft_delete.sql`; touch `crates/egret-metastore/src/lib.rs`.

- [ ] **Step 1: Write the migration** (mirror `0019_dashboard_soft_delete.sql`):
```sql
ALTER TABLE charts   ADD COLUMN deleted_at timestamptz NULL;
CREATE INDEX charts_deleted_idx   ON charts   (workspace_id, deleted_at);
ALTER TABLE datasets ADD COLUMN deleted_at timestamptz NULL;
CREATE INDEX datasets_deleted_idx ON datasets (workspace_id, deleted_at);
```
- [ ] **Step 2:** Touch `crates/egret-metastore/src/lib.rs` (no-op whitespace) to re-embed.
- [ ] **Step 3: Apply:** `EGRET_DATABASE__URL="postgres://egret:egret@localhost:5432/egret" cargo run -p egret-bin -- migrate` → `migrations applied`.
- [ ] **Step 4: Commit:**
```bash
git add migrations/0022_charts_datasets_soft_delete.sql crates/egret-metastore/src/lib.rs
git commit -m "feat(db): soft-delete (deleted_at) for charts + datasets

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Charts backend — trash + bookmark

**Files:** `crates/egret-bi/src/chart.rs`, `model.rs`, `routes.rs`; test in `crates/egret-bi/tests/`.

Mirror `DashboardRepo` (`dashboard.rs`) and the `db_*` chart-equivalents in `routes.rs`.

- [ ] **Step 1: ChartRepo (`chart.rs`)**
  - Add `pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,` to `struct Chart` (after `updated_at`, matching COLS order) and append `deleted_at` to `const COLS` (last).
  - `set_trashed(id, trashed: bool)`: `UPDATE charts SET deleted_at = now() WHERE id=$1` (trashed) / `... = NULL ...` (restore). Mirror `DashboardRepo::set_trashed`.
  - Replace/augment the list with `list_by_workspace_for(workspace_id, user, trashed) -> Vec<(Chart, bool)>`: select `{COLS}`, plus `EXISTS (SELECT 1 FROM bookmarks b WHERE b.user_id=$2 AND b.entity_type='chart' AND b.entity_id = charts.id) AS bookmarked`, filtered by `deleted_at IS NULL` (live) / `IS NOT NULL` (trash), `ORDER BY updated_at DESC`. Use a `#[derive(FromRow)]` row wrapper like dashboard.rs does (a struct holding `#[sqlx(flatten)] chart: Chart` + `bookmarked: bool`), and map to `(chart, bookmarked)`. Keep the old `list_by_workspace` only if still referenced elsewhere; otherwise update callers.
  - `add_bookmark(user, entity_id)` / `remove_bookmark(user, entity_id)`: INSERT … ON CONFLICT DO NOTHING / DELETE, with `entity_type='chart'`. (Mirror dashboard's generic versions but fix entity_type to 'chart'.)
- [ ] **Step 2: ChartDto (`model.rs`)** — add `pub deleted_at: Option<chrono::DateTime<chrono::Utc>>` and `pub bookmarked: bool`. In `chart_dto(c)` set `bookmarked: false`, `deleted_at: c.deleted_at`. Add `pub fn chart_dto_bm(c: Chart, bookmarked: bool) -> ChartDto` (same as chart_dto but sets bookmarked).
- [ ] **Step 3: Routes (`routes.rs`)**
  - `ch_list`: take `AxQuery(q): AxQuery<ListQuery>` (the existing `ListQuery{trashed}`), call `list_by_workspace_for(ws, user, q.trashed)`, map with `chart_dto_bm`.
  - Add handlers `ch_trash`, `ch_restore` (require `CH_MANAGE`, `require_chart_in_ws`, `set_trashed(id,true/false)`), `ch_bookmark_add`/`ch_bookmark_remove` (require `CH_READ`, `require_chart_in_ws`, add/remove_bookmark). Mirror `db_trash`/`db_restore`/`db_bookmark_*`.
  - Register on `chart_router()`: `.route("/ws/{workspace}/charts/{id}/trash", post(ch_trash))`, `.route(".../restore", post(ch_restore))`, `.route(".../bookmark", post(ch_bookmark_add).delete(ch_bookmark_remove))`.
  - The existing `DELETE /charts/{id}` stays = permanent.
- [ ] **Step 4: Test** (mirror the dashboard repo test) in `crates/egret-bi/tests/` (extend an existing chart test file or add one): seed a chart; `set_trashed(id,true)` → `list_by_workspace_for(ws,user,false)` excludes it, `(…,true)` includes it; `set_trashed(id,false)` restores; `add_bookmark` → bookmarked flag true; `remove_bookmark` → false.
- [ ] **Step 5: Sweep + commit:** `cargo fmt --all -- --check && cargo clippy -p egret-bi --all-targets -- -D warnings && cargo test -p egret-bi`. Then:
```bash
git add crates/egret-bi/src/chart.rs crates/egret-bi/src/model.rs crates/egret-bi/src/routes.rs crates/egret-bi/tests/
git commit -m "feat(bi): charts trash + bookmark (mirror dashboards)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Datasets backend — trash + bookmark

**Files:** `crates/egret-bi/src/dataset.rs`, `model.rs`, `routes.rs`; test.

Same as Task 2 but for datasets (`entity_type='dataset'`, table `datasets`, perms `DS_*`, guard `require_dataset_in_ws`).

- [ ] **Step 1: DatasetRepo** — `deleted_at` on `struct Dataset` + `COLS`; `set_trashed`; `list_by_workspace_for(ws, user, trashed) -> Vec<(Dataset, bool)>` with the `entity_type='dataset'` bookmark EXISTS and `deleted_at` filter; `add_bookmark`/`remove_bookmark` (entity_type 'dataset'). NOTE: the dataset list also needs `column_count` — check how `ds_list` builds `DatasetSummaryDto` today (it uses `summary_dto(ds, column_count)`); keep returning column_count alongside, i.e. the list method returns `(Dataset, column_count, bookmarked)` OR fetch counts as today and add bookmarked. Match the existing dataset list shape; just add the `bookmarked` flag and the `trashed` filter.
- [ ] **Step 2: DatasetSummaryDto (`model.rs`)** — add `deleted_at: Option<...>` + `bookmarked: bool`; update `summary_dto` (default bookmarked false) and add a `summary_dto_bm(ds, column_count, bookmarked)` builder.
- [ ] **Step 3: Routes** — `ds_list` reads `?trashed` + returns bookmarked; add `ds_trash`/`ds_restore` (DS_MANAGE) + `ds_bookmark_add`/`ds_bookmark_remove` (DS_READ); register `/datasets/{id}/trash`, `/restore`, `/bookmark` on the dataset router. `DELETE /datasets/{id}` stays permanent.
- [ ] **Step 4: Test** — mirror Task 2's repo test for datasets.
- [ ] **Step 5: Sweep + commit:**
```bash
git add crates/egret-bi/src/dataset.rs crates/egret-bi/src/model.rs crates/egret-bi/src/routes.rs crates/egret-bi/tests/
git commit -m "feat(bi): datasets trash + bookmark (mirror dashboards)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Frontend API — charts + datasets trash/bookmark

**Files:** `frontend/src/api/charts.ts`, `frontend/src/api/datasets.ts`.

Mirror `frontend/src/api/dashboards.ts`.

- [ ] **Step 1: charts.ts**
  - `interface Chart`: add `bookmarked?: boolean;` and `deleted_at?: string | null;`.
  - `useCharts(ws, opts?: { trashed?: boolean })`: append `?trashed=true` when set and include `{trashed}` in the query key (mirror `useDashboards`).
  - In `useChartMutations`, add: `trash` (`POST /ws/${ws}/charts/${id}/trash`), `restore` (`POST .../restore`), `addBookmark` (`POST .../bookmark`), `removeBookmark` (`DELETE .../bookmark`), each `onSuccess: invalidate`. Return them.
- [ ] **Step 2: datasets.ts** — same: `DatasetSummary` gains `bookmarked?`/`deleted_at?`; `useDatasets(ws,{trashed})`; add `trash`/`restore`/`addBookmark`/`removeBookmark` mutations. NOTE datasets currently has separate hooks (`useDeleteDataset` etc.) rather than one `useDatasetMutations` for the list — add a small `useDatasetListMutations(ws)` (or extend the existing pattern) exposing trash/restore/bookmark; keep `useDeleteDataset` as the permanent delete. Match the file's existing style.
- [ ] **Step 3:** `npm run typecheck` clean. Fix any test fixtures needing the new optional fields (they're optional, so likely none).
- [ ] **Step 4: Commit:**
```bash
git add frontend/src/api/charts.ts frontend/src/api/datasets.ts
git commit -m "feat(api): charts + datasets trash/restore/bookmark + trashed list

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Charts page — bookmark + Trash view + delete→trash

**Files:** `frontend/src/routes/explore/Charts.tsx`, `frontend/src/components/chart/ChartActionsMenu.tsx`, `frontend/src/routes/explore/__tests__/Charts.test.tsx`.

Mirror `Dashboards.tsx` + `DashboardActionsMenu.tsx`.

- [ ] **Step 1: ChartActionsMenu** — add a `trashed: boolean` prop and the two branches:
  - `trashed`: `Restore` (`onRestore(chart)`) + separator + `Delete permanently` (`onDeletePermanent(chart)`, destructive).
  - normal: `Open`, `Duplicate`, `<MoveToFolderItems …/>`, separator, then **Move to Trash** (`onTrash(chart)`) instead of the current destructive Delete.
- [ ] **Step 2: Charts.tsx**
  - Add `const [showTrash, setShowTrash] = useState(false)` and `const [onlyBookmarked, setOnlyBookmarked] = useState(false)`; `useCharts(ws, { trashed: showTrash })`.
  - From `useChartMutations`: also take `trash, restore, addBookmark, removeBookmark` (keep `remove` for permanent delete).
  - Header: add **★ Bookmarks** and **Trash** toggle buttons (mirror Dashboards), plus keep Bulk select / + Chart. Hide Bulk select, type filter, bookmark filter appropriately in Trash view (mirror Dashboards: in trash it shows just the list).
  - `rows` filter: when `onlyBookmarked`, keep only `c.bookmarked`.
  - Per-row bookmark **star** (only when `!showTrash`) toggling add/removeBookmark (mirror Dashboards' star markup).
  - `<ChartActionsMenu trashed={showTrash} onTrash=… onRestore=… onDeletePermanent=… onDuplicate=… onMove=… onNewFolder=… />` where onTrash → `trash.mutateAsync(c.id).then(()=>toast.success("Moved to Trash"))`, onRestore → `restore.mutateAsync`, onDeletePermanent → opens a `ConfirmDeleteDialog` (import from `@/components/ui/confirm-delete-dialog`, as Dashboards does) → `remove.mutateAsync`.
  - Bulk action in normal view becomes **Move N to Trash** (`trash` each selected) instead of permanent delete; no bulk in Trash view.
  - Folders/group-inline only when `!showTrash`; Trash renders a flat `<tbody>` (mirror the Dashboards trash path).
- [ ] **Step 3: Update `Charts.test.tsx`** — the single-delete test now asserts **Move to Trash** (kebab item "Move to Trash" → calls `trash` mutation), not the old permanent-delete dialog. Add the new mutations to the `useChartMutations` mock (`trash`/`restore`/`addBookmark`/`removeBookmark` each `{mutateAsync: vi.fn().mockResolvedValue({})}`). Keep the Duplicate + bulk + folder tests passing (mock `useCharts` to return `bookmarked:false`).
- [ ] **Step 4: Sweep + commit:** `npm run typecheck && npm run lint && npx vitest run src/routes/explore`. Then:
```bash
git add frontend/src/routes/explore/Charts.tsx frontend/src/components/chart/ChartActionsMenu.tsx frontend/src/routes/explore/__tests__/Charts.test.tsx
git commit -m "feat(charts): bookmark + Trash view (delete→trash, restore, permanent)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Datasets page — bookmark + Trash view + delete→trash

**Files:** `frontend/src/routes/explore/Datasets.tsx`, `frontend/src/components/dataset/DatasetActionsMenu.tsx`, any Datasets test.

Same mirror as Task 5 for Datasets. Differences: Datasets has Edit/Refresh/Duplicate(virtual)/Delete — in normal view the destructive item becomes **Move to Trash**; Trash view = Restore + Delete permanently; Edit/Refresh hidden in trash. Keep bulk → **Move N to Trash** in normal view (it was bulk delete). Datasets has no existing page test, so no test to update; add a brief Vitest for the Move-to-Trash kebab if practical (optional).

- [ ] **Step 1:** `DatasetActionsMenu` gains `trashed` prop + the two branches (mirror Task 5).
- [ ] **Step 2:** `Datasets.tsx`: `showTrash`/`onlyBookmarked` state, `useDatasets(ws,{trashed:showTrash})`, ★ Bookmarks + Trash toggles, per-row star, kebab trashed branch, bulk → Move to Trash, `ConfirmDeleteDialog` for permanent delete, folders only when `!showTrash`.
- [ ] **Step 3: Sweep + commit:** `npm run typecheck && npm run lint && npx vitest run`. Then:
```bash
git add frontend/src/routes/explore/Datasets.tsx frontend/src/components/dataset/DatasetActionsMenu.tsx frontend/src/routes/explore/__tests__/
git commit -m "feat(datasets): bookmark + Trash view (delete→trash, restore, permanent)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Folder polish — indent items + "+ New folder" button

**Files:** `frontend/src/routes/explore/Charts.tsx`, `Dashboards.tsx`, `Datasets.tsx` (+ possibly `FolderHeaderRow.tsx`).

- [ ] **Step 1: Indent (item 1)** — give each grouped item row's FIRST content cell extra left padding so rows sit under the folder header. Implement once and apply on all three pages: in each page's grouped item `<tr>`, add a left-padding class (e.g. `pl-6`) to the first `<td>` (the name cell / or the bulk-checkbox cell when present). Keep the folder header row at base indent. Subtle (~1.5rem). Ensure alignment with the `<thead>` is still acceptable (the header columns don't indent; only the body name cell). If cleaner, add an optional `indent?: boolean` styling prop pattern — but a `pl-6` on the name cell is sufficient.
- [ ] **Step 2: + New folder button (item 2)** — on each page header (next to Bulk select / + Chart|Create, in the live view only), add a button **"+ New folder"** that does `const n = prompt("New folder name"); if (n) createFolder.mutateAsync(n).catch((e)=>toast.error(String(e)))` using `useFolderMutations(ws, kind).create`. The new empty folder appears as a section (groupByFolder includes empty folders when not filtering). Hide the button in Trash view.
- [ ] **Step 3: Sweep + commit:** `npm run typecheck && npm run lint && npm run build && npx vitest run`. Then:
```bash
git add frontend/src/routes/explore/Charts.tsx frontend/src/routes/explore/Dashboards.tsx frontend/src/routes/explore/Datasets.tsx frontend/src/components/explore/FolderHeaderRow.tsx
git commit -m "feat(explore): indent items under folders + New folder button

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Full sweep, live verification, finish

**Files:** none.

- [ ] **Step 1: Sweep** — backend (`cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p egret-bi`) and frontend (`npm run typecheck && npm run lint && npm run build && npx vitest run`). All green.
- [ ] **Step 2: Live verify (Playwright)** — rebuild + restart `egret-bin serve`; log in `admin@egret.local`/`Admin12345!`. On **Charts** and **Datasets**: bookmark a row + filter ★ Bookmarks; **Move to Trash** → row leaves the live list and appears under **Trash**; **Restore**; **Delete permanently** (confirm dialog); click **+ New folder** → an empty folder section appears; confirm item rows are **indented** under folder headers. 0 new console errors beyond the known favicon-404 / Trino-warmup 502s. Clean up any test rows afterward (or via DB).
- [ ] **Step 3: Finish** — `git status` clean; dispatch a final reviewer over `git diff main...HEAD` for the new commits (focus: trash/bookmark route guards, ch_list/ds_list bookmarked wiring, delete→trash UX on all three pages). Then ask the user how to finish the branch.

## Self-Review notes
- **Spec coverage:** migration → T1; Charts BE → T2; Datasets BE → T3; FE API → T4; Charts page → T5; Datasets page → T6; folder indent + New-folder (items 1 & 2) → T7; verify → T8.
- **Mirror fidelity:** every backend piece names its Dashboards reference (`set_trashed`, `list_by_workspace_for`, `add_bookmark`, `db_trash`/`db_restore`/`db_bookmark_*`, `dashboard_summary_dto`); every frontend piece mirrors `dashboards.ts` / `Dashboards.tsx` / `DashboardActionsMenu`.
- **Behaviour change called out:** normal-view single + bulk delete on Charts/Datasets become **Move to Trash**; permanent delete moves to the Trash view (matches Dashboards). Charts.test.tsx updated for this.
- **Green-per-commit:** T1 migration; T2/T3 each compile + test; T4 types/hooks; T5/T6 page mirrors with tests; T7 polish; T8 verifies.
