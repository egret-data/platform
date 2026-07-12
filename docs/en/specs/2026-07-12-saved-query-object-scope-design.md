# Saved-Query Object-Scope — Design

**Status:** Approved
**Date:** 2026-07-12
**Effort:** B (unified authorization) — post-B4 leftover
**Mirrors:** B2-2 (notebook object-scope), B2-1a (BI object-scope foundation)

## Goal

Bring SQL Lab saved queries to full parity with charts / dashboards / notebooks:
make them **object-grantable, shareable, and folder-organized** — governed by the
Cedar PDP like every other BI object — instead of the current flat, workspace-wide,
owner-gated model.

## Motivation / current state

`analytics.saved_queries` today:

- Columns: `id, workspace_id, database_id, name, sql, description, created_by, created_at, updated_at`.
  **No `folder_id`** (charts/datasets/notebooks all have one).
- All five handlers (`save`/`list`/`load`/`update`/`delete`) gate on
  `require(query:run)` at **workspace scope**; `update`/`delete` add `require_owner`.
  Consequence: *every* ws member with `query:run` sees **all** saved queries in the
  workspace; none can be shared to an object-guest; only the owner can edit.
- **Latent cross-workspace IDOR:** `load`/`update`/`delete` fetch `WHERE id = $1`
  with no `workspace_id` predicate, so a member of workspace A with `query:run` can
  load a saved query belonging to workspace B by id. Object-scoping via `resolve`
  closes this as a side effect.

`folders` is already per-type via a `entity_type` text column
(`'chart' | 'dashboard' | 'dataset' | 'notebook'`); adding `'saved_query'` needs no
schema change. `shares.rs` (`object_table` / `manage_action` / `object_ref` / share
API `/objects/{kind}/{id}/grants`) is already generic over `kind`; it needs one new
arm.

## Design

### 1. Data model — migration `0077_saved_query_objects.sql`

```sql
ALTER TABLE analytics.saved_queries
  ADD COLUMN folder_id uuid REFERENCES analytics.folders(id) ON DELETE SET NULL;
CREATE INDEX saved_queries_folder_idx ON analytics.saved_queries (folder_id);

-- Grant the two new permissions to the seeded workspace roles (mirrors mig 0070).
-- ws_viewer -> saved_query:read
-- ws_editor, ws_admin -> saved_query:read + saved_query:manage
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'saved_query:read'   FROM roles WHERE name IN ('ws_viewer','ws_editor','ws_admin')
ON CONFLICT DO NOTHING;
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'saved_query:manage' FROM roles WHERE name IN ('ws_editor','ws_admin')
ON CONFLICT DO NOTHING;
```

- `folders.entity_type` gains the value `'saved_query'` at runtime (no DDL — it is a
  free text column; the folder CRUD is already generic per `entity_type`).
- `folder_id` is nullable: existing saved queries land in the implicit root folder,
  exactly like charts pre-B2.

### 2. Permission catalog — `crates/egret-core/src/rbac.rs`

Add two grantable, object-scopable permissions next to the `chart:*` / `notebook:*`
entries:

- `saved_query:read`
- `saved_query:manage`

These are **not** added to `ADMIN_GRAPH_ACTIONS` — they behave like `chart:*`
(grantable to any role, object-scopable), not like admin-only graph actions.

### 3. Authz wiring — `crates/egret-bi/src/shares.rs`

- `object_table`: add `"saved_query" => Some("saved_queries")`. (The helper returns
  the **bare** table name; `object_ref` prefixes `analytics.`, so no schema prefix
  here.)
- `manage_action`: no change needed — the default branch already yields
  `format!("{kind}:manage")` = `"saved_query:manage"`.
- `object_ref`: no change needed — the generic
  `SELECT folder_id, workspace_id FROM analytics.{table} WHERE id = $1` works once the
  column exists.
- **Object-guest admission** (`has_object_grant_in_workspace`, the WsContext
  admission arm): add a `"saved_query"` branch mirroring the `"notebook"` branch, so a
  principal holding only a saved_query object grant is admitted to the workspace with
  object scope.

### 4. Handlers — `crates/egret-bi/src/routes.rs` (migrate all five)

| Handler  | From                              | To                                                        |
|----------|-----------------------------------|-----------------------------------------------------------|
| `save`   | `require(query:run)`              | `require(saved_query:manage)` (ws-level create)           |
| `list`   | `require(query:run)`              | `require(saved_query:read)`, then filter to grant-visible rows (owned ∪ folder-granted ∪ object-granted), mirroring chart `list` |
| `load`   | `require(query:run)`              | `require_object(saved_query:read, resolve("saved_query", id))` |
| `update` | `require(query:run)` + `require_owner` | `require_object(saved_query:manage, resolve(...))` — **owner-gate dropped**, role/grant-based edit (matches B2-1a) |
| `delete` | `require(query:run)` + `require_owner` | `require_object(saved_query:manage, resolve(...))`        |

Plus:

- New `set_folder` handler for saved_query (mirror chart `set_folder`): `PUT
  /ws/{ws}/sqllab/queries/{id}/folder`, body `{ folder_id: Option<Uuid> }`, gated
  `require_object(saved_query:manage, ...)` + `require_folder(...)` when assigning a
  folder (verifies the folder belongs to the same workspace and `entity_type =
  'saved_query'`).
- `SavedQueryDto` / `SavedQuery` / `SavedQueryRepo` gain `folder_id: Option<Uuid>`
  (repo `COLS`, `create`, `update`, `list`, `get`, and the DTO mapper `dto()`).
- Drop the now-unused `require_owner` helper for saved queries (keep it if other
  callers remain; otherwise remove).

The `list` filter follows the chart-list precedent exactly: a caller with a
workspace role that grants `saved_query:read` sees all workspace saved queries; an
object-guest sees only the saved queries granted to them.

### 5. Share API + folders — already generic (no new routes)

- `PUT/DELETE/GET /ws/{ws}/objects/saved_query/{id}/grants` starts working the moment
  `object_table` accepts the kind.
- `GET/POST/DELETE /ws/{ws}/folders?entity_type=saved_query` already works.

### 6. Frontend — `frontend/src/components/sqllab/SavedTab.tsx` + share/folder plumbing

- **Folders:** reuse `FolderHeaderRow` + folder-tree grouping (copy the
  Charts/Notebooks list pattern), `useFolders('saved_query')`, and a move-to-folder
  row action calling the new `set_folder` endpoint.
- **Share:** add a `Share…` row action opening the existing `ShareDialog` with
  `kind='saved_query'`; extend the `ShareKind` union and the `objectKindLabel` map
  with `saved_query`.
- orval regen picks up the new perms and the `folder_id` DTO field automatically.

## Testing

**Backend**

- `shares` unit: `object_table`/`object_ref`/`manage_action` resolve `saved_query`.
- Handler tests:
  - Object-guest holding a saved_query grant can `load` (and run) the granted query
    but gets 403/404 on a non-granted one.
  - **IDOR regression:** loading a saved query that belongs to another workspace now
    returns `NotFound` (was previously readable).
  - `update`/`delete` by a `ws_editor` who is *not* the owner now succeeds
    (owner-gate removed → role/grant-based).
  - `list` returns only grant-visible rows for an object-guest; all workspace rows
    for a ws-role holder.
  - `set_folder` rejects a folder from another workspace / wrong `entity_type`.

**Frontend (Vitest)**

- `SavedTab` renders folder headers and groups queries under them.
- `SavedTab` exposes a `Share…` action that opens `ShareDialog` with the
  `saved_query` kind.

**Security review**

- Adversarial security-review subagent before merge (standard per-increment gate):
  focus on the IDOR fix, object-guest admission, folder cross-workspace guard, and
  that no saved-query SQL path splices `kind` as an identifier.

## Out of scope

- Per-user data-plane delegation for saved-query *execution* — already covered by B3a
  (SQL Lab `run_query` already mints per-user engine JWTs); this epic governs the
  saved-query *object*, not the run path, which is unchanged.
- Saved-query versioning / history.
- Cross-workspace saved-query sharing (object grants stay within one workspace, like
  every other BI object).

## Migration / rollout notes

- Migration `0077` is additive and backfill-free (`folder_id` nullable, existing
  rows → root folder; new role_permissions rows are idempotent via
  `ON CONFLICT DO NOTHING`).
- Behavior change for existing users: saved-query edit is no longer owner-restricted —
  any ws_editor/ws_admin can edit, matching charts/dashboards/notebooks. This is the
  intended parity change (documented here so it is not a surprise in review).
- Remember to `touch crates/egret-metastore/src/lib.rs` so the embedded migrator
  re-embeds the new migration (known sqlx staleness gotcha).
