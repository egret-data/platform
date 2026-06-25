# Explore Trash + Bookmark Parity + Folder Polish (Design)

**Status:** Approved
**Date:** 2026-06-25
**Scope:** Frontend (React/TS) **and** backend (Rust + Postgres migration).
**Sequencing:** Addendum to **Project B** (Explore Parity + Folders), on the same unmerged branch
`feat/explore-parity-folders`. Builds directly on the folders + bulk-select work already there.

## Problem / Goals

Three follow-up items from live review of Project B:

1. **Folder items not visually nested.** Rows under a folder header sit at the same indent as the
   header, so the grouping is hard to read. → indent item rows under each folder header.
2. **No way to create an empty folder.** Folders can only be created indirectly via the row kebab
   "Move to new folder…". → add a **"+ New folder"** header button on all three Explore pages.
3. **Trash + Bookmark exist only for Dashboards.** Charts and Datasets have no soft-delete and no
   bookmark. → bring **Charts and Datasets** up to the Dashboards capability so all three Explore
   pages are identical: bookmark star + Bookmarks filter, Move to Trash (soft delete), Trash view,
   Restore, and Delete permanently.

## Non-Goals

- No change to the Dashboards trash/bookmark behaviour (it is the reference; reuse its shape).
- No folders inside the Trash view (Trash stays a flat list — same as Dashboards today).
- No new bookmark storage — the `bookmarks` table is already polymorphic
  (`migrations/0020_bookmarks.sql`, `(user_id, entity_type, entity_id)`).

## Design — reference is Dashboards (mirror it exactly)

The Dashboards implementation already has everything; Charts and Datasets copy it per type.

### Backend

**Migration `0022_charts_datasets_soft_delete.sql`** (mirror `0019_dashboard_soft_delete.sql`):
```sql
ALTER TABLE charts   ADD COLUMN deleted_at timestamptz NULL;
CREATE INDEX charts_deleted_idx   ON charts   (workspace_id, deleted_at);
ALTER TABLE datasets ADD COLUMN deleted_at timestamptz NULL;
CREATE INDEX datasets_deleted_idx ON datasets (workspace_id, deleted_at);
```
Touch `crates/egret-metastore/src/lib.rs` to re-embed (memory `egret-sqlx-migration-staleness`).

**ChartRepo / DatasetRepo** (mirror `DashboardRepo`):
- `deleted_at: Option<DateTime<Utc>>` on the row struct + in `COLS`.
- `set_trashed(id, bool)` → `UPDATE … SET deleted_at = now()|NULL WHERE id=$1`.
- The list method filters `deleted_at IS NULL` (live) vs `IS NOT NULL` (trash), and returns a
  per-user `bookmarked` flag via `EXISTS (SELECT 1 FROM bookmarks b WHERE b.user_id=$2 AND
  b.entity_type='chart'|'dataset' AND b.entity_id = <table>.id)`. Mirror
  `DashboardRepo::list_by_workspace_for(ws, user, trashed)`.
- `add_bookmark(user, entity_type, id)` / `remove_bookmark(user, entity_type, id)` — these already
  exist generically on `DashboardRepo`; replicate as small methods (or reuse) with entity_type
  `'chart'` / `'dataset'`. An `is_bookmarked` is only needed if the detail endpoint surfaces it
  (Charts detail does not need it; skip unless trivial).

**DTOs** (`model.rs`): `ChartDto` and `DatasetSummaryDto` each gain `deleted_at: Option<DateTime>`
and `bookmarked: bool`. Add a builder variant that sets `bookmarked` from the list query (mirror
`dashboard_summary_dto(d, count, bm)`); the existing `chart_dto(c)` / `summary_dto(ds, n)` keep
working with `bookmarked: false` for non-list callers.

**Routes** (`routes.rs`, mirror the `db_*` handlers):
- `ch_list` / `ds_list` accept `?trashed=` (the existing `ListQuery{trashed}`) and return the
  bookmarked flag.
- New: `POST /ws/{ws}/charts/{id}/trash`, `/restore`; `POST|DELETE /ws/{ws}/charts/{id}/bookmark`.
  Same for `datasets`. Permission: trash/restore require `<type>:manage`; bookmark is per-user
  (any read access). All guarded by the existing in-ws check.
- The existing `DELETE /ws/{ws}/{type}/{id}` stays **permanent delete** (used only from Trash).

### Frontend

**API (`charts.ts`, `datasets.ts`)** — mirror `dashboards.ts`:
- `useCharts(ws, { trashed })` / `useDatasets(ws, { trashed })` add the `?trashed` query + key.
- Mutations `trash`, `restore`, `addBookmark`, `removeBookmark`.
- `Chart` / `DatasetSummary` types gain `bookmarked?: boolean` and `deleted_at?: string | null`.

**Pages (`Charts.tsx`, `Datasets.tsx`)** — mirror `Dashboards.tsx`:
- Header: **★ Bookmarks** toggle (filters `bookmarked`), **Trash** toggle (switches to the
  soft-deleted list), plus the existing Bulk select / + Chart|Create, and the new **+ New folder**
  button (item 2). All folder UI + bookmark/trash toggles hidden appropriately in Trash view.
- Per-row bookmark **star** (only when `!showTrash`), toggling add/removeBookmark.
- Row kebab, **normal view**: existing actions but the destructive item becomes **Move to Trash**
  (soft, no heavy confirm) instead of permanent Delete. **Trash view**: **Restore** + **Delete
  permanently** (typed `ConfirmDeleteDialog`). Mirror `DashboardActionsMenu`'s `trashed` branch.
  `ChartActionsMenu`/`DatasetActionsMenu` gain a `trashed` prop.
- Bulk action, normal view: **Move N to Trash** (replaces bulk permanent Delete); Trash view: no
  bulk (or permanent bulk delete — keep simple: no bulk in Trash, matching Dashboards).
- Folders are shown only in the live view; Trash renders a flat list.

### Folder polish (items 1 & 2)

- **Indent (item 1):** in the grouped tables, give each item row's first content cell extra left
  padding (e.g. `pl-6`) so rows sit visually under the folder header. Applies to all three pages'
  shared rendering (the header row stays at base indent; item rows indent). Keep it subtle.
- **+ New folder (item 2):** a header button that `prompt()`s for a name and calls
  `useFolderMutations(ws, kind).create`. On success the new (empty) folder appears as a section
  (it renders because `groupByFolder` includes empty folders when not filtering). All three pages.

## Interactions / edge cases

- A trashed chart/dataset keeps its `folder_id`; Restore returns it to the live list under its
  folder. Trash view ignores folders (flat).
- Deleting a chart that is referenced by a dashboard card: unchanged — permanent delete still
  cascades per the existing FK (`dashboard_cards.chart_id ON DELETE CASCADE`); Move-to-Trash only
  sets `deleted_at` (the card keeps pointing at it). This matches today's dashboard behaviour and
  is acceptable for this scope.
- Bookmarks are per-user and survive trashing.

## Testing

- **Backend:** repo tests for `set_trashed` (list excludes trashed / Trash view includes it),
  bookmark add/remove + bookmarked flag, for charts and datasets. fmt + clippy + `cargo test`.
- **Frontend (Vitest):** Charts/Datasets — Move-to-Trash from kebab calls `trash`; Trash view shows
  Restore + permanent delete; bookmark star toggles; "+ New folder" calls create. Update the
  existing Charts.test.tsx delete assertions (single delete is now Move-to-Trash).
- **Live (Playwright):** on Charts and Datasets — bookmark a row, filter Bookmarks; Move to Trash →
  appears in Trash; Restore; permanent delete; create an empty folder via the button; confirm item
  rows are visually indented. 0 new console errors.

## Implementation order (one branch, green per commit)

1. Migration `0022` + touch metastore.
2. Charts backend (repo trash+bookmark+list_for, DTO, routes) + test.
3. Datasets backend (same) + test.
4. Frontend API (charts.ts + datasets.ts).
5. Charts page (bookmark/trash/Trash view/ActionsMenu trashed branch; delete→trash) + tests.
6. Datasets page (same).
7. Folder polish: indent items + "+ New folder" button on all three pages.
8. Full sweep + live verification + finish.

## Self-Review notes

- **Spec coverage:** item 1 (indent) → Task 7; item 2 (+New folder) → Task 7; item 3 (trash+bookmark
  for Charts & Datasets) → Tasks 1–6. Each maps to a task.
- **Reuse:** mirrors the existing Dashboards trash/bookmark (DashboardRepo / db_* routes /
  Dashboards.tsx / DashboardActionsMenu) and the polymorphic `bookmarks` table — no new storage,
  no new patterns invented.
- **Consistency:** after this, Charts/Dashboards/Datasets share an identical capability set
  (folders, bulk-select, duplicate where applicable, bookmark, trash) — the Project B goal.
