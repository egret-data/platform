# Tier 3a — Dashboard view polish (design)

**Status:** approved (brainstorm) · **Date:** 2026-06-22 · **Branch:** `feat/dashboards-tier3a` (off main)
**Part of:** the dashboard Metabase-parity program ([audit](./2026-06-22-dashboard-metabase-parity-audit.md))

## Goal

Close the visible "view polish" gaps between egret and Metabase dashboards: an **info sidebar**, **duplicate dashboard**, **trash/soft-delete** (replacing the current hard delete), and **bookmark**. Plus a dark-mode audit (egret's global theme already covers the dashboard; this only verifies + fixes stray colors).

Trash and Bookmark are inherently platform-wide, but this tier wires them **dashboard-only** with a **generic schema** so charts/datasets can adopt them later without re-migrating.

## Scope

**In:** (1) dark-mode audit (no new toggle); (2) info sidebar (frontend-only); (3) duplicate dashboard (1 endpoint + UI); (4) trash/soft-delete + restore + permanent-delete (migration + UI); (5) bookmark (generic table + dashboard-scoped endpoints + UI).

**Out / deferred:** dashboard-scoped night toggle (global theme suffices — YAGNI); trash/bookmark UI for charts/datasets (schema is generic, wiring deferred); bookmark folders/ordering; trash auto-purge/retention; dashboard edit history/versioning (Metabase's info-sidebar "history" tab is out).

## Existing code touched

- Backend: `crates/egret-bi/src/dashboard.rs` (`DashboardRepo`), `routes.rs` (`dashboard_router`, handlers, `require_dashboard_owner`), `model.rs` (DTOs). Current routes: `POST/GET /ws/{w}/dashboards`, `GET/PUT/DELETE /ws/{w}/dashboards/{id}`. Current user id is `ctx.principal.user_id.0`; owner-or-admin guard is `require_dashboard_owner`.
- Frontend: `routes/explore/Dashboards.tsx` (list), `routes/explore/DashboardView.tsx` (header), `api/dashboards.ts` (types + hooks).
- Migrations: `migrations/` (sqlx, compile-time embedded). **After adding a migration, touch `crates/egret-metastore/src/lib.rs`** so `serve` re-embeds it (known staleness gotcha).

## Data model (2 migrations)

```sql
-- migrations/0019_dashboard_soft_delete.sql
ALTER TABLE dashboards ADD COLUMN deleted_at timestamptz NULL;
CREATE INDEX dashboards_deleted_idx ON dashboards (workspace_id, deleted_at);

-- migrations/0020_bookmarks.sql  (generic, dashboard-only wiring for now)
CREATE TABLE bookmarks (
  user_id     uuid        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  entity_type text        NOT NULL,            -- 'dashboard' for this tier
  entity_id   uuid        NOT NULL,
  created_at  timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (user_id, entity_type, entity_id)
);
CREATE INDEX bookmarks_user_idx ON bookmarks (user_id, entity_type);
```

`bookmarks` has no FK to a specific entity table (it is polymorphic by `entity_type`); a deleted dashboard's bookmark rows are cleaned up by the repo on permanent delete (see below). Soft-delete uses `deleted_at IS NULL` as the "live" predicate everywhere.

## Feature 1 — Dark-mode audit (frontend, ~0 work)

The dashboard already renders correctly in the global dark theme (canvas/cards/pills/charts verified). One task: walk view + edit + filter popovers + the new Tier 3a UI (info sidebar, trash view) in dark mode; replace any hardcoded light color with a theme token. No dashboard-scoped toggle is added.

## Feature 2 — Info sidebar (frontend-only)

- Header gets an **ⓘ "info"** button (in `DashboardView` header, next to Fullscreen/Edit). Clicking opens a right slide-over panel.
- Panel shows, from the already-loaded `DashboardDetail`: **Owner** (`created_by` resolved to user email/name), **Last edited** (`updated_at`, localized), and **Description** as an inline-editable textarea.
- Saving the description calls the existing `PUT /ws/{w}/dashboards/{id}` (which already accepts `description` alongside name/parameters/width) with the current layout unchanged. Owner-or-admin only (the PUT is already owner-gated).
- No new endpoint. (There is no `created_at` column, so no "Created" row.)

## Feature 3 — Duplicate dashboard (backend + frontend)

- New endpoint `POST /ws/{w}/dashboards/{id}/duplicate` → `db_duplicate`. Requires `require_dashboard_in_ws` (read access); any member may duplicate.
- `DashboardRepo::duplicate(src_id, new_owner) -> Dashboard`: insert a new dashboard row (new id, `name = "<name> (copy)"`, same `description`/`parameters`/`width`, `created_by = new_owner`, `deleted_at = NULL`), then copy tabs (new ids) and cards (new ids, `tab_id` remapped via the old→new tab map), preserving grid/options/ordinal. Runs in one transaction. Reuses the tab/card insert SQL shape from `replace_layout`.
- Returns the new `DashboardSummary` (or detail); frontend navigates to `/explore/{w}/dashboards/{newId}`.
- Frontend: "Duplicate" action in the list-row overflow (`Dashboards.tsx`) and the dashboard header overflow (`DashboardView.tsx`).

## Feature 4 — Trash / soft-delete (backend migration + frontend)

- `db_list` (`GET /ws/{w}/dashboards`) gains a `?trashed=<bool>` query (default `false`): `false` → `WHERE deleted_at IS NULL`; `true` → `WHERE deleted_at IS NOT NULL`. `list_by_workspace` takes a `trashed: bool` arg.
- New `POST /ws/{w}/dashboards/{id}/trash` → `db_trash` → `DashboardRepo::set_trashed(id, true)` sets `deleted_at = now()`. Owner-or-admin (`require_dashboard_owner`).
- New `POST /ws/{w}/dashboards/{id}/restore` → `db_restore` → `set_trashed(id, false)` sets `deleted_at = NULL`. Owner-or-admin.
- `DELETE /ws/{w}/dashboards/{id}` (`db_remove`) stays as **permanent delete** (hard `DELETE`, cascades cards/tabs; also delete `bookmarks WHERE entity_type='dashboard' AND entity_id=$1`). Used by the Trash view's "Delete permanently".
- `GET /ws/{w}/dashboards/{id}` (`db_get`) is unchanged — a trashed dashboard still resolves by deep link (the Trash view drives restore/purge). The detail view shows a small "In Trash — Restore" banner when `deleted_at` is set (add `deleted_at` to the detail DTO).
- Frontend (`Dashboards.tsx`): the row "Delete" becomes **"Move to trash"** (calls `/trash`). A **Trash toggle/section** lists trashed dashboards (`?trashed=true`) with **Restore** and **Delete permanently** (the existing `DELETE`, behind a confirm).

## Feature 5 — Bookmark (backend table + frontend)

- `DashboardRepo` (or a small `BookmarkRepo`) methods: `add_bookmark(user, 'dashboard', id)` (INSERT … ON CONFLICT DO NOTHING — idempotent), `remove_bookmark(user, 'dashboard', id)`.
- `db_list` includes a per-user `bookmarked` boolean (`EXISTS (SELECT 1 FROM bookmarks b WHERE b.user_id=$user AND b.entity_type='dashboard' AND b.entity_id=dashboards.id)`); add `bookmarked` to `DashboardSummary` DTO. Detail DTO likewise gets `bookmarked`.
- Endpoints: `POST /ws/{w}/dashboards/{id}/bookmark` → add; `DELETE /ws/{w}/dashboards/{id}/bookmark` → remove. Require `require_dashboard_in_ws` (any member bookmarks for themselves).
- Frontend: a **star toggle** in the dashboard header (`DashboardView`) and each list row; a **Bookmarks** section/filter on the list (client-side filter on the `bookmarked` flag, or a `?bookmarked=true` later — client-side filter for this tier).

## API summary

| Method | Path | Handler | Guard |
|---|---|---|---|
| GET | `/ws/{w}/dashboards?trashed&` | `db_list` (+ `bookmarked` flag) | member |
| POST | `/ws/{w}/dashboards/{id}/duplicate` | `db_duplicate` | member (read) |
| POST | `/ws/{w}/dashboards/{id}/trash` | `db_trash` | owner/admin |
| POST | `/ws/{w}/dashboards/{id}/restore` | `db_restore` | owner/admin |
| DELETE | `/ws/{w}/dashboards/{id}` | `db_remove` (permanent) | owner/admin |
| POST | `/ws/{w}/dashboards/{id}/bookmark` | `db_bookmark_add` | member |
| DELETE | `/ws/{w}/dashboards/{id}/bookmark` | `db_bookmark_remove` | member |

Frontend `api/dashboards.ts` gains hooks: `useDuplicateDashboard`, `useTrashDashboard`, `useRestoreDashboard`, `useAddBookmark`, `useRemoveBookmark`; `useDashboards(ws, { trashed })`; `DashboardSummary`/`DashboardDetail` gain `bookmarked: boolean` and `deleted_at: string | null`.

## Error handling

- Duplicate of a missing/foreign dashboard → 404 via `require_dashboard_in_ws`.
- Trash/restore by non-owner non-admin → 403 (`require_dashboard_owner`).
- Bookmark add is idempotent (ON CONFLICT DO NOTHING); remove of a non-bookmark is a no-op (0 rows).
- Permanent delete also purges `bookmarks` rows for that entity (no orphan stars).
- Restoring a dashboard whose name now collides with a live one: the unique constraint is `(workspace_id, name)` — restore may hit `dashboard_name_uniq`. Handle by surfacing a 409 ("a dashboard with this name exists; rename first") rather than auto-renaming. (Duplicate avoids this by suffixing "(copy)".)
- Trashed dashboards are excluded from the default list and from the name-uniqueness practical conflict only at restore time.

## Testing

- **Backend** (`crates/egret-bi/tests/`, DB-gated): `duplicate` copies tabs+cards with remapped ids and new owner; `set_trashed`/list `trashed` filter (live list excludes trashed, trash list includes); restore; permanent delete purges bookmarks; `add/remove_bookmark` idempotency + `bookmarked` flag in list; restore name-collision → conflict.
- **Frontend** (Vitest): info sidebar renders owner/last-edited/description + description edit calls update; duplicate action calls hook + navigates; list "Move to trash" + Trash section restore/permanent-delete; bookmark star toggles add/remove and reflects `bookmarked`.
- **Live** (Playwright MCP, side-by-side Metabase): info sidebar; duplicate → copy opens; move-to-trash → leaves list → Trash → restore; bookmark star + Bookmarks filter; dark-mode pass over all new UI. **0 console errors.**
- **Migration staleness:** after adding the two migrations, `touch crates/egret-metastore/src/lib.rs` before `cargo build`/`serve`.
- **Full sweep** before each commit — backend: `cargo test -p egret-bi` + `cargo fmt --all` ; frontend (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run`.

## Conventions

Metabase as the standard · clean-room · TDD · full sweep before each commit · configured git identity + `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` · stage explicit paths · docs bilingual (en canonical + vi reference) · squash-merge · branch `feat/dashboards-tier3a`.
