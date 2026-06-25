# Explore Parity + Folders (Design)

**Status:** Approved
**Date:** 2026-06-25
**Scope:** Frontend (React/TS) **and** backend (Rust + Postgres migration). This is the
backend-touching follow-up that Project A (Explore List Consistency) deferred.
**Sequencing:** This is **Project B** of the Explore effort, building on Project A
(PR #60, merged). It combines two things the user asked to ship together:
1. **Parity sweep** — bring Charts and Dashboards up to the Datasets feature set (Duplicate,
   bulk-select).
2. **Folders** — per-type, flat folders/groups for datasets/charts/dashboards (the piece
   Project A explicitly forwarded to "Project B": needs a backend table + routes).

## Problem

After Project A the three Explore lists are visually consistent, but their **capabilities**
still differ, and there is no way to organize a growing list:

- **Datasets** is the capability reference: row actions Edit / Refresh / Duplicate (virtual) /
  Delete, plus a **Bulk select** mode (multi-select → Delete N with an `AlertDialog`).
- **Charts** has only Open / Delete — **no Duplicate** (chart mutations are create/update/remove
  only; there is no duplicate endpoint) and **no bulk select**.
- **Dashboards** has Open / Duplicate / Move to Trash (and Restore / Delete permanently in
  Trash) — it already has a backend `POST /dashboards/{id}/duplicate`, but **no bulk select**.

And none of the three can be **organized into folders**. As workspaces accumulate dozens of
charts/dashboards/datasets, a flat list does not scale.

## Goals

1. **Parity:** Charts gains Duplicate; Charts and Dashboards gain bulk-select. Reach the
   Datasets capability bar (Datasets itself adopts the shared bulk-select with no behaviour
   change).
2. **Folders:** per-type, flat (one level) folders for each resource type, presented as
   collapsible inline groups, with move-to-folder via the row kebab, via bulk action, and via
   drag-drop.

## Non-Goals

- **No nested folders / no unified Metabase-style collections.** Folders are per-type and flat
  (decided in brainstorming). A folder of charts is distinct from a folder of dashboards.
- **No per-user folders.** Folders are workspace-shared, managed under the existing
  `<resource>:manage` permission. (Bookmarks stay per-user; folders are org structure.)
- **No responsive overhaul, no seed-data cleanup in code** (carried over from Project A).
- **No client-side chart duplicate.** Charts get a real backend endpoint (mirrors dashboards)
  rather than the client-side copy Datasets uses, to safely satisfy `chart_name_uniq`.

## Design

### Decision A — Folder model: one polymorphic table

Mirror the existing `bookmarks` polymorphic pattern (`migrations/0020_bookmarks.sql`).

```sql
-- New migration: folders + folder_id columns
CREATE TABLE folders (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    entity_type  text NOT NULL,           -- 'chart' | 'dashboard' | 'dataset'
    name         text NOT NULL,
    created_by   uuid REFERENCES users(id),
    created_at   timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT folder_name_uniq UNIQUE (workspace_id, entity_type, name)
);
CREATE INDEX folders_ws_type_idx ON folders (workspace_id, entity_type);

ALTER TABLE charts     ADD COLUMN folder_id uuid REFERENCES folders(id) ON DELETE SET NULL;
ALTER TABLE dashboards ADD COLUMN folder_id uuid REFERENCES folders(id) ON DELETE SET NULL;
ALTER TABLE datasets   ADD COLUMN folder_id uuid REFERENCES folders(id) ON DELETE SET NULL;
```

- **One folder per resource** (`folder_id` nullable column). `NULL` = Uncategorized.
- **Delete folder → `ON DELETE SET NULL`**: items fall back to Uncategorized, never lost.
- **Cross-type integrity** (a chart must not land in a dashboard-folder) is enforced in the
  repo/API layer: the move handler verifies `folder.entity_type == 'chart'` and
  `folder.workspace_id == ws` before assigning. A DB-level CHECK across the discriminator is not
  practical with a single FK; the application guard plus the per-type-scoped folder list is the
  boundary. This matches how `bookmarks` is kept consistent in the app layer today.

After adding the migration, **touch `crates/egret-metastore/src/lib.rs`** so the embedded
migrator re-embeds (known staleness footgun — see project memory `egret-sqlx-migration-staleness`).

### Decision B — Chart Duplicate: backend endpoint

Add `POST /api/ws/{workspace}/charts/{id}/duplicate`, mirroring `db_duplicate` in
`crates/egret-bi/src/routes.rs:1193`. Server-side:
1. Load the chart (workspace-scoped; 404 if not in `ws`).
2. Insert a copy with the same `dataset_id`, `viz_type`, `params`, `description`, `folder_id`,
   `created_by = caller`, and a collision-free name: `"<name> (copy)"`, then `"(copy 2)"`, … to
   satisfy `chart_name_uniq (workspace_id, name)`.
3. Return the new `Chart`.

Reuse whatever name-suffix helper `db_duplicate` uses; if dashboards inline it, extract a small
shared `unique_copy_name(existing_names, base)` helper rather than duplicating the logic.

Frontend: add `duplicate` to `useChartMutations` (`POST .../charts/{id}/duplicate`, invalidate
list) and a `Duplicate` item to `ChartActionsMenu`.

### Decision C — Folder API (per type, parallel route sets)

For each `{type}` in `charts` | `dashboards` | `datasets`:

| Method & path | Action |
|---|---|
| `GET /api/ws/{ws}/{type}/folders` | list folders of this type (with item counts) |
| `POST /api/ws/{ws}/{type}/folders` `{name}` | create folder |
| `PATCH /api/ws/{ws}/{type}/folders/{id}` `{name}` | rename |
| `DELETE /api/ws/{ws}/{type}/folders/{id}` | delete (items → Uncategorized via SET NULL) |
| `PATCH /api/ws/{ws}/{type}/{resourceId}` `{folder_id}` | move a resource into / out of a folder |

- Guard: `{type}/folders` rows are filtered by `entity_type` matching the route; mutations check
  `workspace_id == ws`. The move endpoint validates that both the resource **and** the target
  folder belong to `ws` and that the folder's `entity_type` matches — a cross-tenant / cross-type
  move is a 404/400, not a silent write (see memory `api-nested-resource-ownership-scoping`).
- The resource-list endpoints (`GET …/charts`, etc.) already return each row; add `folder_id`
  to the summary payload so the FE can group without an extra round-trip.
- Permission: folder + move mutations require `<type>:manage` (e.g. `chart:manage`).

### Decision D — Shared bulk-select

Extract the multi-select machinery currently inlined in `Datasets.tsx` (`bulkMode`, `selected`
set, select-all, `<Checkbox>` column, bulk `AlertDialog`) into:
- `useBulkSelect<T>(rows)` hook — `{ bulkMode, toggleBulk, selected, toggle, toggleAll,
  allSelected, someSelected, clear }`.
- a small `<BulkBar>` (count + action buttons) and a reusable checkbox `<th>/<td>` pattern.

Apply to all three pages:
- **Datasets:** Delete N (unchanged behaviour, now via the shared hook).
- **Charts:** Delete N (new) — `AlertDialog` confirm, same wording style as the single-delete.
- **Dashboards:** Move N to Trash (new) — light, no heavy confirm (matches single Move to Trash;
  it is reversible).
- Once folders land, all three also get **Move N to folder…** in the bulk bar.

### Decision E — Group-inline folder UI

No sidebar. Each list renders its rows **grouped into collapsible sections by folder**:

```
Charts                                   [Bulk select] [+ Chart]
🔍 Search    ▾ Type
▾ Revenue (5)                            ⋯  (header kebab: Rename / Delete)
   □ Revenue by category   Bar    2d   ⋯
   □ Revenue by state      Map    2d   ⋯
▾ Funnels (3)                            ⋯
   □ Checkout funnel       Funnel 2d   ⋯
▸ Uncategorized (4)
```

- **Folder order:** alphabetical (A→Z); **Uncategorized pinned last**.
- **Within a section:** the Project A `SortableHeader` sorts rows *inside* each section; **search
  and type-filter span all folders** (matching items show under their folder; empty folders are
  hidden while a search/filter is active).
- **Collapse state** persists in `localStorage`, keyed by `ws + entityType + folderId`; default
  expanded.
- Shared `<FolderSection>` component (header with count + collapse chevron + kebab, body slot)
  and a `useFolderGroups(rows, folders)` helper that buckets rows and appends Uncategorized.
  Used by all three pages so the three lists stay consistent (the Project A philosophy).
- **Move-to-folder (single):** the row kebab gains `Move to folder ▸` → submenu listing this
  type's folders + `New folder…` (inline create). Built on the Project A `RowActionsMenu` shell.
- **Section header kebab:** `Rename` (inline) / `Delete` (folder only; items survive).

### Decision F — Drag-drop (separate task, same PR)

Drag a row onto another section header to move it between folders, as an **enhancement** over the
kebab (the kebab is the accessible primary path). Implemented with `dnd-kit`. Kept as its own
task/commit so it can be split out if it risks the PR's green state.

### Interactions / edge cases

- **Dashboards trash:** trashed dashboards stay out of the folder view (Trash remains its own
  view as today). `folder_id` is retained, so Restore returns the dashboard to its folder.
- **Charts still have no per-row "Edit"** label — `Open` already routes to the editor; that is
  parity with Datasets' Edit, not a gap.
- **Bulk select + collapsed groups:** select-all selects all loaded rows regardless of collapse
  state; collapsing a group does not deselect.
- **Empty folder:** shows with `(0)` and is expandable (so it is a visible drop target / move
  destination) when no search/filter is active.

## Testing

- **Backend (Rust, `egret-bi`):** folders CRUD; `ON DELETE SET NULL` falls items back to
  Uncategorized; cross-type and cross-workspace move rejection; chart duplicate including the
  name-collision suffix path. Run the manual real-Trino integration only if touching
  catalog/authz (not expected here).
- **Frontend (Vitest):** `useBulkSelect` (toggle, select-all, clear); `<FolderSection>` (count,
  collapse, Uncategorized-last, empty state); Chart Duplicate action; single + bulk move-to-folder;
  bulk Delete (Charts) / Trash (Dashboards). Drag-drop core logic unit-tested where jsdom allows;
  the gesture itself is covered by live verify.
- **Sweep:** `npm run typecheck && npm run lint && npm run build && npx vitest run` (frontend);
  `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test`
  (backend — fmt is a CI gate, see memory `egret-cargo-fmt-ci`).
- **Live verify (Playwright):** duplicate a chart; bulk-delete charts; bulk-trash dashboards;
  create/rename/delete a folder; move via kebab, via bulk, via drag-drop; collapse persistence
  across reload; 0 new console errors beyond the known favicon-404 / Trino-warmup 502s.

## Implementation order (one PR, green per commit)

**Backend first (each compiles + tests green):**
1. Migration: `folders` table + `folder_id` columns; touch `egret-metastore/src/lib.rs`.
2. Folder repo + CRUD routes (per type) + `folder_id` in resource summaries.
3. Move endpoint (`PATCH …/{resourceId}` with ownership/type guard).
4. Chart duplicate endpoint (+ shared `unique_copy_name` helper).

**Frontend:**
5. `useBulkSelect` + `<BulkBar>` extracted from Datasets; Datasets refactored (no behaviour change).
6. Chart Duplicate (mutation + menu item).
7. Bulk Delete (Charts) + bulk Move to Trash (Dashboards).
8. `useFolderGroups` + `<FolderSection>`; group-inline rendering on all three pages.
9. Move-to-folder: row kebab submenu + bulk "Move N to folder…".
10. Drag-drop (dnd-kit) — separate commit.
11. Full sweep + live verification + finish branch.

## Self-Review notes

- **Spec coverage:** Parity → Decisions B (chart duplicate) + D (bulk-select). Folders →
  Decisions A (schema) + C (API) + E (UI) + F (drag-drop). Each maps to numbered tasks above.
- **Reuses Project A:** `RowActionsMenu` (kebab) hosts Move-to-folder; `SortableHeader` sorts
  within sections; `RelativeTime`/`ListSkeleton`/`Skeleton` unchanged. Bulk-select is extracted
  from the existing Datasets implementation, not invented.
- **Backend consistency:** chart duplicate mirrors the existing dashboard `db_duplicate`; folder
  routes mirror the existing per-type resource routes; move guard follows the nested-ownership
  rule already used elsewhere.
- **Green-per-commit:** backend tasks 1–4 land with their own tests; FE task 5 is a pure refactor
  (existing Datasets tests stay green); 6–10 each add isolated, tested units.
