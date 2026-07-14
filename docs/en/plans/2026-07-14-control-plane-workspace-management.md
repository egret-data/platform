# Control-Plane Workspace Management Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the control-plane `Workspaces` placeholder with a real list page plus a per-workspace admin cockpit (Overview · Members · Resources · Access requests · Activity), backed by four small additions to the existing workspace-admin API.

**Architecture:** The backend lifecycle API (`admin_workspace_router` in `egret-workspace`, state `WorkspaceState`, mounted at `/api/admin`) already does create/list/get/rename/archive. We add: a **restore** route, **owner+created** fields on `WorkspaceDto`, a **usage-stats** endpoint (member + query counts), and an **ownership-transfer** route. The frontend regenerates its orval client, extracts a reusable `MembersPanel` from the existing workspace-side `Members.tsx`, and adds two pages under `RequireGlobalAdmin`.

**Tech Stack:** Rust (axum, sqlx, utoipa), Postgres; React 18 + TypeScript, TanStack Query, orval-generated client, shadcn/ui, Vitest.

---

## Deviations from the design spec (architecture drift found during planning)

The spec (`docs/en/specs/2026-07-14-control-plane-workspace-management-design.md`) was written against a 14-day-old snapshot. Verification against current code found the resource model has moved:

1. **Storage is now GLOBAL** (`storage.storage_connections`, migration 0050+; `workspace_storages` no longer referenced in `src/`). Per-workspace `storage_conn_count` and `storage_bytes` are therefore **not coherent metrics** and are **dropped** from the usage-stats endpoint. `storage_bytes` also required an object-store dependency that `WorkspaceState` does not have.
2. **Warehouse/engine admin relocated** to `egret-catalog-engine` / `egret-query` with new tables (0056–0058). Rather than hand-write COUNT SQL against tables that may move again, the **usage-stats endpoint returns only the two metrics `egret-workspace` owns from stable tables**: `member_count` (via `MemberRepo::list`) and `query_count` (`queries.workspace_id`, stable since migration 0009).
3. **Resources tab** ships as **navigation cards deep-linking** into the existing CP resource pages (Warehouses, Storage, Engines, Secrets), without live per-workspace counts. Live counts are a follow-up once the current per-workspace resource list endpoints are re-confirmed.
4. **Restore** is implemented as a **dedicated `POST …/restore` route** (mirrors the `DELETE …` archive) instead of overloading `PUT`, keeping the existing rename contract untouched.

These are scope refinements, not feature removals: "usage stats" still ships (members + queries run); storage GB is dropped because storage is no longer per-workspace.

---

## File structure

**Backend — `crates/egret-workspace/`**
- `src/workspaces.rs` — add `set_status`, `set_owner`, `count_queries`; extend `WorkspaceRow` + queries with owner/created columns.
- `src/model.rs` — extend `Workspace` struct with `created_at`, `created_by`, `owner_name`.
- `src/routes.rs` — extend `WorkspaceDto`; add `restore_ws`, `workspace_stats`, `transfer_owner` handlers + `WorkspaceStatsDto`/`TransferOwner` DTOs; register routes + OpenAPI paths.
- `tests/workspace_db.rs` — route/repo tests for the four additions.

**Frontend — `frontend/`**
- `../openapi.json` — regenerated (repo root).
- `src/api/generated/admin-workspaces/admin-workspaces.ts` — regenerated (new hooks).
- `src/components/access/MembersPanel.tsx` — **new**, extracted from `Members.tsx`.
- `src/routes/explore/settings/Members.tsx` — becomes a thin wrapper around `MembersPanel`.
- `src/routes/control-plane/Workspaces.tsx` — **new** list page.
- `src/routes/control-plane/WorkspaceDetail.tsx` — **new** tabbed detail page.
- `src/routes/control-plane/__tests__/Workspaces.test.tsx`, `WorkspaceDetail.test.tsx` — **new**.
- `src/App.tsx` — replace placeholder, add `workspaces/:id` route.

---

## Task 1: Backend — Restore (un-archive)

**Files:**
- Modify: `crates/egret-workspace/src/workspaces.rs`
- Modify: `crates/egret-workspace/src/routes.rs`
- Test: `crates/egret-workspace/tests/workspace_db.rs`

- [ ] **Step 1: Write the failing test**

Add to `crates/egret-workspace/tests/workspace_db.rs` (follow the existing harness in that file — it builds a `Router` with `admin_workspace_router()` at `/api/admin` and a seeded platform-admin session):

```rust
#[sqlx::test]
async fn restore_reactivates_an_archived_workspace(pool: PgPool) {
    let (app, admin) = admin_app(pool.clone()).await; // existing helper in this test module
    let ws = create_ws_via_api(&app, &admin, "restore-me", "Restore Me").await;

    // archive it
    let res = req(&app, &admin, "DELETE", &format!("/api/admin/workspaces/{ws}"), None).await;
    assert_eq!(res.status(), StatusCode::OK);

    // restore it
    let res = req(&app, &admin, "POST", &format!("/api/admin/workspaces/{ws}/restore"), None).await;
    assert_eq!(res.status(), StatusCode::OK);

    // get: status is active again
    let body = json_body(req(&app, &admin, "GET", &format!("/api/admin/workspaces/{ws}"), None).await).await;
    assert_eq!(body["status"], "active");
}
```

> If `admin_app`/`create_ws_via_api`/`req`/`json_body` helpers don't exist yet under these names, mirror the request-building pattern already used by the workspace tests in this file; reuse whatever helper the existing `list_ws`/`archive_ws` tests use.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p egret-workspace restore_reactivates -- --nocapture`
Expected: FAIL — route `POST /api/admin/workspaces/{id}/restore` returns 404 (not registered).

- [ ] **Step 3: Add the repo method**

In `crates/egret-workspace/src/workspaces.rs`, after `archive` (line ~107):

```rust
    pub async fn set_status(&self, id: WorkspaceId, status: &str) -> Result<()> {
        sqlx::query("UPDATE workspaces SET status = $1 WHERE id = $2")
            .bind(status)
            .bind(id.0)
            .execute(&self.db.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }
```

- [ ] **Step 4: Add the handler + route**

In `crates/egret-workspace/src/routes.rs`, add the route to `admin_workspace_router` (line ~88):

```rust
pub fn admin_workspace_router() -> Router<WorkspaceState> {
    Router::new()
        .route("/workspaces", post(create_ws).get(list_ws))
        .route(
            "/workspaces/{id}",
            get(get_ws).put(update_ws).delete(archive_ws),
        )
        .route("/workspaces/{id}/restore", post(restore_ws))
}
```

Add the handler near `archive_ws` (line ~232):

```rust
#[utoipa::path(
    post,
    path = "/api/admin/workspaces/{id}/restore",
    tag = "admin-workspaces",
    params(("id" = Uuid, Path, description = "Workspace id")),
    responses(
        (status = 200, description = "Workspace restored", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing platform permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn restore_ws(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, AuthError> {
    egret_core::authz::require(&*st.pdp, &p, "workspace:archive")
        .await
        .map_err(AuthError)?;
    st.workspaces
        .set_status(WorkspaceId(id), "active")
        .await
        .map_err(AuthError)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}
```

Register the path in the `AdminWorkspaceApi` OpenApi derive (line ~1182): add `restore_ws` to `paths(...)`:

```rust
    paths(create_ws, list_ws, get_ws, update_ws, archive_ws, restore_ws),
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p egret-workspace restore_reactivates -- --nocapture`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/egret-workspace/src/workspaces.rs crates/egret-workspace/src/routes.rs crates/egret-workspace/tests/workspace_db.rs
git commit -m "feat(workspace): restore (un-archive) endpoint for control-plane admin"
```

---

## Task 2: Backend — Owner + created fields on WorkspaceDto

**Files:**
- Modify: `crates/egret-workspace/src/model.rs`
- Modify: `crates/egret-workspace/src/workspaces.rs`
- Modify: `crates/egret-workspace/src/routes.rs`
- Test: `crates/egret-workspace/tests/workspace_db.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[sqlx::test]
async fn workspace_dto_exposes_owner_and_created(pool: PgPool) {
    let (app, admin) = admin_app(pool.clone()).await;
    let ws = create_ws_via_api(&app, &admin, "owned", "Owned WS").await;

    let body = json_body(req(&app, &admin, "GET", &format!("/api/admin/workspaces/{ws}"), None).await).await;
    assert!(body["created_at"].is_string(), "created_at present");
    // the admin who created it is the owner; name defaults may be empty -> falls back to email
    assert!(body["owner_name"].is_string(), "owner_name present");
    assert!(body.get("created_by").is_some(), "created_by present");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p egret-workspace workspace_dto_exposes_owner -- --nocapture`
Expected: FAIL — `created_at`/`owner_name` are null/absent.

- [ ] **Step 3: Extend the domain model**

In `crates/egret-workspace/src/model.rs`, replace the `Workspace` struct (lines 7–13):

```rust
#[derive(Debug, Clone, Serialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub slug: String,
    pub name: String,
    pub status: String, // active | archived
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub created_by: Option<Uuid>,
    pub owner_name: Option<String>,
}
```

- [ ] **Step 4: Extend the row + queries**

In `crates/egret-workspace/src/workspaces.rs`, update `WorkspaceRow` (lines 141–147) and its `From` impl (lines 149–158):

```rust
#[derive(sqlx::FromRow)]
struct WorkspaceRow {
    id: Uuid,
    slug: String,
    name: String,
    status: String,
    created_at: chrono::DateTime<chrono::Utc>,
    created_by: Option<Uuid>,
    owner_name: Option<String>,
}

impl From<WorkspaceRow> for Workspace {
    fn from(r: WorkspaceRow) -> Self {
        Workspace {
            id: WorkspaceId(r.id),
            slug: r.slug,
            name: r.name,
            status: r.status,
            created_at: r.created_at,
            created_by: r.created_by,
            owner_name: r.owner_name,
        }
    }
}
```

Update the SELECT in `list`, `get_by_id`, `get_by_slug`, `list_for_principal`, and the `create` RETURNING to select the new columns via a shared column list. For each `SELECT ... FROM workspaces` add the join + columns. Example for `get_by_id` (lines 79–88):

```rust
    pub async fn get_by_id(&self, id: WorkspaceId) -> Result<Option<Workspace>> {
        let row = sqlx::query_as::<_, WorkspaceRow>(
            "SELECT w.id, w.slug, w.name, w.status, w.created_at, w.created_by, \
                    NULLIF(u.name, '') AS owner_name \
             FROM workspaces w LEFT JOIN users u ON u.id = w.created_by \
             WHERE w.id = $1",
        )
        .bind(id.0)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(row.map(Into::into))
    }
```

> `owner_name` uses `NULLIF(u.name,'')` because `users.name` defaults to `''`; the FE falls back to email/id when it is null. Apply the same `w.`-qualified column list + `LEFT JOIN users u` to `list`, `get_by_slug`, and `list_for_principal` (keep their existing `WHERE`/`ORDER BY`/`DISTINCT` clauses, qualifying columns with `w.`). For `create`, change `RETURNING id, slug, name, status` to `RETURNING id, slug, name, status, created_at, created_by, NULL::text AS owner_name` (the creator's name isn't joined on insert; the DTO's `owner_name` is populated on subsequent reads).

- [ ] **Step 5: Extend the DTO**

In `crates/egret-workspace/src/routes.rs`, replace `WorkspaceDto` (lines 55–71):

```rust
#[derive(Serialize, utoipa::ToSchema)]
pub struct WorkspaceDto {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub created_by: Option<String>,
    pub owner_name: Option<String>,
}
impl From<crate::model::Workspace> for WorkspaceDto {
    fn from(w: crate::model::Workspace) -> Self {
        WorkspaceDto {
            id: w.id.to_string(),
            slug: w.slug,
            name: w.name,
            status: w.status,
            created_at: w.created_at,
            created_by: w.created_by.map(|u| u.to_string()),
            owner_name: w.owner_name,
        }
    }
}
```

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test -p egret-workspace workspace_dto_exposes_owner -- --nocapture`
Expected: PASS. Then run the whole crate to catch call-site breakage: `cargo test -p egret-workspace`
Expected: PASS (fix any `Workspace { .. }` literal construction sites the compiler flags — e.g. in `mapping.rs` or tests — by adding the three new fields).

- [ ] **Step 7: Commit**

```bash
git add crates/egret-workspace/src/model.rs crates/egret-workspace/src/workspaces.rs crates/egret-workspace/src/routes.rs crates/egret-workspace/tests/workspace_db.rs
git commit -m "feat(workspace): expose owner + created_at on WorkspaceDto"
```

---

## Task 3: Backend — Usage-stats endpoint (member + query counts)

**Files:**
- Modify: `crates/egret-workspace/src/workspaces.rs`
- Modify: `crates/egret-workspace/src/routes.rs`
- Test: `crates/egret-workspace/tests/workspace_db.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[sqlx::test]
async fn workspace_stats_returns_member_and_query_counts(pool: PgPool) {
    let (app, admin) = admin_app(pool.clone()).await;
    let ws = create_ws_via_api(&app, &admin, "stats-ws", "Stats WS").await;

    let body = json_body(req(&app, &admin, "GET", &format!("/api/admin/workspaces/{ws}/stats"), None).await).await;
    // creator is a member; no queries yet
    assert!(body["member_count"].as_i64().unwrap() >= 1, "creator counts as a member");
    assert_eq!(body["query_count"], 0);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p egret-workspace workspace_stats_returns -- --nocapture`
Expected: FAIL — 404 (route not registered).

- [ ] **Step 3: Add the query-count repo method**

In `crates/egret-workspace/src/workspaces.rs`, after `set_status`:

```rust
    /// Count queries submitted in a workspace. Raw COUNT against the stable
    /// `queries` table (migration 0009); no Rust dependency on egret-query.
    pub async fn count_queries(&self, id: WorkspaceId) -> Result<i64> {
        let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM queries WHERE workspace_id = $1")
            .bind(id.0)
            .fetch_one(&self.db.pool)
            .await
            .map_err(db_err)?;
        Ok(n)
    }
```

- [ ] **Step 4: Add the DTO, handler, route, and OpenAPI registration**

In `crates/egret-workspace/src/routes.rs`, add the DTO near `WorkspaceDto`:

```rust
#[derive(Serialize, utoipa::ToSchema)]
pub struct WorkspaceStatsDto {
    pub member_count: i64,
    pub query_count: i64,
}
```

Add the handler (near `get_ws`):

```rust
#[utoipa::path(
    get,
    path = "/api/admin/workspaces/{id}/stats",
    tag = "admin-workspaces",
    params(("id" = Uuid, Path, description = "Workspace id")),
    responses(
        (status = 200, description = "Workspace usage stats", body = WorkspaceStatsDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing platform permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn workspace_stats(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<WorkspaceStatsDto>, AuthError> {
    egret_core::authz::require(&*st.pdp, &p, "workspace:read")
        .await
        .map_err(AuthError)?;
    let wid = WorkspaceId(id);
    let member_count = st.members.list(wid).await.map_err(AuthError)?.len() as i64;
    let query_count = st.workspaces.count_queries(wid).await.map_err(AuthError)?;
    Ok(Json(WorkspaceStatsDto { member_count, query_count }))
}
```

Add the route to `admin_workspace_router`:

```rust
        .route("/workspaces/{id}/stats", get(workspace_stats))
```

Register in the `AdminWorkspaceApi` derive: add `workspace_stats` to `paths(...)` and `WorkspaceStatsDto` to `components(schemas(...))`:

```rust
    paths(create_ws, list_ws, get_ws, update_ws, archive_ws, restore_ws, workspace_stats),
    components(schemas(WorkspaceDto, WorkspaceStatsDto, CreateWorkspace, UpdateWorkspace, Ack, ErrorBody)),
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p egret-workspace workspace_stats_returns -- --nocapture`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/egret-workspace/src/workspaces.rs crates/egret-workspace/src/routes.rs crates/egret-workspace/tests/workspace_db.rs
git commit -m "feat(workspace): usage-stats endpoint (member + query counts)"
```

---

## Task 4: Backend — Ownership transfer

**Files:**
- Modify: `crates/egret-workspace/src/workspaces.rs`
- Modify: `crates/egret-workspace/src/routes.rs`
- Test: `crates/egret-workspace/tests/workspace_db.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[sqlx::test]
async fn transfer_owner_sets_owner_and_adds_admin(pool: PgPool) {
    let (app, admin) = admin_app(pool.clone()).await;
    let ws = create_ws_via_api(&app, &admin, "xfer", "Transfer WS").await;
    let bob = seed_user(&pool, "bob@egret.local").await; // existing test helper; returns Uuid

    let body = serde_json::json!({ "new_owner_id": bob.to_string() });
    let res = req(&app, &admin, "POST", &format!("/api/admin/workspaces/{ws}/transfer"), Some(body)).await;
    assert_eq!(res.status(), StatusCode::OK);

    // owner reflected in the DTO
    let got = json_body(req(&app, &admin, "GET", &format!("/api/admin/workspaces/{ws}"), None).await).await;
    assert_eq!(got["created_by"], bob.to_string());

    // bob is now a ws_admin member
    let members = json_body(req(&app, &admin, "GET", &format!("/api/ws/{ws}/members"), None).await).await;
    let has_admin = members.as_array().unwrap().iter().any(|m| {
        m["principal_id"] == bob.to_string() && m["role"] == "ws_admin"
    });
    assert!(has_admin, "new owner added as ws_admin");
}
```

> If `seed_user` isn't the existing helper name, use whatever the members tests use to insert a user row.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p egret-workspace transfer_owner_sets_owner -- --nocapture`
Expected: FAIL — 404 (route not registered).

- [ ] **Step 3: Add the set-owner repo method**

In `crates/egret-workspace/src/workspaces.rs`, after `set_status`:

```rust
    pub async fn set_owner(&self, id: WorkspaceId, owner: UserId) -> Result<()> {
        sqlx::query("UPDATE workspaces SET created_by = $1 WHERE id = $2")
            .bind(owner.0)
            .bind(id.0)
            .execute(&self.db.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }
```

- [ ] **Step 4: Add the DTO, handler, route, and OpenAPI registration**

In `crates/egret-workspace/src/routes.rs`, add the DTO:

```rust
#[derive(Deserialize, utoipa::ToSchema)]
pub struct TransferOwner {
    pub new_owner_id: String,
}
```

Add the handler. It sets the owner, ensures the new owner is a `ws_admin` member (idempotent via the existing upsert-style `MemberRepo::add`), and emits an audit event:

```rust
#[utoipa::path(
    post,
    path = "/api/admin/workspaces/{id}/transfer",
    tag = "admin-workspaces",
    params(("id" = Uuid, Path, description = "Workspace id")),
    request_body = TransferOwner,
    responses(
        (status = 200, description = "Workspace updated", body = WorkspaceDto),
        (status = 400, description = "Invalid owner id", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing platform permission", body = ErrorBody),
        (status = 404, description = "No such workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn transfer_owner(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<TransferOwner>,
) -> Result<Json<WorkspaceDto>, AuthError> {
    egret_core::authz::require(&*st.pdp, &p, "workspace:update")
        .await
        .map_err(AuthError)?;
    let new_owner = Uuid::parse_str(&body.new_owner_id)
        .map_err(|_| AuthError(Error::InvalidInput("new_owner_id must be a uuid".into())))?;
    let wid = WorkspaceId(id);

    // Ensure the workspace exists (404 otherwise).
    let _ = st
        .workspaces
        .get_by_id(wid)
        .await
        .map_err(AuthError)?
        .ok_or(AuthError(Error::NotFound("workspace".into())))?;

    // Make the new owner a ws_admin (idempotent upsert; role-replace if already a member).
    st.members
        .add(wid, "user", new_owner, "ws_admin", p.user_id.0)
        .await
        .map_err(AuthError)?;

    st.workspaces
        .set_owner(wid, UserId(new_owner))
        .await
        .map_err(AuthError)?;

    st.audit.emit(egret_audit::AuditEvent::Mutation {
        actor: Some(p.user_id.0),
        workspace_id: Some(id),
        action: "workspace:transfer_owner".into(),
        resource_type: "workspace".into(),
        resource_id: Some(id.to_string()),
        detail: serde_json::json!({ "new_owner_id": new_owner }),
    });

    let ws = st
        .workspaces
        .get_by_id(wid)
        .await
        .map_err(AuthError)?
        .ok_or(AuthError(Error::NotFound("workspace".into())))?;
    Ok(Json(ws.into()))
}
```

> Verify `MemberRepo::add`'s exact signature in `crates/egret-workspace/src/members.rs` (the `add_member` handler resolves a principal then calls the repo). Match its parameter order/types — the pseudocall above assumes `add(ws, principal_type, principal_id, role, granted_by)`. Adjust to the real signature; do not invent one.

Add the route to `admin_workspace_router`:

```rust
        .route("/workspaces/{id}/transfer", post(transfer_owner))
```

Register in the `AdminWorkspaceApi` derive: add `transfer_owner` to `paths(...)` and `TransferOwner` to `components(schemas(...))`.

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p egret-workspace transfer_owner_sets_owner -- --nocapture`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/egret-workspace/src/workspaces.rs crates/egret-workspace/src/routes.rs crates/egret-workspace/tests/workspace_db.rs
git commit -m "feat(workspace): ownership-transfer endpoint with audit"
```

---

## Task 5: Regenerate OpenAPI + orval client

**Files:**
- Modify: `openapi.json` (repo root)
- Modify: `frontend/src/api/generated/admin-workspaces/admin-workspaces.ts` (+ `model/`)

- [ ] **Step 1: Emit the updated OpenAPI spec**

The spec is produced offline by the `egret openapi` subcommand (see `openapi_doc()` in `crates/egret-server/src/lib.rs`). Run:

Run: `cargo run -p egret-bin -- openapi > openapi.json`
(Use the actual binary/subcommand this repo uses to emit the spec — confirm via `cargo run -p egret-bin -- --help`; the doc comment at `crates/egret-server/src/lib.rs:83` states an `egret openapi` subcommand emits it offline.)
Expected: `openapi.json` now contains `/api/admin/workspaces/{id}/restore`, `/stats`, `/transfer`, and the new `WorkspaceStatsDto`/`TransferOwner` schemas + owner/created fields on `WorkspaceDto`.

- [ ] **Step 2: Regenerate the client**

Run: `cd frontend && npm run gen:api`
Expected: `src/api/generated/admin-workspaces/admin-workspaces.ts` gains create/update/restore/stats/transfer hooks; `WorkspaceDto` model gains `createdAt`, `createdBy`, `ownerName`.

- [ ] **Step 3: Typecheck**

Run: `cd frontend && npm run typecheck`
Expected: PASS (generated code compiles).

- [ ] **Step 4: Commit**

```bash
git add openapi.json frontend/src/api/generated
git commit -m "chore(api): regenerate client for workspace admin additions"
```

---

## Task 6: Frontend — Extract `MembersPanel` from `Members.tsx`

**Files:**
- Create: `frontend/src/components/access/MembersPanel.tsx`
- Modify: `frontend/src/routes/explore/settings/Members.tsx`
- Test: existing `Members` tests must still pass.

- [ ] **Step 1: Create the panel component**

Read `frontend/src/routes/explore/settings/Members.tsx` fully. Create `frontend/src/components/access/MembersPanel.tsx` exporting a component that takes the workspace id as a prop instead of reading the route:

```tsx
export function MembersPanel({ workspaceId }: { workspaceId: string }) {
  const ws = workspaceId;
  // ...move the ENTIRE body of the current Members() component here verbatim,
  // deleting only the `const ws = useParams().workspace!;` line (ws now comes from the prop).
  // Keep all hooks (useWorkspaceMembers(ws), useAddMember(ws), useRemoveMember(ws), etc.),
  // dialogs, and JSX unchanged.
}
```

- [ ] **Step 2: Make the page a thin wrapper**

Replace the body of `frontend/src/routes/explore/settings/Members.tsx`:

```tsx
import { useParams } from "react-router-dom";
import { MembersPanel } from "@/components/access/MembersPanel";

export default function Members() {
  const ws = useParams().workspace!;
  return <MembersPanel workspaceId={ws} />;
}
```

Move any in-file sub-dialogs (the role-picker / add-member dialogs at the bottom of the original file) into `MembersPanel.tsx` alongside the component.

- [ ] **Step 3: Run the existing Members tests**

Run: `cd frontend && npx vitest run src/routes/explore/settings`
Expected: PASS (behaviour unchanged; if a test imported `Members` and mocked `useParams`, it still works through the wrapper).

- [ ] **Step 4: Commit**

```bash
git add frontend/src/components/access/MembersPanel.tsx frontend/src/routes/explore/settings/Members.tsx
git commit -m "refactor(members): extract reusable MembersPanel from Members page"
```

---

## Task 7: Frontend — Workspaces list page

**Files:**
- Create: `frontend/src/routes/control-plane/Workspaces.tsx`
- Modify: `frontend/src/App.tsx`
- Test: `frontend/src/routes/control-plane/__tests__/Workspaces.test.tsx`

- [ ] **Step 1: Write the failing test**

```tsx
import { render, screen, waitFor } from "@testing-library/react";
// mock the generated admin-workspaces hooks to return two workspaces (one archived)
// follow the mocking style used in access/__tests__/UsersPage.test.tsx

it("renders workspaces with a summary strip and status filter", async () => {
  renderWorkspaces([
    { id: "1", slug: "eng", name: "Engineering", status: "active", createdAt: "2026-03-01T00:00:00Z", ownerName: "alice" },
    { id: "2", slug: "old", name: "Old WS", status: "archived", createdAt: "2026-01-01T00:00:00Z", ownerName: null },
  ]);
  expect(await screen.findByText("Engineering")).toBeInTheDocument();
  expect(screen.getByText("Old WS")).toBeInTheDocument();
  // summary strip shows totals
  await waitFor(() => expect(screen.getByText(/2 total/i)).toBeInTheDocument());
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npx vitest run src/routes/control-plane/__tests__/Workspaces.test.tsx`
Expected: FAIL — module `Workspaces.tsx` not found.

- [ ] **Step 3: Build the page**

Create `frontend/src/routes/control-plane/Workspaces.tsx` modelled on `access/UsersPage.tsx`:

- `useAdminWorkspacesListWs()` for data.
- `SummaryStrip` with total / active / archived counts.
- `FilterableTable<WorkspaceDto>` with columns: Slug (mono), Name (links to `./${id}`), Owner (`ownerName ?? "—"`), Created (`RelativeTime`/date), Status (`Badge`, green active / muted archived), and a `RowActionsMenu`.
- `FacetSelect` status filter (All / active / archived).
- Toolbar **+ New workspace** button → dialog with `slug` + `name` inputs; submit calls the generated create mutation, then invalidate the list query. Validate slug client-side: `/^[a-z0-9-]+$/`.
- Row menu: **Open** (navigate to detail), **Rename** (dialog → update mutation), **Archive** (confirm → delete mutation) for active rows; **Restore** (restore mutation) for archived rows.
- Use OKLCH tokens via `var(--token)` in any inline styles; reuse shadcn primitives.

- [ ] **Step 4: Wire the route**

In `frontend/src/App.tsx`, replace the placeholder line (~136):

```tsx
<Route path="workspaces" element={<RequireGlobalAdmin><Workspaces /></RequireGlobalAdmin>} />
```

Add the import at the top with the other `routes/control-plane/*` imports:

```tsx
import Workspaces from "@/routes/control-plane/Workspaces";
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cd frontend && npx vitest run src/routes/control-plane/__tests__/Workspaces.test.tsx`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add frontend/src/routes/control-plane/Workspaces.tsx frontend/src/routes/control-plane/__tests__/Workspaces.test.tsx frontend/src/App.tsx
git commit -m "feat(control-plane): workspaces list page (create/rename/archive/restore)"
```

---

## Task 8: Frontend — Workspace detail shell + Overview tab

**Files:**
- Create: `frontend/src/routes/control-plane/WorkspaceDetail.tsx`
- Modify: `frontend/src/App.tsx`
- Test: `frontend/src/routes/control-plane/__tests__/WorkspaceDetail.test.tsx`

- [ ] **Step 1: Write the failing test**

```tsx
it("shows the overview with owner, status, and stat counts", async () => {
  renderDetail({
    ws: { id: "1", slug: "eng", name: "Engineering", status: "active", createdAt: "2026-03-01T00:00:00Z", ownerName: "alice" },
    stats: { memberCount: 7, queryCount: 42 },
  });
  expect(await screen.findByText("Engineering")).toBeInTheDocument();
  expect(screen.getByText("alice")).toBeInTheDocument();
  expect(screen.getByText("7")).toBeInTheDocument();   // members
  expect(screen.getByText("42")).toBeInTheDocument();  // queries
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npx vitest run src/routes/control-plane/__tests__/WorkspaceDetail.test.tsx`
Expected: FAIL — module not found.

- [ ] **Step 3: Build the shell + Overview**

Create `frontend/src/routes/control-plane/WorkspaceDetail.tsx`:

- `const id = useParams().id!`.
- `useAdminWorkspacesGetWs(id)` for the header; `useAdminWorkspacesStats(id)` (generated) for the stat row.
- Header: back link to `../workspaces`, name, `slug` mono chip, status `Badge`, and action buttons: **Rename**, **Archive**/**Restore**, **Transfer owner** (opens a dialog — built in Task 9/10 or inline here).
- Tabs component (shadcn `Tabs`) with values `overview | members | resources | requests | activity`, synced to `?tab=` via `useSearchParams`.
- **Overview** panel: a definition list (Owner `ownerName ?? "—"`, Created `createdAt` formatted, Status) + a stat row of two `SummaryStrip`-style tiles: Members = `memberCount`, Queries run = `queryCount`.

- [ ] **Step 4: Wire the route**

In `frontend/src/App.tsx`, after the `workspaces` route add:

```tsx
<Route path="workspaces/:id" element={<RequireGlobalAdmin><WorkspaceDetail /></RequireGlobalAdmin>} />
```

Add the import:

```tsx
import WorkspaceDetail from "@/routes/control-plane/WorkspaceDetail";
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cd frontend && npx vitest run src/routes/control-plane/__tests__/WorkspaceDetail.test.tsx`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add frontend/src/routes/control-plane/WorkspaceDetail.tsx frontend/src/routes/control-plane/__tests__/WorkspaceDetail.test.tsx frontend/src/App.tsx
git commit -m "feat(control-plane): workspace detail shell + overview tab"
```

---

## Task 9: Frontend — Members tab + Resources tab + Transfer dialog

**Files:**
- Modify: `frontend/src/routes/control-plane/WorkspaceDetail.tsx`
- Test: `frontend/src/routes/control-plane/__tests__/WorkspaceDetail.test.tsx`

- [ ] **Step 1: Extend the test**

```tsx
it("renders the members panel and resource cards, and submits a transfer", async () => {
  const transfer = vi.fn();
  renderDetail({ /* ws + stats as above */, onTransfer: transfer });
  // switch to Members tab -> MembersPanel renders (mock useWorkspaceMembers to 1 member)
  await userEvent.click(screen.getByRole("tab", { name: /members/i }));
  expect(await screen.findByText(/member/i)).toBeInTheDocument();
  // Resources tab -> deep-link cards present
  await userEvent.click(screen.getByRole("tab", { name: /resources/i }));
  expect(screen.getByRole("link", { name: /warehouses/i })).toBeInTheDocument();
  // Transfer dialog
  await userEvent.click(screen.getByRole("button", { name: /transfer owner/i }));
  // ...fill + submit -> expect transfer mutation called
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npx vitest run src/routes/control-plane/__tests__/WorkspaceDetail.test.tsx`
Expected: FAIL — tabs/cards/dialog not present yet.

- [ ] **Step 3: Implement Members + Resources + Transfer**

- **Members** panel: `<MembersPanel workspaceId={id} />` (from Task 6). Because `platform_admin` bypasses the `workspace:manage_members` gate, the panel's add/remove/change-role work for any workspace with no changes.
- **Resources** panel: four `Card`s (Warehouses, Storage, Engines, Secrets), each a `Link` to the corresponding CP page:
  - Warehouses → `/control-plane/warehouses`
  - Storage → `/control-plane/storage`
  - Engines → `/control-plane/engines`
  - Secrets → `/control-plane/secrets`
  Add a one-line note that these are platform-wide views (storage/secrets are global; warehouses/engines list across workspaces). No live per-workspace counts in v1 (see plan deviations).
- **Transfer owner** dialog: a user picker (reuse the principal picker used by `MembersPanel`'s add-member dialog, or a simple email→id resolve) → calls the generated transfer mutation with `{ newOwnerId }`, then invalidates the get + list queries. Show a success toast.

- [ ] **Step 4: Run test to verify it passes**

Run: `cd frontend && npx vitest run src/routes/control-plane/__tests__/WorkspaceDetail.test.tsx`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add frontend/src/routes/control-plane/WorkspaceDetail.tsx frontend/src/routes/control-plane/__tests__/WorkspaceDetail.test.tsx
git commit -m "feat(control-plane): workspace detail members, resources, transfer-owner"
```

---

## Task 10: Frontend — Access-requests tab + Activity tab

**Files:**
- Modify: `frontend/src/routes/control-plane/WorkspaceDetail.tsx`
- Test: `frontend/src/routes/control-plane/__tests__/WorkspaceDetail.test.tsx`

- [ ] **Step 1: Extend the test**

```tsx
it("lists pending access requests with approve/deny and shows activity", async () => {
  renderDetail({
    /* ws + stats */,
    accessRequests: [{ id: "r1", requesterName: "carol", status: "pending" }],
    audit: [{ id: "a1", action: "workspace:transfer_owner", actorName: "admin", at: "2026-07-14T00:00:00Z" }],
  });
  await userEvent.click(screen.getByRole("tab", { name: /access requests/i }));
  expect(await screen.findByText("carol")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: /approve/i })).toBeInTheDocument();

  await userEvent.click(screen.getByRole("tab", { name: /activity/i }));
  expect(await screen.findByText(/transfer_owner/i)).toBeInTheDocument();
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npx vitest run src/routes/control-plane/__tests__/WorkspaceDetail.test.tsx`
Expected: FAIL — the two tabs aren't implemented.

- [ ] **Step 3: Implement Access requests + Activity**

- **Access requests** panel: fetch `GET /api/ws/{id}/access-requests` (add a hook to `api/access.ts` or use the generated `workspace-members` client if the operation is present; the routes exist in `ws_member_router`). Render pending rows with **Approve** / **Deny** buttons calling `.../access-requests/{reqId}/approve|deny`; invalidate on success. Empty state: "No pending requests."
- **Activity** panel: `useControlPlaneAuditListAudit({ workspace: id })` (the generated CP-audit hook; confirm the query param name is `workspace` from `crates/egret-controlplane/src/audit.rs`). Render the same columns `AuditLog.tsx` uses (time, actor, action, resource, allow/deny). Reuse a shared row renderer if `AuditLog.tsx` exposes one; otherwise a local table matching its columns.

> `platform_admin` bypasses the workspace gate on the access-request approve/deny handlers, so these work from the control plane.

- [ ] **Step 4: Run test to verify it passes**

Run: `cd frontend && npx vitest run src/routes/control-plane/__tests__/WorkspaceDetail.test.tsx`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add frontend/src/routes/control-plane/WorkspaceDetail.tsx frontend/src/routes/control-plane/__tests__/WorkspaceDetail.test.tsx
git commit -m "feat(control-plane): workspace detail access-requests + activity tabs"
```

---

## Task 11: Full verification + rollout

**Files:** none (verification only).

- [ ] **Step 1: Backend CI locally**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p egret-workspace`
Expected: all green. (Full `cargo test --workspace` if touching shared types — do NOT run two workspace test suites concurrently.)

- [ ] **Step 2: Frontend CI locally**

Run: `cd frontend && npm run lint && npm run typecheck && npx vitest run && npm run build`
Expected: all green.

- [ ] **Step 3: Live smoke as platform_admin**

Start the stack (docker + `cargo run -p egret-bin -- serve` + `npm run dev`), log in `admin@egret.local` / `Admin12345!`, then via the browser (manual Playwright-MCP driving — NOT an automated test):
1. `/control-plane/workspaces` → create a workspace, confirm it appears active.
2. Open detail → Overview shows owner + created + member/query counts.
3. Members tab → add then remove a member.
4. Transfer owner to another user → Overview owner updates; the new owner appears as `ws_admin`.
5. Access requests + Activity tabs render (Activity shows the `workspace:transfer_owner` event).
6. Archive the workspace from the list → row shows archived; **Restore** → back to active.

- [ ] **Step 4: Finish the branch**

Use the `superpowers:finishing-a-development-branch` skill to open the PR (or squash-merge with `--admin` per the project's out-of-quota-CI rule, only after Steps 1–2 are green locally). PR title: `feat(control-plane): workspace management cockpit`.

---

## Self-review notes

- **Spec coverage:** list/create/rename/archive (Task 7), restore (Task 1/7), owner+created (Task 2/8), usage stats (Task 3/8), ownership transfer (Task 4/9), members (Task 6/9), resources (Task 9), access-requests + activity (Task 10). Storage-byte/storage-conn stats intentionally dropped — see **Deviations** (storage is global).
- **Type consistency:** backend `WorkspaceStatsDto { member_count, query_count }` ↔ FE `{ memberCount, queryCount }` (orval camel-cases). `TransferOwner { new_owner_id }` ↔ FE `{ newOwnerId }`. `WorkspaceDto` gains `created_at`/`created_by`/`owner_name` ↔ `createdAt`/`createdBy`/`ownerName`.
- **Verify-before-invent flags:** confirm `MemberRepo::add` signature (Task 4), the `egret openapi` emit command (Task 5), the CP-audit query param name (Task 10), and the members test helpers (`admin_app`/`seed_user`) before writing each — the plan calls these out inline.
