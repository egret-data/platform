# Phase 1a-2 — Workspace & RBAC Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the workspace entity, membership, and union-semantics RBAC (`egret-workspace` crate) with admin/workspace routes, plus the security hardening deferred from 1a-1 (login rate-limit, dummy-verify, `egret init --force`).

**Architecture:** New `egret-workspace` crate (depends on `egret-auth`; no cycle) with `WorkspaceRepo`, `MemberRepo`, `Authorizer` (most-permissive union across direct + group roles), and a `WsContext` extractor. Routes mount into `egret-server` behind the existing `csrf_layer`. Per the [Phase 1a-2 design spec](../specs/2026-06-17-phase-1a-2-workspace-rbac-design.md).

**Tech Stack:** Rust (stable), axum 0.8, sqlx 0.8 (runtime queries), argon2, std `Instant` rate-limiter. Reuses Phase 1a-1 helpers.

**Reference:** [Phase 1a-2 spec](../specs/2026-06-17-phase-1a-2-workspace-rbac-design.md); existing crate `egret-auth` (modules `password`, `token`, `model`, `users`, `session`, `cookie`, `principal` [`Auth`,`AuthError`], `csrf`, `routes` [`AuthState`]).

**Conventions (reuse from 1a-1):**
- Runtime sqlx only (`sqlx::query`, `query_as`, `query_scalar`) — never compile-time macros.
- `fn db_err(e: sqlx::Error) -> egret_core::Error { egret_core::Error::Internal(anyhow::anyhow!(e)) }` in each repo module.
- DB-gated tests self-skip when `DATABASE_URL` is unset; CI / `DATABASE_URL=postgres://egret:egret@localhost:5432/egret` runs them for real.
- Commit per task with plain `git commit` (repo git identity is configured — NO `-c user.*` overrides) and trailer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

---

## File Structure

```text
migrations/0004_rbac_indexes.sql
crates/egret-workspace/Cargo.toml
crates/egret-workspace/src/lib.rs
crates/egret-workspace/src/model.rs           # Workspace, WorkspaceMember, role consts + precedence
crates/egret-workspace/src/workspaces.rs       # WorkspaceRepo
crates/egret-workspace/src/members.rs          # MemberRepo
crates/egret-workspace/src/rbac.rs             # Authorizer (union, group resolution)
crates/egret-workspace/src/mapping.rs          # map_idp_claims_to_roles
crates/egret-workspace/src/context.rs          # WsContext extractor
crates/egret-workspace/src/routes.rs           # WorkspaceState + admin/ws routers
crates/egret-workspace/tests/workspace_db.rs   # DB-gated integration tests
crates/egret-auth/src/session.rs               # MODIFY: platform_roles group branch
crates/egret-auth/src/ratelimit.rs             # NEW: RateLimiter
crates/egret-auth/src/routes.rs                # MODIFY: rate-limit + dummy-verify in login; AuthState gains rate_limiter
crates/egret-auth/src/users.rs                 # MODIFY: list()
crates/egret-auth/src/model.rs                 # MODIFY: remove unused Session
crates/egret-config/src/lib.rs                 # MODIFY: RateLimitConfig
crates/egret-server/src/lib.rs                 # MODIFY: AppState.workspace; mount routers
crates/egret-bin/src/cli.rs                    # MODIFY: Init { force }
crates/egret-bin/src/main.rs                   # MODIFY: init --force; build WorkspaceState + RateLimiter
crates/egret-metastore/src/lib.rs              # MODIFY: migrator count >= 4
```

**Note:** internal working plan — English only under `docs/en/plans/`.

---

### Task 1: Migration 0004 + `egret-workspace` scaffold + model

**Files:** Create `migrations/0004_rbac_indexes.sql`, `crates/egret-workspace/{Cargo.toml,src/lib.rs,src/model.rs}`; Modify `Cargo.toml`, `crates/egret-metastore/src/lib.rs`.

- [ ] **Step 1: Create the index migration**

`migrations/0004_rbac_indexes.sql`:
```sql
CREATE INDEX workspace_members_principal_idx ON workspace_members (principal_type, principal_id);
```

- [ ] **Step 2: Bump the migrator count test**

In `crates/egret-metastore/src/lib.rs`, change the assertion in `migrator_embeds_baseline_migration` to `>= 4`:
```rust
        assert!(
            MIGRATOR.migrations.len() >= 4,
            "expected baseline + identity + rbac + index migrations"
        );
```

- [ ] **Step 3: Register the crate**

In `Cargo.toml` `[workspace.dependencies]`, add:
```toml
egret-workspace = { path = "crates/egret-workspace" }
```

- [ ] **Step 4: Create the crate manifest**

`crates/egret-workspace/Cargo.toml`:
```toml
[package]
name = "egret-workspace"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
egret-core.workspace = true
egret-metastore.workspace = true
egret-auth.workspace = true
axum.workspace = true
sqlx.workspace = true
serde.workspace = true
serde_json.workspace = true
chrono.workspace = true
uuid.workspace = true
anyhow.workspace = true
tracing.workspace = true

[dev-dependencies]
tokio.workspace = true
tower.workspace = true
http-body-util = "0.1"
egret-config.workspace = true
```

- [ ] **Step 5: Write the model with a unit test**

`crates/egret-workspace/src/model.rs`:
```rust
//! Workspace + RBAC domain types and the fixed role catalog.

use egret_core::WorkspaceId;
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub slug: String,
    pub name: String,
    pub status: String, // active | archived
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceMember {
    pub principal_type: String, // user | group
    pub principal_id: Uuid,
    pub role: String,
}

pub const WORKSPACE_ROLES: &[&str] = &["ws_admin", "ws_editor", "ws_viewer"];
pub const PLATFORM_ROLES: &[&str] = &["platform_admin", "data_steward"];

/// Display precedence for a workspace role (higher = more privileged). Enforcement is
/// always union-based; this is only used to pick a single role name for display.
pub fn workspace_role_rank(role: &str) -> u8 {
    match role {
        "ws_admin" => 3,
        "ws_editor" => 2,
        "ws_viewer" => 1,
        _ => 0,
    }
}

/// Validate a workspace slug: non-empty, lowercase letters/digits/hyphen only.
pub fn is_valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 63
        && slug.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_validation() {
        assert!(is_valid_slug("sales-eu"));
        assert!(!is_valid_slug("Sales"));
        assert!(!is_valid_slug(""));
        assert!(!is_valid_slug("a b"));
    }

    #[test]
    fn role_precedence_orders_admin_highest() {
        assert!(workspace_role_rank("ws_admin") > workspace_role_rank("ws_editor"));
        assert!(workspace_role_rank("ws_editor") > workspace_role_rank("ws_viewer"));
        assert_eq!(workspace_role_rank("nope"), 0);
    }
}
```

`crates/egret-workspace/src/lib.rs`:
```rust
//! `egret-workspace` — workspace entity, membership, and RBAC authorization.

pub mod model;
```

- [ ] **Step 6: Build + test**

Run: `cargo test -p egret-workspace -p egret-metastore --lib`
Expected: PASS (slug + precedence unit tests; migrator count). Then `cargo fmt --all` and `cargo clippy -p egret-workspace --all-targets -- -D warnings`.

- [ ] **Step 7: Commit**
```bash
git add Cargo.toml migrations/0004_rbac_indexes.sql crates/egret-workspace crates/egret-metastore/src/lib.rs
git commit -m "feat(workspace): scaffold egret-workspace crate, model, rbac index migration"
```

---

### Task 2: `WorkspaceRepo`

**Files:** Create `crates/egret-workspace/src/workspaces.rs`, `crates/egret-workspace/tests/workspace_db.rs`; Modify `src/lib.rs`.

- [ ] **Step 1: Write the repository**

`crates/egret-workspace/src/workspaces.rs`:
```rust
//! Postgres-backed workspace CRUD.

use egret_core::{Error, Pagination, Result, UserId, WorkspaceId};
use egret_metastore::Db;
use uuid::Uuid;

use crate::model::{is_valid_slug, Workspace};

#[derive(Clone)]
pub struct WorkspaceRepo {
    db: Db,
}

impl WorkspaceRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub async fn create(&self, slug: &str, name: &str, created_by: UserId) -> Result<Workspace> {
        if !is_valid_slug(slug) {
            return Err(Error::InvalidInput(format!("invalid slug: {slug}")));
        }
        let row = sqlx::query_as::<_, WorkspaceRow>(
            "INSERT INTO workspaces (slug, name, created_by) VALUES ($1, $2, $3) \
             RETURNING id, slug, name, status",
        )
        .bind(slug)
        .bind(name)
        .bind(created_by.0)
        .fetch_one(&self.db.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::Database(db) if db.is_unique_violation() => {
                Error::Conflict(format!("workspace slug already exists: {slug}"))
            }
            other => db_err(other),
        })?;
        Ok(row.into())
    }

    pub async fn list(&self, page: Pagination) -> Result<Vec<Workspace>> {
        let rows = sqlx::query_as::<_, WorkspaceRow>(
            "SELECT id, slug, name, status FROM workspaces ORDER BY created_at DESC \
             LIMIT $1 OFFSET $2",
        )
        .bind(page.limit() as i64)
        .bind(page.offset() as i64)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn get_by_slug(&self, slug: &str) -> Result<Option<Workspace>> {
        let row = sqlx::query_as::<_, WorkspaceRow>(
            "SELECT id, slug, name, status FROM workspaces WHERE slug = $1",
        )
        .bind(slug)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(row.map(Into::into))
    }

    pub async fn get_by_id(&self, id: WorkspaceId) -> Result<Option<Workspace>> {
        let row = sqlx::query_as::<_, WorkspaceRow>(
            "SELECT id, slug, name, status FROM workspaces WHERE id = $1",
        )
        .bind(id.0)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(row.map(Into::into))
    }

    pub async fn update_name(&self, id: WorkspaceId, name: &str) -> Result<()> {
        sqlx::query("UPDATE workspaces SET name = $1 WHERE id = $2")
            .bind(name)
            .bind(id.0)
            .execute(&self.db.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    pub async fn archive(&self, id: WorkspaceId) -> Result<()> {
        sqlx::query("UPDATE workspaces SET status = 'archived' WHERE id = $1")
            .bind(id.0)
            .execute(&self.db.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }
}

#[derive(sqlx::FromRow)]
struct WorkspaceRow {
    id: Uuid,
    slug: String,
    name: String,
    status: String,
}

impl From<WorkspaceRow> for Workspace {
    fn from(r: WorkspaceRow) -> Self {
        Workspace { id: WorkspaceId(r.id), slug: r.slug, name: r.name, status: r.status }
    }
}

pub(crate) fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::anyhow!(e))
}
```

Add `pub mod workspaces;` to `src/lib.rs`.

- [ ] **Step 2: Write the DB-gated test harness + workspace tests**

`crates/egret-workspace/tests/workspace_db.rs`:
```rust
//! DB-gated integration tests; self-skip without DATABASE_URL.

use egret_auth::password::hash_password;
use egret_auth::users::UserRepo;
use egret_core::{Pagination, UserId};
use egret_metastore::Db;
use egret_workspace::workspaces::WorkspaceRepo;

async fn connect() -> Option<Db> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn uniq(tag: &str) -> String {
    format!("{tag}-{}", uuid::Uuid::new_v4().simple())
}

async fn seed_user(db: &Db) -> UserId {
    let repo = UserRepo::new(db.clone());
    repo.insert_local(
        &format!("{}@test.local", uuid::Uuid::new_v4()),
        "U",
        &hash_password("pw").unwrap(),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn create_get_and_unique_slug() {
    let Some(db) = connect().await else { eprintln!("skip"); return; };
    let owner = seed_user(&db).await;
    let repo = WorkspaceRepo::new(db.clone());
    let slug = uniq("ws");

    let ws = repo.create(&slug, "Sales", owner).await.unwrap();
    assert_eq!(ws.slug, slug);
    assert_eq!(ws.status, "active");

    let got = repo.get_by_slug(&slug).await.unwrap().expect("exists");
    assert_eq!(got.id, ws.id);

    // duplicate slug → Conflict
    let dup = repo.create(&slug, "Other", owner).await;
    assert!(matches!(dup, Err(egret_core::Error::Conflict(_))));
}

#[tokio::test]
async fn invalid_slug_rejected() {
    let Some(db) = connect().await else { eprintln!("skip"); return; };
    let owner = seed_user(&db).await;
    let repo = WorkspaceRepo::new(db);
    let err = repo.create("Bad Slug", "x", owner).await;
    assert!(matches!(err, Err(egret_core::Error::InvalidInput(_))));
}

#[tokio::test]
async fn archive_hides_from_active_lookups() {
    let Some(db) = connect().await else { eprintln!("skip"); return; };
    let owner = seed_user(&db).await;
    let repo = WorkspaceRepo::new(db);
    let ws = repo.create(&uniq("ws"), "Temp", owner).await.unwrap();
    repo.archive(ws.id).await.unwrap();
    let got = repo.get_by_id(ws.id).await.unwrap().unwrap();
    assert_eq!(got.status, "archived");
    let _ = Pagination::default();
}
```

- [ ] **Step 3: Run offline then with DB**

Offline: `cargo test -p egret-workspace` → unit tests pass, DB tests skip.
With DB: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-workspace --test workspace_db` → 3 pass.
Then `cargo clippy -p egret-workspace --all-targets -- -D warnings` and `cargo fmt --all`.

- [ ] **Step 4: Commit**
```bash
git add crates/egret-workspace/src/workspaces.rs crates/egret-workspace/src/lib.rs crates/egret-workspace/tests/workspace_db.rs
git commit -m "feat(workspace): WorkspaceRepo with slug validation and DB tests"
```

---

### Task 3: `MemberRepo`

**Files:** Create `crates/egret-workspace/src/members.rs`; Modify `src/lib.rs`, `tests/workspace_db.rs`.

- [ ] **Step 1: Write the repository**

`crates/egret-workspace/src/members.rs`:
```rust
//! Postgres-backed workspace membership.

use egret_core::{Error, Result, UserId, WorkspaceId};
use egret_metastore::Db;
use uuid::Uuid;

use crate::model::{WorkspaceMember, WORKSPACE_ROLES};
use crate::workspaces::db_err;

#[derive(Clone)]
pub struct MemberRepo {
    db: Db,
}

impl MemberRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub async fn add(
        &self,
        ws: WorkspaceId,
        ptype: &str,
        pid: Uuid,
        role: &str,
        by: UserId,
    ) -> Result<()> {
        if ptype != "user" && ptype != "group" {
            return Err(Error::InvalidInput(format!("bad principal_type: {ptype}")));
        }
        if !WORKSPACE_ROLES.contains(&role) {
            return Err(Error::InvalidInput(format!("unknown workspace role: {role}")));
        }
        sqlx::query(
            "INSERT INTO workspace_members (workspace_id, principal_type, principal_id, role, added_by) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (workspace_id, principal_type, principal_id) DO UPDATE SET role = EXCLUDED.role",
        )
        .bind(ws.0)
        .bind(ptype)
        .bind(pid)
        .bind(role)
        .bind(by.0)
        .execute(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    pub async fn remove(&self, ws: WorkspaceId, ptype: &str, pid: Uuid) -> Result<()> {
        sqlx::query(
            "DELETE FROM workspace_members WHERE workspace_id = $1 AND principal_type = $2 AND principal_id = $3",
        )
        .bind(ws.0)
        .bind(ptype)
        .bind(pid)
        .execute(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    pub async fn list(&self, ws: WorkspaceId) -> Result<Vec<WorkspaceMember>> {
        let rows = sqlx::query_as::<_, MemberRow>(
            "SELECT principal_type, principal_id, role FROM workspace_members WHERE workspace_id = $1",
        )
        .bind(ws.0)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(Into::into).collect())
    }
}

#[derive(sqlx::FromRow)]
struct MemberRow {
    principal_type: String,
    principal_id: Uuid,
    role: String,
}

impl From<MemberRow> for WorkspaceMember {
    fn from(r: MemberRow) -> Self {
        WorkspaceMember { principal_type: r.principal_type, principal_id: r.principal_id, role: r.role }
    }
}
```

Add `pub mod members;` to `src/lib.rs`.

- [ ] **Step 2: Add membership tests**

Append to `crates/egret-workspace/tests/workspace_db.rs`:
```rust
use egret_workspace::members::MemberRepo;

#[tokio::test]
async fn add_list_remove_member() {
    let Some(db) = connect().await else { eprintln!("skip"); return; };
    let owner = seed_user(&db).await;
    let member = seed_user(&db).await;
    let wrepo = WorkspaceRepo::new(db.clone());
    let mrepo = MemberRepo::new(db.clone());
    let ws = wrepo.create(&uniq("ws"), "W", owner).await.unwrap();

    mrepo.add(ws.id, "user", member.0, "ws_editor", owner).await.unwrap();
    let members = mrepo.list(ws.id).await.unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].role, "ws_editor");

    // upsert role
    mrepo.add(ws.id, "user", member.0, "ws_admin", owner).await.unwrap();
    assert_eq!(mrepo.list(ws.id).await.unwrap()[0].role, "ws_admin");

    mrepo.remove(ws.id, "user", member.0).await.unwrap();
    assert!(mrepo.list(ws.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn rejects_unknown_role() {
    let Some(db) = connect().await else { eprintln!("skip"); return; };
    let owner = seed_user(&db).await;
    let wrepo = WorkspaceRepo::new(db.clone());
    let mrepo = MemberRepo::new(db);
    let ws = wrepo.create(&uniq("ws"), "W", owner).await.unwrap();
    let err = mrepo.add(ws.id, "user", owner.0, "superuser", owner).await;
    assert!(matches!(err, Err(egret_core::Error::InvalidInput(_))));
}
```

- [ ] **Step 3: Run with DB + lint**

`DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-workspace --test workspace_db` → all pass. Clippy + fmt clean.

- [ ] **Step 4: Commit**
```bash
git add crates/egret-workspace/src/members.rs crates/egret-workspace/src/lib.rs crates/egret-workspace/tests/workspace_db.rs
git commit -m "feat(workspace): MemberRepo (add/list/remove, role validation)"
```

---

### Task 4: `Authorizer` (union + group resolution)

**Files:** Create `crates/egret-workspace/src/rbac.rs`; Modify `src/lib.rs`, `tests/workspace_db.rs`.

- [ ] **Step 1: Write the Authorizer**

`crates/egret-workspace/src/rbac.rs`:
```rust
//! RBAC: most-permissive union of permissions across a principal's roles (direct + group).

use std::collections::HashSet;

use egret_core::{Error, Principal, Result, UserId, WorkspaceContext, WorkspaceId};
use egret_metastore::Db;

use crate::model::PLATFORM_ROLES;
use crate::workspaces::db_err;

#[derive(Clone)]
pub struct Authorizer {
    db: Db,
}

impl Authorizer {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Union of platform permissions over the user's platform roles (direct + group).
    pub async fn platform_permissions(&self, user_id: UserId) -> Result<HashSet<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT DISTINCT rp.permission \
             FROM platform_role_assignments pra \
             JOIN roles r ON r.id = pra.role_id \
             JOIN role_permissions rp ON rp.role_id = r.id \
             WHERE (pra.principal_type = 'user'  AND pra.principal_id = $1) \
                OR (pra.principal_type = 'group' AND pra.principal_id IN \
                       (SELECT group_id FROM user_groups WHERE user_id = $1))",
        )
        .bind(user_id.0)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(|(p,)| p).collect())
    }

    /// Union of workspace permissions over the principal's workspace roles for `ws` (direct + group).
    pub async fn workspace_permissions(
        &self,
        user_id: UserId,
        ws: WorkspaceId,
    ) -> Result<HashSet<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT DISTINCT rp.permission \
             FROM workspace_members wm \
             JOIN roles r ON r.scope = 'workspace' AND r.name = wm.role \
             JOIN role_permissions rp ON rp.role_id = r.id \
             WHERE wm.workspace_id = $1 \
               AND ( (wm.principal_type = 'user'  AND wm.principal_id = $2) \
                  OR (wm.principal_type = 'group' AND wm.principal_id IN \
                         (SELECT group_id FROM user_groups WHERE user_id = $2)) )",
        )
        .bind(ws.0)
        .bind(user_id.0)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(|(p,)| p).collect())
    }

    /// The principal's workspace role names for `ws` (direct + group), for display precedence.
    pub async fn workspace_roles_for(&self, user_id: UserId, ws: WorkspaceId) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT DISTINCT wm.role FROM workspace_members wm \
             WHERE wm.workspace_id = $1 \
               AND ( (wm.principal_type = 'user'  AND wm.principal_id = $2) \
                  OR (wm.principal_type = 'group' AND wm.principal_id IN \
                         (SELECT group_id FROM user_groups WHERE user_id = $2)) )",
        )
        .bind(ws.0)
        .bind(user_id.0)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(|(r,)| r).collect())
    }

    pub async fn require_platform(&self, p: &Principal, perm: &str) -> Result<()> {
        if self.platform_permissions(p.user_id).await?.contains(perm) {
            Ok(())
        } else {
            Err(Error::Forbidden)
        }
    }

    pub async fn require_workspace(&self, ctx: &WorkspaceContext, perm: &str) -> Result<()> {
        if ctx.is_platform_admin() {
            return Ok(());
        }
        if self
            .workspace_permissions(ctx.principal.user_id, ctx.workspace_id)
            .await?
            .contains(perm)
        {
            Ok(())
        } else {
            Err(Error::Forbidden)
        }
    }

    /// Replace the user's platform role assignments with the supplied (validated) role names.
    pub async fn replace_platform_roles(&self, user_id: UserId, roles: &[String]) -> Result<()> {
        for r in roles {
            if !PLATFORM_ROLES.contains(&r.as_str()) {
                return Err(Error::InvalidInput(format!("unknown platform role: {r}")));
            }
        }
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;
        sqlx::query(
            "DELETE FROM platform_role_assignments WHERE principal_type = 'user' AND principal_id = $1",
        )
        .bind(user_id.0)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        for r in roles {
            sqlx::query(
                "INSERT INTO platform_role_assignments (principal_type, principal_id, role_id) \
                 SELECT 'user', $1, id FROM roles WHERE scope = 'platform' AND name = $2",
            )
            .bind(user_id.0)
            .bind(r)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }
}
```

Add `pub mod rbac;` to `src/lib.rs`.

- [ ] **Step 2: Add Authorizer DB tests (incl. group resolution)**

Append to `crates/egret-workspace/tests/workspace_db.rs`:
```rust
use egret_workspace::rbac::Authorizer;

async fn make_group(db: &Db, name: &str) -> uuid::Uuid {
    sqlx::query_scalar("INSERT INTO groups (name) VALUES ($1) RETURNING id")
        .bind(name)
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn workspace_permission_via_group_is_unioned() {
    let Some(db) = connect().await else { eprintln!("skip"); return; };
    let owner = seed_user(&db).await;
    let user = seed_user(&db).await;
    let wrepo = WorkspaceRepo::new(db.clone());
    let mrepo = MemberRepo::new(db.clone());
    let authz = Authorizer::new(db.clone());
    let ws = wrepo.create(&uniq("ws"), "W", owner).await.unwrap();

    // user is NOT a direct member; their GROUP is ws_admin.
    let gid = make_group(&db, &uniq("grp")).await;
    sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
        .bind(user.0).bind(gid).execute(&db.pool).await.unwrap();
    mrepo.add(ws.id, "group", gid, "ws_admin", owner).await.unwrap();

    let perms = authz.workspace_permissions(user.0_userid(), ws.id).await.unwrap();
    assert!(perms.contains("workspace:manage_members"), "group role must grant the perm");
}

#[tokio::test]
async fn replace_platform_roles_and_check() {
    let Some(db) = connect().await else { eprintln!("skip"); return; };
    let user = seed_user(&db).await;
    let authz = Authorizer::new(db.clone());
    authz.replace_platform_roles(user.0_userid(), &["data_steward".into()]).await.unwrap();
    let perms = authz.platform_permissions(user.0_userid()).await.unwrap();
    assert!(perms.contains("user:read"));
    assert!(!perms.contains("workspace:create")); // data_steward lacks it
    // bad role rejected
    assert!(authz.replace_platform_roles(user.0_userid(), &["root".into()]).await.is_err());
}
```
> **Note:** `seed_user` returns a `UserId`; in these tests pass it directly where `UserId` is expected. The helper expressions `user.0_userid()` above are placeholders for "the `UserId` value" — replace with the `UserId` returned by `seed_user` (the test already binds `user`/`owner` as `UserId`). Concretely use `user` and `owner` directly: `authz.workspace_permissions(user, ws.id)`, `authz.replace_platform_roles(user, ...)`, `authz.platform_permissions(user)`. (`UserId` is `Copy`.)

- [ ] **Step 3: Fix the test to use `UserId` directly**

Edit the two tests from Step 2 so every `*.0_userid()` is replaced by the bound `UserId` variable (`user`), e.g.:
```rust
    let perms = authz.workspace_permissions(user, ws.id).await.unwrap();
```
and
```rust
    authz.replace_platform_roles(user, &["data_steward".into()]).await.unwrap();
    let perms = authz.platform_permissions(user).await.unwrap();
    ...
    assert!(authz.replace_platform_roles(user, &["root".into()]).await.is_err());
```

- [ ] **Step 4: Run with DB + lint**

`DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-workspace --test workspace_db` → all pass (group-union + platform-roles). Clippy + fmt clean.

- [ ] **Step 5: Commit**
```bash
git add crates/egret-workspace/src/rbac.rs crates/egret-workspace/src/lib.rs crates/egret-workspace/tests/workspace_db.rs
git commit -m "feat(workspace): Authorizer with union permissions and group resolution"
```

---

### Task 5: `SessionService` platform-roles group branch

**Files:** Modify `crates/egret-auth/src/session.rs`, `crates/egret-auth/tests/session_db.rs`.

- [ ] **Step 1: Update the platform_roles query**

In `crates/egret-auth/src/session.rs`, replace the body of the `platform_roles` method's SQL to include the group branch:
```rust
    async fn platform_roles(&self, user_id: UserId) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT DISTINCT r.name FROM platform_role_assignments pra \
             JOIN roles r ON r.id = pra.role_id \
             WHERE (pra.principal_type = 'user'  AND pra.principal_id = $1) \
                OR (pra.principal_type = 'group' AND pra.principal_id IN \
                       (SELECT group_id FROM user_groups WHERE user_id = $1))",
        )
        .bind(user_id.0)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(|(n,)| n).collect())
    }
```

- [ ] **Step 2: Add a DB test for group-derived platform roles**

Append to `crates/egret-auth/tests/session_db.rs`:
```rust
#[tokio::test]
async fn platform_role_via_group_appears_in_principal() {
    let Some(db) = connect().await else { eprintln!("skip"); return; };
    let uid = seed_user(&db).await;
    // user's group is assigned platform_admin
    let gid: uuid::Uuid = sqlx::query_scalar("INSERT INTO groups (name) VALUES ($1) RETURNING id")
        .bind(format!("g-{}", uuid::Uuid::new_v4().simple()))
        .fetch_one(&db.pool).await.unwrap();
    sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
        .bind(uid.0).bind(gid).execute(&db.pool).await.unwrap();
    sqlx::query(
        "INSERT INTO platform_role_assignments (principal_type, principal_id, role_id) \
         SELECT 'group', $1, id FROM roles WHERE scope='platform' AND name='platform_admin'",
    ).bind(gid).execute(&db.pool).await.unwrap();

    let svc = SessionService::new(db.clone(), Duration::hours(24), Duration::minutes(60));
    let issued = svc.create(uid, "a", "").await.unwrap();
    let principal = svc.validate(&issued.token).await.unwrap().unwrap();
    assert!(principal.platform_roles.iter().any(|r| r == "platform_admin"));
}
```

- [ ] **Step 3: Run with DB + lint**

`DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-auth --test session_db` → all pass incl. the new group test. Clippy + fmt clean.

- [ ] **Step 4: Commit**
```bash
git add crates/egret-auth/src/session.rs crates/egret-auth/tests/session_db.rs
git commit -m "feat(auth): resolve platform roles via group membership"
```

---

### Task 6: `mapping.rs` + `WsContext` extractor

**Files:** Create `crates/egret-workspace/src/mapping.rs`, `crates/egret-workspace/src/context.rs`; Modify `src/lib.rs`.

- [ ] **Step 1: Write the IdP mapping (pure) with unit tests**

`crates/egret-workspace/src/mapping.rs`:
```rust
//! Map OIDC claims to platform role names per configured rules. Consumed by 1a-3.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct IdpRoleMapping {
    pub claim: String,
    pub value: String,
    pub role: String,
    pub scope: String, // "platform" (workspace mappings handled in a later phase)
}

/// Return the role names whose mapping rule matches the claims. A rule matches when
/// `claims[rule.claim]` equals `rule.value` (string) or contains it (array of strings).
pub fn map_idp_claims_to_roles(
    claims: &serde_json::Value,
    rules: &[IdpRoleMapping],
) -> Vec<String> {
    let mut out = Vec::new();
    for rule in rules {
        let matched = match claims.get(&rule.claim) {
            Some(serde_json::Value::String(s)) => s == &rule.value,
            Some(serde_json::Value::Array(arr)) => {
                arr.iter().any(|v| v.as_str() == Some(rule.value.as_str()))
            }
            _ => false,
        };
        if matched && !out.contains(&rule.role) {
            out.push(rule.role.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(claim: &str, value: &str, role: &str) -> IdpRoleMapping {
        IdpRoleMapping { claim: claim.into(), value: value.into(), role: role.into(), scope: "platform".into() }
    }

    #[test]
    fn matches_array_claim() {
        let claims = serde_json::json!({ "groups": ["a", "data-admins"] });
        let roles = map_idp_claims_to_roles(&claims, &[rule("groups", "data-admins", "platform_admin")]);
        assert_eq!(roles, vec!["platform_admin"]);
    }

    #[test]
    fn matches_string_claim_and_dedupes() {
        let claims = serde_json::json!({ "role": "steward" });
        let rules = [rule("role", "steward", "data_steward"), rule("role", "steward", "data_steward")];
        assert_eq!(map_idp_claims_to_roles(&claims, &rules), vec!["data_steward"]);
    }

    #[test]
    fn no_match_returns_empty() {
        let claims = serde_json::json!({ "groups": ["x"] });
        assert!(map_idp_claims_to_roles(&claims, &[rule("groups", "y", "platform_admin")]).is_empty());
    }
}
```

- [ ] **Step 2: Write the WsContext extractor**

`crates/egret-workspace/src/context.rs`:
```rust
//! `WsContext` — authenticate + resolve the workspace from the `{workspace}` path slug.

use std::collections::HashMap;

use axum::extract::{FromRef, FromRequestParts, Path};
use axum::http::request::Parts;
use egret_auth::principal::{Auth, AuthError};
use egret_auth::session::SessionService;
use egret_core::{Error, WorkspaceContext};

use crate::model::workspace_role_rank;
use crate::rbac::Authorizer;
use crate::workspaces::WorkspaceRepo;

pub struct WsContext(pub WorkspaceContext);

impl<S> FromRequestParts<S> for WsContext
where
    SessionService: FromRef<S>,
    WorkspaceRepo: FromRef<S>,
    Authorizer: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        // 1. Authenticate.
        let Auth(principal) = Auth::from_request_parts(parts, state).await?;

        // 2. Read the {workspace} slug.
        let Path(params): Path<HashMap<String, String>> = Path::from_request_parts(parts, state)
            .await
            .map_err(|_| AuthError(Error::InvalidInput("missing workspace path".into())))?;
        let slug = params
            .get("workspace")
            .ok_or_else(|| AuthError(Error::InvalidInput("missing workspace param".into())))?;

        // 3. Load the active workspace.
        let repo = WorkspaceRepo::from_ref(state);
        let ws = repo
            .get_by_slug(slug)
            .await
            .map_err(AuthError)?
            .filter(|w| w.status == "active")
            .ok_or_else(|| AuthError(Error::NotFound(format!("workspace: {slug}"))))?;

        // 4. Membership: platform_admin bypass, else must hold ≥1 workspace role.
        let is_admin = principal.platform_roles.iter().any(|r| r == "platform_admin");
        let workspace_role = if is_admin {
            "platform_admin".to_string()
        } else {
            let authz = Authorizer::from_ref(state);
            let roles = authz
                .workspace_roles_for(principal.user_id, ws.id)
                .await
                .map_err(AuthError)?;
            roles
                .into_iter()
                .max_by_key(|r| workspace_role_rank(r))
                .ok_or(AuthError(Error::Forbidden))?
        };

        Ok(WsContext(WorkspaceContext {
            workspace_id: ws.id,
            principal,
            workspace_role,
        }))
    }
}
```

Add `pub mod context;` and `pub mod mapping;` to `src/lib.rs`.

- [ ] **Step 3: Build + run mapping unit tests**

Run: `cargo test -p egret-workspace --lib` → mapping tests pass; crate compiles (the extractor is exercised by route tests in Task 9). Clippy + fmt clean.

- [ ] **Step 4: Commit**
```bash
git add crates/egret-workspace/src/mapping.rs crates/egret-workspace/src/context.rs crates/egret-workspace/src/lib.rs
git commit -m "feat(workspace): IdP role mapping and WsContext extractor"
```

---

### Task 7: `WorkspaceState` + admin workspace routes + server wiring

**Files:** Create `crates/egret-workspace/src/routes.rs`; Modify `src/lib.rs`, `crates/egret-auth/src/users.rs`, `crates/egret-server/src/lib.rs`, `crates/egret-server/Cargo.toml`, `crates/egret-bin/src/main.rs`, `tests/workspace_db.rs`.

- [ ] **Step 1: Add `UserRepo::list` (egret-auth)**

In `crates/egret-auth/src/users.rs`, add a method to `impl UserRepo`:
```rust
    pub async fn list(&self, limit: i64, offset: i64) -> Result<Vec<User>> {
        let rows = sqlx::query_as::<_, UserRow>(
            "SELECT id, provider, email, name, password_hash, status FROM users \
             ORDER BY created_at DESC LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(Into::into).collect())
    }
```

- [ ] **Step 2: Write `WorkspaceState` + admin workspace router**

`crates/egret-workspace/src/routes.rs`:
```rust
//! HTTP state + routers for workspace administration and membership.

use axum::extract::{FromRef, Path, Query, State};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use egret_auth::principal::{Auth, AuthError};
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_core::{Error, Pagination, UserId, WorkspaceId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::context::WsContext;
use crate::members::MemberRepo;
use crate::rbac::Authorizer;
use crate::workspaces::WorkspaceRepo;

#[derive(Clone)]
pub struct WorkspaceState {
    pub workspaces: WorkspaceRepo,
    pub members: MemberRepo,
    pub authz: Authorizer,
    pub users: UserRepo,
    pub sessions: SessionService,
}

impl FromRef<WorkspaceState> for WorkspaceRepo {
    fn from_ref(s: &WorkspaceState) -> Self { s.workspaces.clone() }
}
impl FromRef<WorkspaceState> for Authorizer {
    fn from_ref(s: &WorkspaceState) -> Self { s.authz.clone() }
}
impl FromRef<WorkspaceState> for SessionService {
    fn from_ref(s: &WorkspaceState) -> Self { s.sessions.clone() }
}

#[derive(Serialize)]
pub struct WorkspaceDto {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub status: String,
}
impl From<crate::model::Workspace> for WorkspaceDto {
    fn from(w: crate::model::Workspace) -> Self {
        WorkspaceDto { id: w.id.to_string(), slug: w.slug, name: w.name, status: w.status }
    }
}

#[derive(Deserialize)]
pub struct CreateWorkspace { pub slug: String, pub name: String }
#[derive(Deserialize)]
pub struct UpdateWorkspace { pub name: String }
#[derive(Deserialize)]
pub struct PageQuery { pub page: Option<u32>, pub per_page: Option<u32> }

pub fn admin_workspace_router() -> Router<WorkspaceState> {
    Router::new()
        .route("/workspaces", post(create_ws).get(list_ws))
        .route("/workspaces/{id}", get(get_ws).put(update_ws).delete(archive_ws))
}

async fn create_ws(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
    Json(body): Json<CreateWorkspace>,
) -> Result<Json<WorkspaceDto>, AuthError> {
    st.authz.require_platform(&p, "workspace:create").await.map_err(AuthError)?;
    let ws = st.workspaces.create(&body.slug, &body.name, p.user_id).await.map_err(AuthError)?;
    Ok(Json(ws.into()))
}

async fn list_ws(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
    Query(q): Query<PageQuery>,
) -> Result<Json<Vec<WorkspaceDto>>, AuthError> {
    st.authz.require_platform(&p, "workspace:read").await.map_err(AuthError)?;
    let page = Pagination { page: q.page.unwrap_or(1), per_page: q.per_page.unwrap_or(50) };
    let list = st.workspaces.list(page).await.map_err(AuthError)?;
    Ok(Json(list.into_iter().map(Into::into).collect()))
}

async fn get_ws(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<WorkspaceDto>, AuthError> {
    st.authz.require_platform(&p, "workspace:read").await.map_err(AuthError)?;
    let ws = st.workspaces.get_by_id(WorkspaceId(id)).await.map_err(AuthError)?
        .ok_or(AuthError(Error::NotFound("workspace".into())))?;
    Ok(Json(ws.into()))
}

async fn update_ws(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateWorkspace>,
) -> Result<Json<serde_json::Value>, AuthError> {
    st.authz.require_platform(&p, "workspace:update").await.map_err(AuthError)?;
    st.workspaces.update_name(WorkspaceId(id), &body.name).await.map_err(AuthError)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn archive_ws(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, AuthError> {
    st.authz.require_platform(&p, "workspace:archive").await.map_err(AuthError)?;
    st.workspaces.archive(WorkspaceId(id)).await.map_err(AuthError)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}
```

Add `pub mod routes;` to `src/lib.rs`.

- [ ] **Step 3: Wire into `egret-server`**

In `crates/egret-server/Cargo.toml` add `egret-workspace.workspace = true`.

In `crates/egret-server/src/lib.rs`: import `use egret_workspace::routes::{admin_workspace_router, WorkspaceState};`, extend `AppState`:
```rust
#[derive(Clone)]
pub struct AppState {
    pub db: Option<Db>,
    pub auth: Option<AuthState>,
    pub workspace: Option<WorkspaceState>,
}
impl AppState {
    pub fn without_db() -> Self {
        Self { db: None, auth: None, workspace: None }
    }
}
```
And in `build_router`, after mounting the auth router, mount the admin router behind CSRF:
```rust
    if let Some(ws) = state.workspace.clone() {
        let admin = Router::new()
            .nest("/api/admin", admin_workspace_router())
            .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
            .with_state(ws);
        router = router.merge(admin);
    }
```

- [ ] **Step 4: Fix `egret-bin` AppState construction**

In `crates/egret-bin/src/main.rs` `serve`, build a `WorkspaceState` and include it. Add imports `use egret_workspace::routes::WorkspaceState; use egret_workspace::{workspaces::WorkspaceRepo, members::MemberRepo, rbac::Authorizer};` and add `egret-workspace.workspace = true` to `crates/egret-bin/Cargo.toml`. Replace the `AppState { db: Some(db), auth: ... }` construction:
```rust
    let users = UserRepo::new(db.clone());
    let sessions = SessionService::new(
        db.clone(),
        Duration::hours(config.session.absolute_ttl_hours as i64),
        Duration::minutes(config.session.idle_ttl_minutes as i64),
    );
    let auth = AuthState { users: users.clone(), sessions: sessions.clone(), config: config.clone() };
    let workspace = WorkspaceState {
        workspaces: WorkspaceRepo::new(db.clone()),
        members: MemberRepo::new(db.clone()),
        authz: Authorizer::new(db.clone()),
        users,
        sessions,
    };
    let app = build_router(AppState { db: Some(db), auth: Some(auth), workspace: Some(workspace) });
```
> If `AuthState` later gains the rate-limiter field (Task 10), update this construction then.

- [ ] **Step 5: Add a DB handler test for workspace admin routes**

Append to `crates/egret-workspace/tests/workspace_db.rs`:
```rust
use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::routes::AuthState;
use egret_auth::session::SessionService;
use egret_config::Config;
use egret_workspace::routes::{admin_workspace_router, WorkspaceState};

async fn login_cookie(db: &Db, user: UserId, sessions: &SessionService) -> String {
    let issued = sessions.create(user, "a", "").await.unwrap();
    let _ = db;
    format!("egret_session={}", issued.token)
}

async fn ws_state(db: &Db) -> WorkspaceState {
    WorkspaceState {
        workspaces: WorkspaceRepo::new(db.clone()),
        members: MemberRepo::new(db.clone()),
        authz: Authorizer::new(db.clone()),
        users: UserRepo::new(db.clone()),
        sessions: SessionService::new(db.clone(), chrono::Duration::hours(24), chrono::Duration::minutes(60)),
    }
}

#[tokio::test]
async fn create_workspace_requires_platform_permission() {
    let Some(db) = connect().await else { eprintln!("skip"); return; };
    let plain = seed_user(&db).await; // no platform role
    let st = ws_state(&db).await;
    let cookie = login_cookie(&db, plain, &st.sessions).await;
    let app = axum::Router::new().nest("/api/admin", admin_workspace_router()).with_state(st);

    let body = serde_json::to_vec(&serde_json::json!({"slug": uniq("ws"), "name": "X"})).unwrap();
    let resp = app.oneshot(HttpRequest::builder().method("POST").uri("/api/admin/workspaces")
        .header("cookie", cookie).header("content-type","application/json")
        .body(Body::from(body)).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn platform_admin_can_create_workspace() {
    let Some(db) = connect().await else { eprintln!("skip"); return; };
    let admin = seed_user(&db).await;
    let st = ws_state(&db).await;
    st.authz.replace_platform_roles(admin, &["platform_admin".into()]).await.unwrap();
    let cookie = login_cookie(&db, admin, &st.sessions).await;
    let app = axum::Router::new().nest("/api/admin", admin_workspace_router()).with_state(st);

    let body = serde_json::to_vec(&serde_json::json!({"slug": uniq("ws"), "name": "X"})).unwrap();
    let resp = app.oneshot(HttpRequest::builder().method("POST").uri("/api/admin/workspaces")
        .header("cookie", cookie).header("content-type","application/json")
        .body(Body::from(body)).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let _ = AuthState { users: UserRepo::new(db.clone()), sessions: SessionService::new(db.clone(), chrono::Duration::hours(1), chrono::Duration::minutes(1)), config: Config::default() };
}
```
> Add `use tower::ServiceExt;` at the top of the test file if not already present (for `oneshot`).

- [ ] **Step 6: Run offline + with DB + verify whole build**

`cargo build --workspace`; `cargo test --workspace` (offline, DB tests skip); `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-workspace --test workspace_db`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all -- --check`. All green.

- [ ] **Step 7: Commit**
```bash
git add crates/egret-workspace crates/egret-auth/src/users.rs crates/egret-server crates/egret-bin
git commit -m "feat(workspace): admin workspace routes, WorkspaceState, server wiring"
```

---

### Task 8: Admin user routes + platform-role assignment

**Files:** Modify `crates/egret-workspace/src/routes.rs`, `tests/workspace_db.rs`.

- [ ] **Step 1: Add the user admin router**

In `crates/egret-workspace/src/routes.rs`, add handlers and a router:
```rust
#[derive(Serialize)]
pub struct UserDto {
    pub id: String,
    pub email: String,
    pub name: String,
    pub provider: String,
    pub status: String,
}

#[derive(Deserialize)]
pub struct SetPlatformRoles { pub roles: Vec<String> }

pub fn admin_user_router() -> Router<WorkspaceState> {
    Router::new()
        .route("/users", get(list_users))
        .route("/users/{id}", get(get_user))
        .route("/users/{id}/platform-roles", put(set_platform_roles))
}

async fn list_users(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
    Query(q): Query<PageQuery>,
) -> Result<Json<Vec<UserDto>>, AuthError> {
    st.authz.require_platform(&p, "user:read").await.map_err(AuthError)?;
    let page = Pagination { page: q.page.unwrap_or(1), per_page: q.per_page.unwrap_or(50) };
    let users = st.users.list(page.limit() as i64, page.offset() as i64).await.map_err(AuthError)?;
    Ok(Json(users.into_iter().map(|u| UserDto {
        id: u.id.to_string(), email: u.email, name: u.name, provider: u.provider, status: u.status,
    }).collect()))
}

async fn get_user(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<UserDto>, AuthError> {
    st.authz.require_platform(&p, "user:read").await.map_err(AuthError)?;
    let u = st.users.find_by_id(UserId(id)).await.map_err(AuthError)?
        .ok_or(AuthError(Error::NotFound("user".into())))?;
    Ok(Json(UserDto { id: u.id.to_string(), email: u.email, name: u.name, provider: u.provider, status: u.status }))
}

async fn set_platform_roles(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<SetPlatformRoles>,
) -> Result<Json<serde_json::Value>, AuthError> {
    st.authz.require_platform(&p, "user:assign_role").await.map_err(AuthError)?;
    st.authz.replace_platform_roles(UserId(id), &body.roles).await.map_err(AuthError)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}
```

- [ ] **Step 2: Mount the user router in `egret-server`**

In `crates/egret-server/src/lib.rs`, update the admin nesting to include both routers:
```rust
    if let Some(ws) = state.workspace.clone() {
        let admin = Router::new()
            .nest("/api/admin", admin_workspace_router().merge(admin_user_router()))
            .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
            .with_state(ws);
        router = router.merge(admin);
    }
```
Add `admin_user_router` to the `use egret_workspace::routes::{...}` import.

- [ ] **Step 3: Add a DB handler test**

Append to `crates/egret-workspace/tests/workspace_db.rs`:
```rust
use egret_workspace::routes::admin_user_router;

#[tokio::test]
async fn assign_platform_role_then_user_gains_access() {
    let Some(db) = connect().await else { eprintln!("skip"); return; };
    let admin = seed_user(&db).await;
    let target = seed_user(&db).await;
    let st = ws_state(&db).await;
    st.authz.replace_platform_roles(admin, &["platform_admin".into()]).await.unwrap();
    let cookie = login_cookie(&db, admin, &st.sessions).await;
    let app = axum::Router::new().nest("/api/admin", admin_user_router()).with_state(st);

    let body = serde_json::to_vec(&serde_json::json!({"roles": ["data_steward"]})).unwrap();
    let resp = app.oneshot(HttpRequest::builder().method("PUT")
        .uri(format!("/api/admin/users/{}/platform-roles", target.0))
        .header("cookie", cookie).header("content-type","application/json")
        .body(Body::from(body)).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
```

- [ ] **Step 4: Run with DB + lint + commit**

`DATABASE_URL=... cargo test -p egret-workspace --test workspace_db` → pass. Clippy/fmt clean.
```bash
git add crates/egret-workspace/src/routes.rs crates/egret-server/src/lib.rs crates/egret-workspace/tests/workspace_db.rs
git commit -m "feat(workspace): admin user routes and platform-role assignment"
```

---

### Task 9: Workspace member routes (`WsContext` enforced)

**Files:** Modify `crates/egret-workspace/src/routes.rs`, `crates/egret-server/src/lib.rs`, `tests/workspace_db.rs`.

- [ ] **Step 1: Add the member router**

In `crates/egret-workspace/src/routes.rs`:
```rust
#[derive(Deserialize)]
pub struct AddMember { pub principal_type: String, pub principal_id: Uuid, pub role: String }

#[derive(Serialize)]
pub struct MemberDto { pub principal_type: String, pub principal_id: String, pub role: String }

pub fn ws_member_router() -> Router<WorkspaceState> {
    Router::new()
        .route("/{workspace}/members", get(list_members).post(add_member))
        .route("/{workspace}/members/{ptype}/{pid}", delete(remove_member))
}

async fn list_members(
    State(st): State<WorkspaceState>,
    WsContext(ctx): WsContext,
) -> Result<Json<Vec<MemberDto>>, AuthError> {
    st.authz.require_workspace(&ctx, "workspace:manage_members").await.map_err(AuthError)?;
    let members = st.members.list(ctx.workspace_id).await.map_err(AuthError)?;
    Ok(Json(members.into_iter().map(|m| MemberDto {
        principal_type: m.principal_type, principal_id: m.principal_id.to_string(), role: m.role,
    }).collect()))
}

async fn add_member(
    State(st): State<WorkspaceState>,
    WsContext(ctx): WsContext,
    Json(body): Json<AddMember>,
) -> Result<Json<serde_json::Value>, AuthError> {
    st.authz.require_workspace(&ctx, "workspace:manage_members").await.map_err(AuthError)?;
    st.members.add(ctx.workspace_id, &body.principal_type, body.principal_id, &body.role, ctx.principal.user_id)
        .await.map_err(AuthError)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn remove_member(
    State(st): State<WorkspaceState>,
    WsContext(ctx): WsContext,
    Path((_ws, ptype, pid)): Path<(String, String, Uuid)>,
) -> Result<Json<serde_json::Value>, AuthError> {
    st.authz.require_workspace(&ctx, "workspace:manage_members").await.map_err(AuthError)?;
    st.members.remove(ctx.workspace_id, &ptype, pid).await.map_err(AuthError)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}
```
> The `remove_member` path captures three params: `{workspace}`, `{ptype}`, `{pid}`. `WsContext` reads `{workspace}` from the same params (it only needs that one key), so both extractors coexist.

- [ ] **Step 2: Mount the member router under `/api/ws`**

In `crates/egret-server/src/lib.rs`, add to the workspace block:
```rust
    if let Some(ws) = state.workspace.clone() {
        let admin = Router::new()
            .nest("/api/admin", admin_workspace_router().merge(admin_user_router()))
            .nest("/api/ws", ws_member_router())
            .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
            .with_state(ws);
        router = router.merge(admin);
    }
```
Add `ws_member_router` to the import.

- [ ] **Step 3: Add DB handler tests (non-member 403, ws_admin 200, archived 404)**

Append to `crates/egret-workspace/tests/workspace_db.rs`:
```rust
use egret_workspace::routes::ws_member_router;

#[tokio::test]
async fn member_routes_enforce_membership() {
    let Some(db) = connect().await else { eprintln!("skip"); return; };
    let owner = seed_user(&db).await;
    let admin_user = seed_user(&db).await;
    let outsider = seed_user(&db).await;
    let st = ws_state(&db).await;
    let ws = st.workspaces.create(&uniq("ws"), "W", owner).await.unwrap();
    st.members.add(ws.id, "user", admin_user.0, "ws_admin", owner).await.unwrap();
    let app = axum::Router::new().nest("/api/ws", ws_member_router()).with_state(st.clone());

    // ws_admin can list members
    let cookie = login_cookie(&db, admin_user, &st.sessions).await;
    let resp = app.clone().oneshot(HttpRequest::builder()
        .uri(format!("/api/ws/{}/members", ws.slug))
        .header("cookie", cookie).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // outsider gets 403
    let cookie2 = login_cookie(&db, outsider, &st.sessions).await;
    let resp = app.oneshot(HttpRequest::builder()
        .uri(format!("/api/ws/{}/members", ws.slug))
        .header("cookie", cookie2).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn archived_workspace_is_404() {
    let Some(db) = connect().await else { eprintln!("skip"); return; };
    let owner = seed_user(&db).await;
    let st = ws_state(&db).await;
    let ws = st.workspaces.create(&uniq("ws"), "W", owner).await.unwrap();
    st.members.add(ws.id, "user", owner.0, "ws_admin", owner).await.unwrap();
    st.workspaces.archive(ws.id).await.unwrap();
    let app = axum::Router::new().nest("/api/ws", ws_member_router()).with_state(st.clone());
    let cookie = login_cookie(&db, owner, &st.sessions).await;
    let resp = app.oneshot(HttpRequest::builder()
        .uri(format!("/api/ws/{}/members", ws.slug))
        .header("cookie", cookie).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
```

- [ ] **Step 4: Run with DB + lint + commit**

`DATABASE_URL=... cargo test -p egret-workspace --test workspace_db` → pass. `cargo test --workspace` offline green. Clippy/fmt clean.
```bash
git add crates/egret-workspace/src/routes.rs crates/egret-server/src/lib.rs crates/egret-workspace/tests/workspace_db.rs
git commit -m "feat(workspace): workspace member routes enforced by WsContext"
```

---

### Task 10: Login rate-limit + dummy-verify + config

**Files:** Create `crates/egret-auth/src/ratelimit.rs`; Modify `crates/egret-auth/src/{lib.rs,routes.rs}`, `crates/egret-config/src/lib.rs`, `crates/egret-bin/src/main.rs`, `crates/egret-auth/tests/session_db.rs`.

- [ ] **Step 1: Write the rate limiter with unit tests**

`crates/egret-auth/src/ratelimit.rs`:
```rust
//! In-memory per-process login rate limiter (IP+email key). Resets on restart.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Entry {
    attempts: Vec<Instant>,
    locked_until: Option<Instant>,
}

#[derive(Clone)]
pub struct RateLimiter {
    inner: Arc<Mutex<HashMap<String, Entry>>>,
    max: u32,
    window: Duration,
    lockout: Duration,
}

impl RateLimiter {
    pub fn new(max: u32, window_secs: u64, lockout_secs: u64) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            max,
            window: Duration::from_secs(window_secs),
            lockout: Duration::from_secs(lockout_secs),
        }
    }

    /// Record an attempt at `now`. Returns true if allowed, false if rate-limited.
    pub fn check_at(&self, key: &str, now: Instant) -> bool {
        let mut map = self.inner.lock().unwrap();
        let e = map.entry(key.to_string()).or_default();
        if let Some(until) = e.locked_until {
            if now < until {
                return false;
            }
            e.locked_until = None;
            e.attempts.clear();
        }
        e.attempts.retain(|t| now.duration_since(*t) < self.window);
        if e.attempts.len() as u32 >= self.max {
            e.locked_until = Some(now + self.lockout);
            return false;
        }
        e.attempts.push(now);
        true
    }

    pub fn check(&self, key: &str) -> bool {
        self.check_at(key, Instant::now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_after_max_attempts_then_unlocks() {
        let rl = RateLimiter::new(2, 300, 300);
        let t0 = Instant::now();
        assert!(rl.check_at("k", t0)); // 1
        assert!(rl.check_at("k", t0)); // 2
        assert!(!rl.check_at("k", t0)); // 3 → blocked, locks
        // still locked within lockout
        assert!(!rl.check_at("k", t0 + Duration::from_secs(100)));
        // after lockout expires → allowed again
        assert!(rl.check_at("k", t0 + Duration::from_secs(601)));
    }

    #[test]
    fn separate_keys_are_independent() {
        let rl = RateLimiter::new(1, 300, 300);
        let t0 = Instant::now();
        assert!(rl.check_at("a", t0));
        assert!(rl.check_at("b", t0));
        assert!(!rl.check_at("a", t0));
    }
}
```

Add `pub mod ratelimit;` to `crates/egret-auth/src/lib.rs`.

- [ ] **Step 2: Add the rate-limit config**

In `crates/egret-config/src/lib.rs` add:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    pub login_max_attempts: u32,
    pub login_window_secs: u64,
    pub login_lockout_secs: u64,
}
```
Add `pub rate_limit: RateLimitConfig,` to `Config` and to its `Default`:
```rust
            rate_limit: RateLimitConfig {
                login_max_attempts: 5,
                login_window_secs: 300,
                login_lockout_secs: 300,
            },
```
Append a test to the config `tests` module:
```rust
    #[test]
    fn rate_limit_defaults_present() {
        figment::Jail::expect_with(|_j| {
            let cfg: Config = Config::figment().extract()?;
            assert_eq!(cfg.rate_limit.login_max_attempts, 5);
            Ok(())
        });
    }
```

- [ ] **Step 3: Wire rate-limit + dummy-verify into login**

In `crates/egret-auth/src/routes.rs`, add a `rate_limiter` to `AuthState`:
```rust
use crate::ratelimit::RateLimiter;

#[derive(Clone)]
pub struct AuthState {
    pub users: UserRepo,
    pub sessions: SessionService,
    pub config: Config,
    pub rate_limiter: RateLimiter,
}
```
Replace the `login` handler body's start to add rate-limiting (by `ip:email` from `X-Forwarded-For`) and a dummy verify on the not-found path:
```rust
use std::sync::OnceLock;
static DUMMY_HASH: OnceLock<String> = OnceLock::new();

async fn login(
    State(st): State<AuthState>,
    headers: axum::http::HeaderMap,
    jar: CookieJar,
    Json(body): Json<LoginRequest>,
) -> Result<impl IntoResponse, AuthError> {
    let ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .unwrap_or("")
        .trim()
        .to_string();
    let rl_key = format!("{ip}:{}", body.email.to_lowercase());
    if !st.rate_limiter.check(&rl_key) {
        return Err(AuthError(Error::TooManyRequests));
    }

    let dummy = DUMMY_HASH.get_or_init(|| crate::password::hash_password("not-a-real-password").unwrap());
    let user = st.users.find_by_email(&body.email).await.map_err(AuthError)?;
    let user = match user {
        Some(u) if u.status == "active" && u.password_hash.is_some() => u,
        _ => {
            // Equalize timing for unknown/disabled/no-password users.
            let _ = crate::password::verify_password(&body.password, dummy);
            return Err(AuthError(Error::Unauthorized));
        }
    };
    let hash = user.password_hash.as_deref().unwrap();
    if !verify_password(&body.password, hash).map_err(AuthError)? {
        return Err(AuthError(Error::Unauthorized));
    }

    let issued = st.sessions.create(user.id, "", &ip).await.map_err(AuthError)?;
    st.users.touch_last_login(user.id).await.map_err(AuthError)?;

    let secure = st.config.session.secure;
    let max_age = st.config.session.absolute_ttl_hours as i64 * 3600;
    let csrf = generate_token();
    let jar = jar
        .add(session_cookie(issued.token, secure, max_age))
        .add(csrf_cookie(csrf, secure, max_age));
    Ok((jar, Json(MeResponse {
        user_id: user.id.to_string(),
        email: user.email,
        platform_roles: vec![],
    })))
}
```

- [ ] **Step 4: Add the `TooManyRequests` error variant + 429 mapping**

In `crates/egret-core/src/error.rs`, add a variant:
```rust
    #[error("too many requests")]
    TooManyRequests,
```
In `crates/egret-auth/src/principal.rs`, add the mapping arm in `AuthError::into_response`:
```rust
            Error::TooManyRequests => (StatusCode::TOO_MANY_REQUESTS, "too many requests"),
```

- [ ] **Step 5: Update all `AuthState` constructions**

`AuthState` now needs `rate_limiter`. Update:
- `crates/egret-bin/src/main.rs` `serve`: build `RateLimiter::new(config.rate_limit.login_max_attempts, config.rate_limit.login_window_secs, config.rate_limit.login_lockout_secs)` and include it in `AuthState { ..., rate_limiter }`. Add `use egret_auth::ratelimit::RateLimiter;`.
- `crates/egret-auth/tests/session_db.rs`: every `AuthState { users, sessions, config }` becomes `AuthState { users, sessions, config, rate_limiter: egret_auth::ratelimit::RateLimiter::new(5, 300, 300) }`.

- [ ] **Step 6: Add a rate-limit DB test**

Append to `crates/egret-auth/tests/session_db.rs` (a generous limiter that still blocks after N):
```rust
#[tokio::test]
async fn login_is_rate_limited() {
    let Some(db) = connect().await else { eprintln!("skip"); return; };
    let email = unique_email("rl");
    UserRepo::new(db.clone()).insert_local(&email, "R", &hash_password("pw").unwrap()).await.unwrap();
    let st = AuthState {
        users: UserRepo::new(db.clone()),
        sessions: SessionService::new(db.clone(), Duration::hours(24), Duration::minutes(60)),
        config: Config::default(),
        rate_limiter: egret_auth::ratelimit::RateLimiter::new(2, 300, 300),
    };
    let app = axum::Router::new().nest("/api/auth", auth_router()).with_state(st);
    let bad = serde_json::to_vec(&serde_json::json!({"email": email, "password": "wrong"})).unwrap();
    let mk = || HttpRequest::builder().method("POST").uri("/api/auth/login")
        .header("content-type","application/json").body(Body::from(bad.clone())).unwrap();

    let s1 = app.clone().oneshot(mk()).await.unwrap().status(); // 401
    let s2 = app.clone().oneshot(mk()).await.unwrap().status(); // 401
    let s3 = app.oneshot(mk()).await.unwrap().status();         // 429 (limiter max=2)
    assert_eq!(s1, StatusCode::UNAUTHORIZED);
    assert_eq!(s2, StatusCode::UNAUTHORIZED);
    assert_eq!(s3, StatusCode::TOO_MANY_REQUESTS);
}
```

- [ ] **Step 7: Run + lint + commit**

`cargo test -p egret-auth -p egret-config --lib` offline green; `DATABASE_URL=... cargo test -p egret-auth --test session_db` → all pass incl. rate-limit. `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all`.
```bash
git add crates/egret-auth crates/egret-core/src/error.rs crates/egret-config/src/lib.rs crates/egret-bin/src/main.rs
git commit -m "feat(auth): login rate-limiting and dummy-verify (anti-timing)"
```

---

### Task 11: `egret init --force` + dead-code cleanup

**Files:** Modify `crates/egret-bin/src/cli.rs`, `crates/egret-bin/src/main.rs`, `crates/egret-auth/src/model.rs`.

- [ ] **Step 1: Add `--force` to the CLI**

In `crates/egret-bin/src/cli.rs`, change the `Init` variant and update the test:
```rust
#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum Command {
    Serve,
    Migrate,
    Init {
        /// Create an additional platform_admin even if one already exists.
        #[arg(long)]
        force: bool,
    },
}
```
Update the parsing test:
```rust
    #[test]
    fn parses_each_subcommand() {
        assert_eq!(Cli::parse_from(["egret", "serve"]).command, Command::Serve);
        assert_eq!(Cli::parse_from(["egret", "migrate"]).command, Command::Migrate);
        assert_eq!(Cli::parse_from(["egret", "init"]).command, Command::Init { force: false });
        assert_eq!(Cli::parse_from(["egret", "init", "--force"]).command, Command::Init { force: true });
    }
```

- [ ] **Step 2: Honor `--force` in `init`**

In `crates/egret-bin/src/main.rs`, change the match arm and `init` signature:
```rust
        Command::Init { force } => init(config, force).await,
```
```rust
async fn init(config: Config, force: bool) -> anyhow::Result<()> {
    let db = Db::connect(&config.database.url, config.database.max_connections).await
        .context("connecting to database")?;
    db.migrate().await.context("running migrations")?;

    if !force {
        let existing: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM platform_role_assignments pra \
             JOIN roles r ON r.id = pra.role_id WHERE r.scope='platform' AND r.name='platform_admin'",
        ).fetch_one(&db.pool).await?;
        if existing > 0 {
            tracing::warn!("a platform_admin already exists; use --force to add another");
            return Ok(());
        }
    }
    // ... unchanged: read env, hash, insert user, assign platform_admin role, print creds ...
```
(Keep the rest of `init` exactly as it was — only the guard is now conditional on `!force`.)

- [ ] **Step 3: Remove the dead `Session` struct**

In `crates/egret-auth/src/model.rs`, delete the `Session` struct (it is unused). Keep `Provider` and `User`. Verify nothing references `Session`:
Run: `grep -rn "model::Session\|use .*model::.*Session\| Session " crates/egret-auth/src` → no matches (other than removal).

- [ ] **Step 4: Build + test + lint**

`cargo build --workspace`; `cargo test -p egret-bin -p egret-auth --lib` → pass; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all -- --check`. Green.

- [ ] **Step 5: Commit**
```bash
git add crates/egret-bin crates/egret-auth/src/model.rs
git commit -m "feat(bin): egret init --force; remove unused Session type"
```

---

### Task 12: Final verification + e2e smoke

**Files:** none (verification only).

- [ ] **Step 1: Full offline gate**

Run:
```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```
Expected: build OK; all tests pass (DB-gated skip offline); clippy clean; fmt clean.

- [ ] **Step 2: Full DB-gated run**

Run:
```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret
cargo test -p egret-auth --test session_db
cargo test -p egret-workspace --test workspace_db
```
Expected: every integration test PASSES against real Postgres.

- [ ] **Step 3: e2e smoke (admin workspace flow)**

Run (uses the existing Postgres, not docker compose):
```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret
EGRET_ADMIN_EMAIL=admin@egret.local EGRET_ADMIN_PASSWORD=devpass cargo run -p egret-bin -- init --force
cargo run -p egret-bin -- serve & sleep 3
# login, capture cookies
COOKIES=$(curl -s -i -X POST localhost:8080/api/auth/login -H 'content-type: application/json' \
  -d '{"email":"admin@egret.local","password":"devpass"}' | grep -i '^set-cookie' | sed 's/set-cookie: //I' | cut -d';' -f1 | tr '\n' ';')
CSRF=$(echo "$COOKIES" | tr ';' '\n' | grep egret_csrf | cut -d= -f2)
# create a workspace (CSRF required on POST)
curl -s -i -X POST localhost:8080/api/admin/workspaces -H 'content-type: application/json' \
  -H "cookie: $COOKIES" -H "x-egret-csrf: $CSRF" -d '{"slug":"sales","name":"Sales"}' | grep -i 'HTTP/'
kill %1
```
Expected: login 200; workspace create returns HTTP 200 (admin has `workspace:create`). If a leftover `sales` workspace exists from a prior run, expect 409 instead — acceptable, note it.

- [ ] **Step 4: Commit (if any verification-only fixes were needed)**

If Steps 1–3 surfaced a fix, commit it:
```bash
git add -A
git commit -m "fix(workspace): address final verification findings"
```
Otherwise nothing to commit.

---

## Self-Review

**Spec coverage (Phase 1a-2 spec):**
- §3 `egret-workspace` crate + dependency on `egret-auth` → Tasks 1–9. ✅
- §4 Authorizer union + group resolution + fixed catalog → Task 4. ✅
- §4 platform roles incl. groups (SessionService change) → Task 5. ✅
- §5 WorkspaceRepo + admin workspace routes → Tasks 2, 7. ✅
- §6 MemberRepo + member routes + admin user routes → Tasks 3, 8, 9. ✅
- §7 WsContext extractor (slug, 404 archived, 403 non-member, display role) → Tasks 6, 9. ✅
- §8 `map_idp_claims_to_roles` → Task 6. ✅
- §9 rate-limit (in-memory) + dummy-verify + `init --force` + remove `Session` → Tasks 10, 11. ✅
- §10 index migration `0004` → Task 1. ✅
- §11 `[rate_limit]` config → Task 10. ✅
- §12 `WorkspaceState` + server wiring → Tasks 7, 8, 9. ✅
- §13 error mapping incl. 429 → Task 10. ✅
- §14 unit + DB-integration (incl. group-union) + handler tests → every task. ✅

**Placeholder scan:** No "TBD"/"add validation"/"similar to Task N". Task 4 Step 2 deliberately uses `*.0_userid()` placeholders that Step 3 immediately replaces with the real `UserId` variable — flagged inline and fixed within the same task (not a dangling placeholder). Task 11 Step 2 says "keep the rest of `init` unchanged" referencing concrete existing code from Phase 1a-1, not an unwritten block.

**Type consistency:**
- `WorkspaceRepo::new/create/list/get_by_slug/get_by_id/update_name/archive` — consistent Tasks 2, 7. ✅
- `MemberRepo::new/add/remove/list` — consistent Tasks 3, 9. ✅
- `Authorizer::new/platform_permissions/workspace_permissions/workspace_roles_for/require_platform/require_workspace/replace_platform_roles` — consistent Tasks 4, 6, 7, 8, 9. ✅
- `WorkspaceState { workspaces, members, authz, users, sessions }` + `FromRef` impls (WorkspaceRepo/Authorizer/SessionService) — consistent Tasks 7–9. ✅
- `AuthState` gains `rate_limiter` — all constructions updated in Task 10 Step 5. ✅
- `Error::TooManyRequests` defined (Task 10 Step 4) before use. ✅
- `WsContext(WorkspaceContext)` uses `egret_core::WorkspaceContext` (fields `workspace_id`, `principal`, `workspace_role`) and `is_platform_admin()` — matches the Phase 0a definition. ✅

**Open follow-ups:** Phase 1a-3 (OIDC/SSO) consumes `map_idp_claims_to_roles` and `model::Provider`; group memberships get populated there.
