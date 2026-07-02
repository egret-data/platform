# Control-Plane Polaris — Plan 2: move Storage + Secrets to the control plane Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.
> Spec: `docs/en/specs/2026-07-02-control-plane-polaris-catalog-design.md`. **P2 of the epic** (after P1 removal). VI reference: `docs/vi/plans/2026-07-02-polaris-p2-storage-secrets-control-plane.md`.

**Goal:** Make Storage connections **global** and rebuild Storage + Secrets management at the **control plane** (`platform_admin` only), since P1 deleted the routes that used to serve them (they lived in the now-removed `egret-catalog-engine::browse.rs`). Workspaces no longer manage Storage/Secrets.

**Architecture:** Re-scope storage to a global `storage_connections` table (drop `workspace_id`); make `workspace_secrets` **global-capable** (nullable `workspace_id`, `NULL` = a global/control-plane secret) so storage can reference global secrets while `egret-database` keeps its workspace-scoped secrets unchanged. Add thin `platform_admin`-gated control-plane routers for both, and control-plane UI pages; remove the workspace Storage/Secrets pages.

**Tech Stack:** Rust (axum, sqlx), Postgres migration, React/TS.

---

## Background the implementer needs

- **P1 already removed** the storage + secrets HTTP routes (they were in `egret-catalog-engine/src/browse.rs`, e.g. `/ws/{workspace}/storages`, `/ws/{workspace}/secrets`, now deleted). The **stores survive**: `egret-catalog::service::storage_conn` (`WorkspaceStorageConn`, `create_storage_conn`/`list_storage_conns`/`get`/`update`/`delete`, table `workspace_storages`, columns incl. `workspace_id`, `name`, `purpose`, `provider`, `endpoint`, `region`, `bucket`, `key_prefix`, `access_key`, `secret_ref`, `path_style`) and `egret-secrets::WorkspaceSecretStore` (`create`/`list`/`get_meta`/`update`/`delete`, table `workspace_secrets(id, workspace_id NOT NULL, name, description, secret_ref → secrets(id))`, `UNIQUE(workspace_id, name)`).
- **`workspace_secrets` is shared:** `egret-database` (`egret-database/src/routes.rs`, `resolve_secret_id`) uses it workspace-scoped for DB connection creds. Do NOT break that — databases stay workspace-scoped.
- **Auth gate:** control-plane routes have no workspace context; use the `egret_auth` `Auth(principal)` extractor and require `principal.platform_roles` contains `"platform_admin"` (the same string checked elsewhere, e.g. `egret-engine` `is_admin`). Frontend uses `RequireGlobalAdmin` (see `App.tsx` control-plane routes).
- Migrations at repo-root `migrations/`; P1 added `0048`, so next is **`0049`** (and `0050` if two). `touch crates/egret-metastore/src/lib.rs` after adding.
- Frontend: `App.tsx` — workspace routes `storage`→`StoragePage`, `secrets`→`SecretsPage` (remove); control-plane routes live under `/control-plane/*` (`ControlPlaneRoutes`, `RequireGlobalAdmin`).

---

## Task 1: Migration — storage global + secrets global-capable

**Files:**
- Create: `migrations/0049_storage_secrets_control_plane.sql`
- Modify: `crates/egret-metastore/src/lib.rs` (touch)

- [ ] **Step 1: Write the migration**

```sql
-- Control-plane pivot: Storage becomes global; Secrets gain a global scope.
-- Greenfield: existing workspace_storages rows are dev-only — drop the FK/scope.

-- 1. Storage: drop workspace scoping (rename table for clarity).
ALTER TABLE workspace_storages DROP COLUMN workspace_id;
ALTER TABLE workspace_storages RENAME TO storage_connections;
-- name is now globally unique.
DROP INDEX IF EXISTS workspace_storages_workspace_id_idx;
ALTER TABLE storage_connections ADD CONSTRAINT storage_connections_name_key UNIQUE (name);

-- 2. Secrets: allow global (control-plane) secrets = workspace_id NULL.
ALTER TABLE workspace_secrets ALTER COLUMN workspace_id DROP NOT NULL;
-- Per-workspace names stay unique per workspace; global names unique among globals.
CREATE UNIQUE INDEX workspace_secrets_global_name_key
    ON workspace_secrets (name) WHERE workspace_id IS NULL;
```

- [ ] **Step 2: Re-embed + apply + verify**

Run: `touch crates/egret-metastore/src/lib.rs && cargo run -q -p egret-bin -- migrate`
Then: `docker exec egret-dev-postgres-1 psql -U egret -d egret -c "\d storage_connections; \d workspace_secrets"`
Expected: `storage_connections` has no `workspace_id`; `workspace_secrets.workspace_id` is nullable.

- [ ] **Step 3: Commit**

```bash
git add migrations/0049_storage_secrets_control_plane.sql crates/egret-metastore/src/lib.rs
git commit -m "feat(db): storage_connections global; workspace_secrets global-capable (nullable workspace_id)"
```

---

## Task 2: Re-scope the storage store to global

**Files:**
- Modify: `crates/egret-catalog/src/service/storage_conn.rs`
- Test: `crates/egret-catalog/tests/` (the storage store test — `rg -l storage_conn crates/egret-catalog/tests`)

- [ ] **Step 1: Update the failing test to the global API**

In the storage store test, drop `workspace_id` from create/list/get calls and assertions (global CRUD): e.g. `s.create_storage_conn(&input).await` (no workspace), `s.list_storage_conns().await` returns all, `s.get(id)` / `s.delete(id)` by id only.

- [ ] **Step 2: Run to verify failure**

Run: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-catalog storage`
Expected: FAIL (signatures still take `workspace_id`).

- [ ] **Step 3: Remove `workspace_id` from the store**

In `storage_conn.rs`: drop the `workspace_id` field from `WorkspaceStorageConn` (rename the struct to `StorageConn`) and its `From` row; remove the `workspace_id` param + bind + `WHERE workspace_id = $` clauses from `create_storage_conn`/`list_storage_conns`/`get`/`update`/`delete` (table `storage_connections`). Map a duplicate-name insert to `AlreadyExists`.

- [ ] **Step 4: Run to verify pass**

Run: `DATABASE_URL=… cargo test -p egret-catalog storage` then `cargo build -p egret-catalog`
Expected: PASS + builds.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-catalog/src/service/storage_conn.rs crates/egret-catalog/tests/
git commit -m "refactor(storage): global storage_connections store (drop workspace scoping)"
```

---

## Task 3: Give the secrets store a global scope

**Files:**
- Modify: `crates/egret-secrets/src/workspace_secrets.rs`
- Test: `crates/egret-secrets/tests/` (or inline)

- [ ] **Step 1: Write the failing test for global secrets**

```rust
#[tokio::test]
async fn global_secret_roundtrips_alongside_workspace_secret() {
    let (store, ws) = fixture().await; // existing helper
    let g = store.create(None, "minio-key", None, "SECRETVAL").await.unwrap();       // global
    let w = store.create(Some(ws), "db-pw", None, "PW").await.unwrap();              // workspace
    assert!(store.list(None).await.unwrap().iter().any(|m| m.id == g));              // global list
    assert!(store.list(Some(ws)).await.unwrap().iter().any(|m| m.id == w));          // ws list
    assert!(!store.list(None).await.unwrap().iter().any(|m| m.id == w));             // scopes isolated
}
```

- [ ] **Step 2: Run to verify failure**

Run: `DATABASE_URL=… cargo test -p egret-secrets global_secret_roundtrips`
Expected: FAIL (`create`/`list` take `workspace_id: Uuid`, not `Option`).

- [ ] **Step 3: Make the scope optional**

In `workspace_secrets.rs`: change `create(&self, workspace_id: Uuid, …)` → `create(&self, workspace_id: Option<Uuid>, …)` and `list(&self, workspace_id: Uuid)` / `get_meta` / `update` / `delete` → accept `Option<Uuid>` (bind `NULL` for `None`; `WHERE workspace_id IS NOT DISTINCT FROM $1` so `None` matches the global rows). Keep behavior identical for `Some(ws)`.

- [ ] **Step 4: Fix `egret-database`'s callers**

`egret-database/src/routes.rs` `resolve_secret_id` calls `create`/`get_meta` with the workspace id — wrap as `Some(ctx.workspace_id.0)`. Run `cargo build -p egret-database`.

- [ ] **Step 5: Run to verify pass**

Run: `DATABASE_URL=… cargo test -p egret-secrets -p egret-database`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-secrets crates/egret-database/src/routes.rs
git commit -m "feat(secrets): optional workspace scope (None = global/control-plane secret)"
```

---

## Task 4: Control-plane routers for Storage + Secrets

**Files:**
- Create: `crates/egret-controlplane/` (new crate) with `src/{lib.rs, storage.rs, secrets.rs, auth.rs}` — OR add a `control_plane` module to an existing suitable crate if the codebase prefers (check: is there an existing control-plane crate? `rg -l control.plane crates`). Use a new `egret-controlplane` crate.
- Modify: `crates/egret-bin/src/main.rs` (construct state + mount), `crates/egret-server/src/lib.rs` (nest the router)

- [ ] **Step 1: Platform-admin guard**

In `auth.rs`, a helper over the `egret_auth` `Auth` extractor:

```rust
pub fn require_platform_admin(principal: &egret_core::Principal) -> Result<(), egret_core::Error> {
    if principal.platform_roles.iter().any(|r| r == "platform_admin") {
        Ok(())
    } else {
        Err(egret_core::Error::Forbidden)
    }
}
```
(Confirm the `Principal`/`Auth` type path via `rg "pub struct Principal|struct Auth" crates/egret-auth crates/egret-core`.)

- [ ] **Step 2: Storage router (control plane)**

`storage.rs` — a `Router` with `Auth`-gated handlers over the global `StorageConn` store:

```rust
// GET/POST /api/control-plane/storage ; GET/PUT/DELETE /api/control-plane/storage/{id}
async fn list(State(st): State<CpState>, Auth(p): Auth) -> Result<Json<Vec<StorageConnDto>>, CpError> {
    require_platform_admin(&p)?;
    Ok(Json(st.storage.list_storage_conns().await?.into_iter().map(Into::into).collect()))
}
async fn create(State(st): State<CpState>, Auth(p): Auth, Json(body): Json<StorageConnInput>) -> Result<(StatusCode, Json<StorageConnDto>), CpError> {
    require_platform_admin(&p)?;
    // resolve body.new_secret → a GLOBAL workspace_secrets row (workspace_id NULL), else body.secret_ref
    let conn = st.storage.create_storage_conn(&resolved).await?;
    Ok((StatusCode::CREATED, Json(conn.into())))
}
// get/update/delete analogous, all require_platform_admin.
```
Reuse the storage-connection validation the deleted `browse.rs` did (reachability check on save is OPTIONAL for v1 — skip live S3 validation here, or add a `test` endpoint later).

- [ ] **Step 3: Secrets router (control plane, global secrets)**

`secrets.rs` — `Auth`-gated CRUD over the secrets store with `None` scope (global):

```rust
// GET/POST /api/control-plane/secrets ; PUT/DELETE /api/control-plane/secrets/{id}
async fn create(State(st): State<CpState>, Auth(p): Auth, Json(b): Json<NewSecret>) -> Result<(StatusCode, Json<WorkspaceSecretMeta>), CpError> {
    require_platform_admin(&p)?;
    Ok((StatusCode::CREATED, Json(st.secrets.create(None, &b.name, b.description.as_deref(), &b.value).await?)))
}
// list(None)/update(None,..)/delete(None,..) analogous.
```

- [ ] **Step 4: Wire state + mount**

In `main.rs` build `CpState { storage: StorageConn::new(db.clone()), secrets: workspace_secrets.clone() }`; in `egret-server/src/lib.rs` nest the control-plane router at `/api/control-plane` (behind the CSRF layer like other mutating APIs). Add unit/integration tests: every endpoint returns 403 for a non-`platform_admin` principal; 200/201 for a platform admin.

- [ ] **Step 5: Build + test**

Run: `cargo build -p egret-controlplane -p egret-bin && DATABASE_URL=… cargo test -p egret-controlplane`
Expected: builds + authz tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-controlplane crates/egret-bin crates/egret-server
git commit -m "feat(control-plane): platform-admin Storage + Secrets routers"
```

---

## Task 5: Frontend — control-plane Storage + Secrets pages

**Files:**
- Create: `frontend/src/routes/control-plane/ControlPlaneStorage.tsx`, `ControlPlaneSecrets.tsx`; API client `frontend/src/api/controlPlane.ts`
- Modify: `frontend/src/App.tsx` (add control-plane routes, remove workspace storage/secrets routes)
- Delete: `frontend/src/routes/explore/StoragePage.tsx`, `SecretsPage.tsx`

- [ ] **Step 1: API client** — `controlPlane.ts`: `listStorage`/`createStorage`/`deleteStorage` → `/api/control-plane/storage`; `listSecrets`/`createSecret`/`deleteSecret` → `/api/control-plane/secrets`.

- [ ] **Step 2: Pages** — `ControlPlaneStorage.tsx` (table Name/Provider/Endpoint/Bucket/Actions + "Add connection" dialog referencing a global secret) and `ControlPlaneSecrets.tsx` (table Name/Description/Actions + "Add secret") — mirror the removed workspace pages' UX. Under `RequireGlobalAdmin`.

- [ ] **Step 3: Routes** — in `App.tsx` `ControlPlaneRoutes`, add `<Route path="storage" .../>` + `<Route path="secrets" .../>`; remove the workspace `storage`/`secrets` routes + imports; `git rm frontend/src/routes/explore/StoragePage.tsx frontend/src/routes/explore/SecretsPage.tsx`. Remove Storage/Secrets from the workspace nav; add them to the control-plane nav.

- [ ] **Step 4: Typecheck + lint + build**

Run: `cd frontend && npm run typecheck && npm run lint && npm run build`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(ui): control-plane Storage + Secrets pages; remove workspace ones"
```

---

## Task 6: Workspace verification

- [ ] **Step 1: Backend** — `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && DATABASE_URL=… cargo test --workspace` → all clean/pass.
- [ ] **Step 2: Frontend** — `cd frontend && npm run typecheck && npm run lint && npm run build` → clean.
- [ ] **Step 3: Manual** — as a platform admin: create a global secret + a storage connection at `/control-plane/storage`; confirm a non-admin gets 403 on `/api/control-plane/storage`.

---

## Self-review notes (spec coverage for P2)

- Spec **Storage + Secrets → control plane**: storage global (Tasks 1–2), secrets global-capable (Tasks 1,3), control-plane routers `platform_admin`-gated (Task 4), UI moved (Task 5).
- **Databases unaffected:** `workspace_secrets` stays usable workspace-scoped (`Some(ws)`); only a global scope (`None`) is added.
- **Note:** P1 already deleted the old workspace storage/secrets routes (browse.rs); P2 rebuilds them at the control plane (fresh thin handlers), so there is no code to "port".
- **Out of P2:** `egret-polaris` (P3), catalog + RBAC + assignment routes (P4), full control-plane catalog UI (P5), Polaris compose/e2e (P6).
