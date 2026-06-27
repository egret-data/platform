# DB connections → per-workspace — Design

**Date:** 2026-06-27 · **Status:** Design — approved · **Branch:** `feat/db-per-workspace` (off main)
**Program:** Epic A of the "DB-per-workspace + Secrets" program. Epic B (Secrets module) follows and depends on this.

## Goal
Replace the global **register-then-grant** database model (ADR-12) with **per-workspace self-service** DB connections (ADR-19), mirroring the Storage registry. Each workspace owns its DB connections, managed by a workspace admin in the **MANAGE** zone. This epic does **not** touch the secret model — DB connections keep their inline `secret_id → secrets` sealing; named-secret reuse is Epic B.

## Background (current state)
- `databases` is a **global** table (no `workspace_id`); platform admins register them (`POST /api/admin/databases`, gated platform `database:manage`) and grant them to workspaces via `workspace_database_grant`. Workspaces see granted DBs via `me_database_router` (`GET /api/me/databases?workspace=<slug>`).
- Query path: `egret-engine/routes.rs::authorize()` checks `DatabaseRepo::list_for_workspace(ws)` (grant join) before running; `egret-engine/job.rs` resolves the connection via `databases.get(id)` + `open_secret(id)`. `queries.database_id` FK → `databases.id`.
- Data snapshot (dev DB): **587 `databases` rows**, but only **2 are granted** to a workspace (`starrocks-analytics`, `trino-local` — each to exactly **1** workspace, no fan-out). The other ~585 are `db-<uuid>` **test fixtures** (0 grants, unreachable). 7128 `queries` rows reference databases (history).

## 1. Data model
Add `workspace_id` to `databases` (do **not** create a new table — keeping `databases.id` stable means `queries.database_id` FK and every engine/bi resolve-by-id path keep working unchanged):

```sql
ALTER TABLE databases
  ADD COLUMN workspace_id uuid REFERENCES workspaces(id) ON DELETE CASCADE;
CREATE INDEX databases_workspace_id_idx ON databases(workspace_id);
```

- **Backfill** from grants (each granted DB → its single workspace; the data shows no fan-out, so this is 1:1, no row-splitting):
  ```sql
  UPDATE databases d SET workspace_id = g.workspace_id
  FROM workspace_database_grant g WHERE g.database_id = d.id;
  ```
- **Orphans** (rows still `workspace_id IS NULL` after backfill — the ~585 ungranted dev test fixtures): left **NULL = legacy/unreachable**, filtered out of every workspace query (**non-destructive**, approved). `workspace_id` stays **NULLable**; new rows always set it. An optional cleanup (`DELETE FROM databases WHERE workspace_id IS NULL`, cascading their `queries`) is **not** run by the migration — documented for the user to run manually if desired.
- **Drop** `workspace_database_grant` and the grant code/routes (clean cut, approved).

## 2. RBAC
The new workspace routes are gated by **`workspace:manage_members`** (the same gate Storage uses via `require_workspace_admin`). The platform `database:manage`/`database:read` permissions stay defined but no longer gate DB-connection management. A workspace admin manages only their own workspace's DB connections.

## 3. Backend — `egret-database`
- **Repo** becomes workspace-scoped. `DatabaseRepo`:
  - `create(workspace_id, input)` — sets `workspace_id` from the route path.
  - `list_for_workspace(workspace_id)` — `WHERE workspace_id = $1` (replaces the grant join).
  - `get(workspace_id, id)` / `update(workspace_id, id, …)` / `delete(workspace_id, id)` / `open_secret(workspace_id, id)` / `set_status(workspace_id, id, …)` — all verify the row belongs to the workspace (cross-workspace id → `NotFound`, IDOR closed).
  - `grants.rs` is **removed**.
- **New workspace router** `/ws/{ws}/databases` (mirrors the storages routes; `WsContext` + `require_workspace_admin`):
  - `GET    /ws/{ws}/databases` — list this workspace's connections.
  - `POST   /ws/{ws}/databases` — create (workspace_id from path).
  - `POST   /ws/{ws}/databases/test` — test an unsaved connection (reachability).
  - `GET    /ws/{ws}/databases/{id}` — detail.
  - `PUT    /ws/{ws}/databases/{id}` — update (secret write-only: blank ⇒ keep existing).
  - `DELETE /ws/{ws}/databases/{id}` — delete.
  - `POST   /ws/{ws}/databases/{id}/test` — test a saved connection; persists `status` (ok/error).
- **Remove** `admin_database_router` and `me_database_router` and their mounts in `egret-server/src/lib.rs`. The workspace router is mounted where the other `/ws/{ws}/…` routers live (alongside catalog/storage).
- **`egret-engine`**: `authorize()` switches from the grant-join to **ownership** — the requested `database_id` must resolve to a row whose `workspace_id == ctx.workspace_id` (else 403/404). `job.rs` `get`/`open_secret` calls become workspace-scoped (pass `ctx.workspace_id`).

## 4. Frontend
- **DatabasesPage** (`src/routes/explore/DatabasesPage.tsx`), a new MANAGE-zone page mirroring `StoragePage`:
  - Registry table: **Name · Engine · Host:Port · Status · Secret · Actions**.
  - Add/Edit dialog: name, **engine** `Select` (trino / starrocks), host, port, options (user, catalog/schema, http_scheme/ssl as the current form has), **password** (write-only). A **Test connection** action (calls the test endpoint, surfaces ok/error). Delete via `ConfirmDeleteDialog`. 403 → workspace-admin notice. Secret never rendered.
- **Sidebar**: add **Databases** under the **MANAGE** group (after Storage) in `Sidebar.tsx`.
- **`src/api/databases.ts`** hooks (mirror `storage.ts`): `useDatabases(ws)`, `useCreateDatabase`, `useUpdateDatabase`, `useDeleteDatabase`, `useTestDatabase` (+ test-unsaved). Paths `/ws/${ws}/databases` (no `/api` prefix — `apiFetch` prepends it).
- **SQL Lab database picker** (and any chart/dataset DB selector): switch its source from `/api/me/databases?workspace=` to the workspace endpoint `GET /ws/{ws}/databases`. The old `admin/Databases.tsx` placeholder is removed from the `/admin` area.

## 5. Migration
A new SQL migration (next number after the current head): the `ALTER TABLE` + index + backfill `UPDATE` + `DROP TABLE workspace_database_grant`. **Touch `egret-metastore/src/lib.rs`** so the embedded migrator re-embeds the new file (per the known sqlx-migration-staleness gotcha). Orphan rows are left NULL (non-destructive).

## 6. Testing
- **Backend (DB + Trino/StarRocks):**
  - workspace-scoped CRUD: create sets `workspace_id`; list returns only this workspace's rows; a connection id from another workspace → 404 (IDOR closed); non-admin member → 403.
  - test-connection: ok on a reachable engine, error status on an unreachable one; `status` persisted.
  - query authorize: a `database_id` owned by another workspace cannot be queried (engine `authorize` rejects); an owned one runs.
  - migration: a granted DB receives its `workspace_id`; an ungranted DB stays NULL and is absent from `list_for_workspace`.
- **Frontend (Vitest, mock `@/api/databases`):** DatabasesPage list / add / edit / delete / test; 403 notice; engine select; secret write-only (no plaintext rendered).
- **Manual:** the real-Trino integration test (per the `egret-manual-trino-integration-test` note) before merge; full local CI (fmt + clippy + workspace test + FE lint/typecheck/test/build) before any admin squash-merge.

## 7. Risks
- **Query path regression** — `authorize()` + the SQL Lab picker must switch to ownership cleanly or query execution breaks. Covered by the engine authorize test + manual SQL Lab check.
- **Consumers of databases** — SQL Lab, charts, datasets resolve a DB by id through the workspace list; verify each reads the workspace endpoint.
- **Orphan test DBs** — non-destructive (NULL + filtered); the optional destructive cleanup is documented, not automatic.
- **CI** — GitHub Actions is out of quota; run full local CI + the manual Trino test before admin-merge (per project rule).

## 8. Out of scope (Epic B and later)
- Named-secret registry + rewiring the DB (and storage) connection forms to reference a named secret + backfilling inline secrets — **Epic B**.
- Cross-workspace DB sharing (the old grant model is removed; if shared DBs are ever needed again, that's a separate feature).
- Ranger secret rewiring; real GCS/Azure storage backends.
