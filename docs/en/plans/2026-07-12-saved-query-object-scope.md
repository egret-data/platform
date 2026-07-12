# Saved-Query Object-Scope Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring SQL Lab saved queries to full parity with charts/dashboards/notebooks — folder-organized, object-grantable/shareable, role-based edit — governed by the Cedar PDP.

**Architecture:** Add a nullable `folder_id` to `analytics.saved_queries`; introduce `saved_query:read`/`saved_query:manage` catalog perms; teach the generic `shares.rs`/`kind_perms`/`has_object_grant_in_workspace` helpers a `saved_query` arm; migrate the five SQL Lab handlers from workspace-scope `query:run` + owner-gate to object-scope `require`/`require_object` (which also closes a latent cross-workspace IDOR); add folders + a Share modal to `SavedTab.tsx`.

**Tech Stack:** Rust (axum, sqlx, Cedar PDP), Postgres, React 18/TS/Vite/Tailwind v4/shadcn, orval, Vitest.

**Spec:** `docs/en/specs/2026-07-12-saved-query-object-scope-design.md`

**Reference (mirror these exactly):** `ch_get`/`ch_list`/`ch_create`/`ds_set_folder` in `crates/egret-bi/src/routes.rs`; notebook object-guest arm in `crates/egret-workspace/src/rbac.rs`; `Notebooks.tsx` folder+share UI.

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `migrations/0077_saved_query_objects.sql` | schema + role grants | **create** |
| `crates/egret-metastore/src/lib.rs` | force migrator re-embed | touch (whitespace) |
| `crates/egret-core/src/rbac.rs` | permission catalog | add 2 perms |
| `crates/egret-bi/src/saved.rs` | saved-query repo | `folder_id` + `set_folder` |
| `crates/egret-bi/src/model.rs` (+ its submodule defining `SavedQueryDto`/`SaveQuery`) | DTOs | `folder_id` field |
| `crates/egret-bi/src/shares.rs` | object-table map | `saved_query` arm |
| `crates/egret-workspace/src/rbac.rs` | object-guest admission | `saved_query` arm |
| `crates/egret-bi/src/routes.rs` | SQL Lab handlers + folder router | consts, `kind_perms`, 5 handlers, `set_folder` route |
| `crates/egret-bi/tests/saved_query_objects.rs` | backend integration tests | **create** |
| `frontend/src/api/folders.ts` | folder hook registry | `saved_queries` kind |
| `frontend/src/api/sqllab.ts` | saved-query hooks/types | `folder_id`, `useSetSavedQueryFolder` |
| `frontend/src/components/sharing/ShareDialog.tsx` | share kinds | `saved_query` in `ShareKind` |
| `frontend/src/components/sqllab/SavedTab.tsx` | saved-query list UI | folders + Share |
| `frontend/src/components/sqllab/__tests__/SavedTab.test.tsx` | FE test | folders + Share assertions |

---

## Task 1: Migration — `folder_id` column + role-permission grants

**Files:**
- Create: `migrations/0077_saved_query_objects.sql`
- Modify: `crates/egret-metastore/src/lib.rs` (touch to force re-embed)

- [ ] **Step 1: Write the migration**

Create `migrations/0077_saved_query_objects.sql`:

```sql
-- Saved-query object-scope (Effort B leftover).
-- 1. folder_id lets a saved query live in an analytics.folders row (entity_type='saved_query').
-- 2. Grant the new saved_query:{read,manage} perms to the seeded workspace roles
--    (mirrors migration 0070's grants for the BI object perms).

ALTER TABLE analytics.saved_queries
  ADD COLUMN folder_id uuid REFERENCES analytics.folders(id) ON DELETE SET NULL;

CREATE INDEX saved_queries_folder_idx ON analytics.saved_queries (folder_id);

INSERT INTO role_permissions (role_id, permission)
SELECT id, 'saved_query:read'
  FROM roles
 WHERE name IN ('ws_viewer', 'ws_editor', 'ws_admin')
ON CONFLICT DO NOTHING;

INSERT INTO role_permissions (role_id, permission)
SELECT id, 'saved_query:manage'
  FROM roles
 WHERE name IN ('ws_editor', 'ws_admin')
ON CONFLICT DO NOTHING;
```

- [ ] **Step 2: Force the embedded migrator to re-embed**

`serve`/tests use the embedded migrator; a new migration file alone doesn't recompile `egret-metastore`. Add a trailing blank line (or bump a comment) in `crates/egret-metastore/src/lib.rs` so the `sqlx::migrate!` macro re-runs at build. (Known gotcha — see memory `egret-sqlx-migration-staleness`.)

- [ ] **Step 3: Verify the migration applies against a fresh test DB**

Run: `cd /Users/1pro-l4r8/Documents/github/data-portal/egret && cargo test -p egret-bi --no-run`
Expected: compiles; the test harness (which runs migrations on its ephemeral DB) will exercise `0077` in Task 6. For a standalone check, if a local `egret_test` DB is available: `psql "$DATABASE_URL" -c "\d analytics.saved_queries"` shows the `folder_id` column.

- [ ] **Step 4: Commit**

```bash
git add migrations/0077_saved_query_objects.sql crates/egret-metastore/src/lib.rs
git commit -m "feat(db): add saved_queries.folder_id + grant saved_query perms to ws roles (mig 0077)"
```

---

## Task 2: Permission catalog — `saved_query:read` / `saved_query:manage`

**Files:**
- Modify: `crates/egret-core/src/rbac.rs` (the `PERMISSIONS` array, after the `notebook:*` entries at ~line 293)

- [ ] **Step 1: Write a failing test**

Add to the tests module of `crates/egret-core/src/rbac.rs` (find the existing `#[cfg(test)] mod`; if none references `is_known`, add this test):

```rust
#[test]
fn saved_query_perms_are_known() {
    assert!(is_known("saved_query:read"));
    assert!(is_known("saved_query:manage"));
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p egret-core saved_query_perms_are_known`
Expected: FAIL (`is_known` returns false — perms not in catalog).

- [ ] **Step 3: Add the two catalog entries**

In `crates/egret-core/src/rbac.rs`, immediately after the `notebook:write` `perm!(...)` entry (before the `workspace:manage_members` entry), insert:

```rust
    perm!(
        "saved_query:read",
        "SQL Lab",
        "View saved queries",
        Scope::Workspace
    ),
    perm!(
        "saved_query:manage",
        "SQL Lab",
        "Create, edit, and delete saved queries",
        Scope::Workspace
    ),
```

Do **not** add these to `ADMIN_GRAPH_ACTIONS` — they are grantable/object-scopable like `chart:*`.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p egret-core saved_query_perms_are_known`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-core/src/rbac.rs
git commit -m "feat(rbac): add saved_query:read + saved_query:manage catalog perms"
```

---

## Task 3: `SavedQueryRepo` — carry `folder_id`, add `set_folder`

**Files:**
- Modify: `crates/egret-bi/src/saved.rs`

- [ ] **Step 1: Add `folder_id` to the row struct + `COLS`**

In `crates/egret-bi/src/saved.rs`, change the `SavedQuery` struct and `COLS`:

```rust
#[derive(sqlx::FromRow)]
pub struct SavedQuery {
    pub id: Uuid,
    pub database_id: Option<Uuid>,
    pub name: String,
    pub sql: String,
    pub description: String,
    pub created_by: Option<Uuid>,
    pub folder_id: Option<Uuid>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

const COLS: &str = "id, database_id, name, sql, description, created_by, folder_id, updated_at";
```

`create` and `update` need no signature change: `create` omits `folder_id` (defaults NULL); `update` does not touch `folder_id` (so it is preserved). Both already `RETURNING {COLS}`, which now includes `folder_id`.

- [ ] **Step 2: Add `set_folder` to the repo**

Add this method to `impl SavedQueryRepo` (after `owner`):

```rust
    /// Move the saved query into (`Some`) or out of (`None`) a folder.
    pub async fn set_folder(&self, id: Uuid, folder_id: Option<Uuid>) -> Result<(), BiError> {
        sqlx::query("UPDATE analytics.saved_queries SET folder_id = $2 WHERE id = $1")
            .bind(id)
            .bind(folder_id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p egret-bi`
Expected: compiles (routes.rs `dto()` will error until Task 5 — if so, proceed; this task's changes are verified together with Task 5's build). To check this task in isolation, temporarily run `cargo check -p egret-bi 2>&1 | grep saved.rs` — expect no errors originating in `saved.rs`.

- [ ] **Step 4: Commit**

```bash
git add crates/egret-bi/src/saved.rs
git commit -m "feat(bi): SavedQueryRepo carries folder_id + set_folder method"
```

---

## Task 4: `shares.rs` object-table + object-guest admission arm

**Files:**
- Modify: `crates/egret-bi/src/shares.rs` (`object_table`, ~line 23)
- Modify: `crates/egret-workspace/src/rbac.rs` (`has_object_grant_in_workspace`, ~line 51)

- [ ] **Step 1: Add the `saved_query` arm to `object_table`**

In `crates/egret-bi/src/shares.rs`, add to the `object_table` match (after the `"notebook"` arm):

```rust
        "saved_query" => Some("saved_queries"),
```

`manage_action` needs no change — its default branch yields `"saved_query:manage"`. `object_ref`/`resolve` need no change — the generic `SELECT folder_id, workspace_id FROM analytics.saved_queries WHERE id = $1` works now that `folder_id` exists.

- [ ] **Step 2: Add the `saved_query` OR-clause to the admission arm**

In `crates/egret-workspace/src/rbac.rs`, inside `has_object_grant_in_workspace`'s SQL, add one line to the object-type disjunction (after the `notebook` line):

```sql
                    OR (ag.object_type = 'saved_query' AND ag.scope_id IN (SELECT id FROM analytics.saved_queries WHERE workspace_id = $2)) ) )
```

(Replace the existing trailing `) )` on the notebook line with `\` continuation as the surrounding lines do, then add the `saved_query` line ending in `) )`. Match the existing Rust string-literal `\`-continuation style exactly.)

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p egret-bi -p egret-workspace`
Expected: compiles (routes.rs errors from Task 5 are expected until Task 5 lands; `shares.rs` and `rbac.rs` themselves compile clean).

- [ ] **Step 4: Commit**

```bash
git add crates/egret-bi/src/shares.rs crates/egret-workspace/src/rbac.rs
git commit -m "feat(bi): shares object_table + object-guest admission learn saved_query"
```

---

## Task 5: Migrate the five SQL Lab handlers to object-scope + `set_folder` route + DTO

**Files:**
- Modify: `crates/egret-bi/src/routes.rs` (consts ~71, `dto` ~101, `sqllab_router` ~79, handlers `save`/`list`/`load`/`update`/`remove` ~133-273, `kind_perms` ~2614)
- Modify: `crates/egret-bi/src/model.rs` submodule defining `SavedQueryDto` and `SaveQuery`

**Note:** `crates/egret-bi/src/model.rs:22` re-exports `SavedQueryDto`, `SaveQuery`, `SetFolder`. Find their `struct` definitions (grep `pub struct SavedQueryDto` under `crates/egret-bi/src/`) and edit there. `SetFolder` already exists (used by `ds_set_folder`) with a `folder_id: Option<Uuid>` field — reuse it, do not redefine.

- [ ] **Step 1: Add the perm consts + `SavedQueryDto.folder_id`**

In `routes.rs`, after `const PERM: &str = "query:run";` add:

```rust
const SQ_READ: &str = "saved_query:read";
const SQ_MANAGE: &str = "saved_query:manage";
```

In the module defining `SavedQueryDto`, add a field (mirror `created_by`'s optional-string style):

```rust
    /// Folder this saved query lives in (`None` = root). Serialized as a string.
    pub folder_id: Option<String>,
```

- [ ] **Step 2: Update `dto()` to map `folder_id`**

In `routes.rs`, extend the `dto` mapper:

```rust
fn dto(q: crate::saved::SavedQuery) -> SavedQueryDto {
    SavedQueryDto {
        id: q.id.to_string(),
        database_id: q.database_id.map(|d| d.to_string()),
        name: q.name,
        sql: q.sql,
        description: q.description,
        created_by: q.created_by.map(|u| u.to_string()),
        folder_id: q.folder_id.map(|f| f.to_string()),
        updated_at: q.updated_at,
    }
}
```

- [ ] **Step 3: Rewrite the five handler gates**

`save` — change the gate line only:

```rust
    egret_core::authz::require(&*st.pdp, &ctx, SQ_MANAGE)
        .await
        .map_err(BiApiError::from)?;
```

`list` — change the gate line only:

```rust
    egret_core::authz::require(&*st.pdp, &ctx, SQ_READ)
        .await
        .map_err(BiApiError::from)?;
```

`load` — replace the `require(PERM)` prologue with the object-scope form (mirror `ch_get`):

```rust
    let obj = crate::shares::resolve(&st, &ctx, "saved_query", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, SQ_READ, obj)
        .await
        .map_err(BiApiError::from)?;
    let q = st
        .saved
        .get(id)
        .await
        .map_err(BiApiError::from)?
        .ok_or(Error::NotFound("saved query".into()))?;
    Ok(Json(dto(q)))
```

`update` — replace both `require(PERM)` **and** `require_owner(&st, &ctx, id).await?;` with:

```rust
    let obj = crate::shares::resolve(&st, &ctx, "saved_query", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, SQ_MANAGE, obj)
        .await
        .map_err(BiApiError::from)?;
```

`remove` (the delete handler) — same replacement as `update`: swap `require(PERM)` + `require_owner(...)` for the `resolve` + `require_object(SQ_MANAGE, obj)` pair above.

After this, `require_owner` (routes.rs ~275) and `SavedQueryRepo::owner` may be unused. If the compiler flags them dead, delete `require_owner` and `owner`; if other callers remain, leave them.

- [ ] **Step 4: Add the `set_folder` route + handler**

In `sqllab_router()`, add a route under the `/{id}` group:

```rust
        .route(
            "/ws/{workspace}/sqllab/queries/{id}/folder",
            put(sq_set_folder),
        )
```

Add the handler (mirror `ds_set_folder`, substituting `"saved_query"`, `SQ_MANAGE`, `st.saved`):

```rust
#[utoipa::path(
    put,
    path = "/api/ws/{workspace}/sqllab/queries/{id}/folder",
    tag = "sqllab",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Saved query id"),
    ),
    request_body = SetFolder,
    responses(
        (status = 200, description = "Saved query moved to folder", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing saved_query:manage permission", body = ErrorBody),
        (status = 404, description = "No such saved query or target folder in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn sq_set_folder(
    State(st): State<BiState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, Uuid)>,
    Json(b): Json<crate::model::SetFolder>,
) -> Result<Json<serde_json::Value>, BiApiError> {
    let obj = crate::shares::resolve(&st, &ctx, "saved_query", id).await?;
    egret_core::authz::require_object(&*st.pdp, &ctx, SQ_MANAGE, obj)
        .await
        .map_err(BiApiError::from)?;
    if let Some(fid) = b.folder_id {
        require_folder(&st, &ctx, fid, "saved_query").await?;
    }
    st.saved
        .set_folder(id, b.folder_id)
        .await
        .map_err(BiApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}
```

Register `sq_set_folder` in the sqllab OpenAPI `paths(...)` doc struct (find the `#[openapi(paths(save, list, load, update, remove, ...))]` for the sqllab tag and add `sq_set_folder`).

- [ ] **Step 5: Add the `saved_queries` arm to `kind_perms`**

In `routes.rs` `kind_perms` (~line 2614), add after the `"notebooks"` arm:

```rust
        "saved_queries" => Some(("saved_query", SQ_READ, SQ_MANAGE)),
```

- [ ] **Step 6: Verify it compiles + regen the OpenAPI-driven client later**

Run: `cargo build -p egret-bi`
Expected: compiles clean. (Any dead-code warning on `require_owner`/`owner` → delete them per Step 3.)

- [ ] **Step 7: Commit**

```bash
git add crates/egret-bi/src/routes.rs crates/egret-bi/src/model.rs
git commit -m "feat(bi): object-scope the 5 SQL Lab handlers + sq_set_folder + saved_query folder kind"
```

---

## Task 6: Backend integration tests

**Files:**
- Create: `crates/egret-bi/tests/saved_query_objects.rs`

Model the harness on the existing `crates/egret-bi/tests/share_deny.rs` (workspace + user + grant setup helpers). Read it first for the exact fixture helpers (DB pool, `make_ws`, `grant_object`, `WsContext` construction). Reuse those helpers rather than re-inventing them.

- [ ] **Step 1: Write the IDOR-regression test**

```rust
// A saved query in workspace B must NOT be loadable through workspace A's path.
#[tokio::test]
async fn load_rejects_cross_workspace_saved_query() {
    // Arrange: two workspaces; a saved query created in ws_b.
    // A user who is a ws_viewer of ws_a (has saved_query:read there).
    // Act: call `load` with ws_a on the path and the ws_b saved query id.
    // Assert: NotFound (404), not Ok.
}
```

- [ ] **Step 2: Write the object-guest read test**

```rust
// A non-member granted saved_query:read on ONE saved query can load it, but not a sibling.
#[tokio::test]
async fn object_guest_can_load_granted_saved_query_only() {
    // Arrange: ws with two saved queries q1,q2; an outside user granted ws_viewer on q1 (object grant).
    // Act+Assert: load(q1) -> Ok; load(q2) -> Forbidden/NotFound.
}
```

- [ ] **Step 3: Write the role-based-edit test (owner-gate removed)**

```rust
// A ws_editor who is NOT the creator can now update another user's saved query.
#[tokio::test]
async fn ws_editor_can_edit_others_saved_query() {
    // Arrange: q created_by user A; user B is ws_editor (has saved_query:manage).
    // Act: update(q) as B.
    // Assert: Ok, name changed.
}
```

- [ ] **Step 4: Write the set_folder cross-workspace guard test**

```rust
// set_folder must reject a folder from another workspace / wrong entity_type.
#[tokio::test]
async fn set_folder_rejects_foreign_folder() {
    // Arrange: q in ws_a; a folder created in ws_b (entity_type saved_query).
    // Act: sq_set_folder(ws_a, q, foreign_folder).
    // Assert: NotFound.
}
```

Fill each test body using the `share_deny.rs` helpers (create workspaces, users, roles, object grants; build `WsContext`; call the repo/PDP directly or via the handler fns as `share_deny.rs` does). Every test must assert the concrete status/outcome, not just `is_err()`.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p egret-bi --test saved_query_objects`
Expected: all 4 PASS. (Run once; redirect to a log if slow — do NOT run two workspace test suites concurrently, per memory `no-concurrent-test-suites-and-tail-buffering`.)

- [ ] **Step 6: Commit**

```bash
git add crates/egret-bi/tests/saved_query_objects.rs
git commit -m "test(bi): saved-query object-scope — IDOR fix, object-guest, role-edit, folder guard"
```

---

## Task 7: Regenerate the API client + wire FE folder/share plumbing

**Files:**
- Modify: `frontend/src/api/folders.ts`
- Modify: `frontend/src/api/sqllab.ts`
- Modify: `frontend/src/components/sharing/ShareDialog.tsx`

- [ ] **Step 1: Regenerate the OpenAPI + orval client**

The backend now exposes `PUT /sqllab/queries/{id}/folder` and a `folder_id` field. Regenerate:

Run: `cd /Users/1pro-l4r8/Documents/github/data-portal/egret && cargo run -p egret-bin -- openapi > frontend/openapi.json 2>/dev/null || true` then `cd frontend && npm run gen`
(Use the repo's actual spec-dump + gen commands — check `frontend/package.json` scripts for `gen`/`orval` and the spec source. Do not hand-edit files under `frontend/src/api/generated/`.)
Expected: `frontend/src/api/generated/sqllab/sqllab.ts` gains a `sqllabSqSetFolder` (or similarly-named) fetch fn; the saved-query DTO model gains `folder_id`.

Confirm the generated fn name:
Run: `grep -o "sqllab[A-Za-z]*SetFolder\|sqllab[A-Za-z]*Folder" frontend/src/api/generated/sqllab/sqllab.ts | head`
Use the exact name it prints in Step 3.

- [ ] **Step 2: Add `saved_query` to `ShareKind`**

In `frontend/src/components/sharing/ShareDialog.tsx` line 32:

```ts
export type ShareKind = "dashboard" | "chart" | "dataset" | "folder" | "notebook" | "saved_query";
```

- [ ] **Step 3: Register the `saved_queries` folder kind**

In `frontend/src/api/folders.ts`:

- Import the generated set-folder fn (use the exact name from Step 1, shown here as `sqllabSqSetFolder`):

```ts
import { sqllabSqSetFolder } from "./generated/sqllab/sqllab";
```

- Extend the `EntityKind` union:

```ts
export type EntityKind = "charts" | "dashboards" | "datasets" | "notebooks" | "saved_queries";
```

- Add to `listKeyOf` (saved queries use the hand-written key `["sqllab", ws, "saved"]`, matching `qk.saved` in `sqllab.ts`):

```ts
  saved_queries: (ws) => ["sqllab", ws, "saved"],
```

- Add to `setFolderOf`:

```ts
  saved_queries: (ws, id, folder_id) => sqllabSqSetFolder(ws, id, { folder_id }),
```

- [ ] **Step 4: Add `folder_id` to the `SavedQuery` type + a set-folder hook**

In `frontend/src/api/sqllab.ts`, extend the `SavedQuery` interface (line 37):

```ts
export interface SavedQuery { id: string; database_id: string | null; name: string; sql: string; description: string; created_by: string | null; folder_id: string | null; updated_at: string }
```

- [ ] **Step 5: Typecheck**

Run: `cd frontend && npm run typecheck`
Expected: passes (no references to a missing generated fn; `EntityKind`/`ShareKind` unions consistent).

- [ ] **Step 6: Commit**

```bash
git add frontend/openapi.json frontend/src/api/generated frontend/src/api/folders.ts frontend/src/api/sqllab.ts frontend/src/components/sharing/ShareDialog.tsx
git commit -m "feat(fe): regen client + wire saved_queries folder kind and share kind"
```

---

## Task 8: `SavedTab.tsx` — folders + Share modal

**Files:**
- Modify: `frontend/src/components/sqllab/SavedTab.tsx`
- Create/Modify: `frontend/src/components/sqllab/__tests__/SavedTab.test.tsx`

Mirror `frontend/src/routes/explore/Notebooks.tsx` for the folder grouping (`useFolders(ws, "saved_queries")`, `useSetFolder(ws, "saved_queries")`, `useFolderMutations`, `groupByFolder`, `useCollapsed`, `FolderHeaderRow`, `MoveToFolderItems`, `RowActionsMenu`, `FolderNameDialog`) and `ShareDialog`. `SavedTab` is a **narrow sidebar** panel (not a full-page table), so keep it list-based: render collapsible `FolderHeaderRow`-style group headers with the existing row layout nested underneath, rather than porting the full `<table>`.

- [ ] **Step 1: Write the failing FE test**

In `frontend/src/components/sqllab/__tests__/SavedTab.test.tsx` (read the existing `SavedTab.test.tsx` if present and extend it):

```tsx
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { SavedTab } from "@/components/sqllab/SavedTab";
// ...wrap in the project's QueryClientProvider test harness (copy from SavedTab.test.tsx / Notebooks.test.tsx)

it("groups saved queries under folder headers and offers a Share action", async () => {
  // Arrange: mock useSavedQueries -> [{..folder_id: 'f1'..}], useFolders -> [{id:'f1', name:'Reports'}]
  // Act: render SavedTab
  // Assert: folder header 'Reports' is shown; opening a row's action menu reveals 'Share…'
  expect(await screen.findByText("Reports")).toBeInTheDocument();
  // open the row menu, then:
  expect(await screen.findByText("Share…")).toBeInTheDocument();
});
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cd frontend && npm run test -- SavedTab`
Expected: FAIL (no folder header / no Share action yet).

- [ ] **Step 3: Implement folders + Share in `SavedTab.tsx`**

Adapt `SavedTab.tsx`:
- Add hooks: `const { data: folders = [] } = useFolders(ws, "saved_queries");` `const setFolder = useSetFolder(ws, "saved_queries");` `const { create, rename, remove } = useFolderMutations(ws, "saved_queries");` `const collapse = useCollapsed(\`folders:${ws}:saved_queries\`);`
- Compute `const groups = useMemo(() => groupByFolder(items, folders, /*filtering*/ false), [items, folders]);` and render each group with a collapsible header (mirror `FolderHeaderRow`'s toggle/rename/delete/onShare/onDropItem wiring from `Notebooks.tsx`) followed by that group's rows (keep the existing row markup from the current `SavedTab`).
- Add a per-row actions menu (`RowActionsMenu` + `MoveToFolderItems` + a `Share…` `DropdownMenuItem`) alongside the existing Edit/Delete buttons, wiring `onShare` to open `<ShareDialog ws={ws} kind="saved_query" id={q.id} name={q.name} folderId={q.folder_id} ... />` and `MoveToFolderItems` to `setFolder.mutateAsync({ id, folder_id })`.
- Add `shareTarget`/`folderShare`/`folderDialog` state exactly as `Notebooks.tsx` does; render `ShareDialog` (kind `saved_query`) and a folder `ShareDialog` (kind `folder`) + `FolderNameDialog`.
- Keep the existing empty-state (`No saved queries.`) for the zero-items case.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd frontend && npm run test -- SavedTab`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/sqllab/SavedTab.tsx frontend/src/components/sqllab/__tests__/SavedTab.test.tsx
git commit -m "feat(fe): SavedTab gains folders + Metabase share modal (saved_query parity)"
```

---

## Task 9: Full local CI + adversarial security review

**Files:** none (verification)

- [ ] **Step 1: Backend sweep**

Run (once, sequentially — never two workspace test suites at once):
```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace 2>&1 | tail -40
```
Expected: fmt clean, clippy 0 warnings, all tests `ok`.

- [ ] **Step 2: Frontend sweep**

```bash
cd frontend && npm run lint && npm run typecheck && npm run test && npm run build
```
Expected: all green.

- [ ] **Step 3: Adversarial security-review subagent**

Dispatch a security-review subagent over the branch diff. Focus: (a) the cross-workspace IDOR is actually closed on `load`/`update`/`remove`/`sq_set_folder` (every path goes through `resolve`, which asserts `obj.workspace_id == ctx.workspace_id`); (b) object-guest admission arm doesn't over-admit (scoped to saved queries in the same workspace, `effect='allow'` only); (c) `require_folder` blocks foreign / wrong-`entity_type` folders; (d) `kind`/`entity_type` never reach SQL as interpolated identifiers (they don't — `object_table` is a hard-coded map, `kind_perms` maps to a `'static` literal). Fix any HIGH/MEDIUM findings in-branch and re-run the relevant tests.

- [ ] **Step 4: Hand off to finishing-a-development-branch**

Use `superpowers:finishing-a-development-branch` → "Push and create a Pull Request" (the user's standard per-increment flow: push, `gh pr create`, `gh pr merge --squash --admin`, sync main, delete branch, update memory).

---

## Self-Review

**Spec coverage:**
- §1 migration → Task 1 ✓ (folder_id + index + role grants)
- §2 perm catalog → Task 2 ✓
- §3 shares arm + object-guest admission → Task 4 ✓
- §4 five handlers + set_folder + DTO folder_id → Task 5 ✓; owner-gate removal → Task 5 Step 3 ✓
- §5 share API + folder router → Task 5 Step 5 (`kind_perms`) ✓ (share routes already generic)
- §6 FE folders + Share → Tasks 7-8 ✓
- Testing §: IDOR/object-guest/role-edit/folder-guard → Task 6 ✓; FE folders+share → Task 8 ✓; security review → Task 9 ✓

**Placeholder scan:** Task 6 test bodies are described (arrange/act/assert) rather than fully coded because they depend on `share_deny.rs`'s fixture helpers, which the implementer must read first; each step names the exact scenario + assertion. Task 8 mirrors `Notebooks.tsx` (concrete reference) adapted to a sidebar. No `TBD`/`TODO`.

**Type consistency:** `folder_id` is `Option<Uuid>` (repo), `Option<String>` (DTO/`SavedQueryDto`), `string | null` (FE). `SQ_READ`/`SQ_MANAGE` used consistently in handlers + `kind_perms`. `EntityKind`/`ShareKind` both gain `saved_queries`/`saved_query` respectively (note the plural-vs-singular split is intentional and matches the existing charts=`"charts"` folder-kind / `"chart"` share-kind convention). `set_folder` (repo) / `sq_set_folder` (handler) / `sqllabSqSetFolder` (generated) names are distinct-by-layer and consistent within each layer.
