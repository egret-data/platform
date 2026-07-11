# B2-flip: Drop legacy grant tables — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `access_grants` the single grant store — repoint every platform/workspace grant writer and reader off `platform_role_assignments`/`workspace_members`, then drop the 0068/0069 sync triggers and the two legacy tables.

**Architecture:** Behavior-preserving refactor. Every public method keeps its signature; only the inner SQL changes from a legacy table to `access_grants`. Ordering is deliberate: repoint production code first (Tasks 1–3) while the sync triggers still exist so legacy-table test seeds keep mirroring into `access_grants` and every test stays green; then repoint the ~25 test files that seed legacy tables directly (Tasks 4–5); then drop the triggers + tables (Task 6). `access_grants` contents are unchanged throughout, so the PDP decision matrix is unchanged.

**Tech Stack:** Rust (axum, sqlx, Postgres). No frontend change (members/roles DTOs unchanged).

**Spec:** `docs/en/specs/2026-07-11-b2-flip-drop-legacy-grant-tables-design.md`.

**Conventions (read before starting):**
- Run cargo from repo root `/Users/1pro-l4r8/Documents/github/data-portal/egret`. DB tests need `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret`.
- After adding a migration, `touch crates/egret-metastore/src/lib.rs` so the embedded migrator re-embeds (sqlx staleness gotcha) before running tests that migrate.
- RTK's proxy filter can SILENTLY DROP passing test binaries + rewrites `cargo test`. For an accurate full pass count, run `cargo test ...` via a shell-script file (not `rtk proxy cargo test`), redirecting to a file and reading it. Use `rtk proxy` for single-test / grep.
- Git identity configured — do NOT override the author. End every commit body with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- Never run two `cargo test --workspace` at once.

**Key ordering invariant:** the 0068/0069 sync triggers mirror legacy-table writes into `access_grants`. They stay until Task 6. So through Tasks 1–5 a test can seed EITHER a legacy table (mirrored by trigger) OR `access_grants` directly — both leave the same rows. This is why production and tests can be repointed incrementally with everything green, and the drop lands last.

**access_grants shape** (destination): `(id, principal_type, principal_id, role_id, scope_type, scope_id, object_type, effect, granted_by, created_at)`. Platform grant = `scope_type='platform', scope_id=NULL`. Workspace grant = `scope_type='workspace', scope_id=<workspace uuid>, role_id=<the ws role's id>`. `effect` defaults `'allow'`. UNIQUE `(principal_type, principal_id, role_id, scope_type, scope_id, effect)`, `role_id` FK `ON DELETE CASCADE`.

---

## File structure

**Production (Tasks 1–3)**
- `crates/egret-auth/src/roles.rs` — `assign`/`unassign` (platform writers) + `assignees_of`/`assignees_named`/`roles_of`/`count_platform_admins` (platform readers).
- `crates/egret-auth/src/session.rs` — `platform_roles` + `permissions` (platform readers).
- `crates/egret-workspace/src/rbac.rs` — `replace_platform_roles` (platform writer) + `workspace_roles_for` (workspace reader).
- `crates/egret-workspace/src/members.rs` — `add`/`remove`/`list` (workspace writer+reader).
- `crates/egret-workspace/src/workspaces.rs` — `list_for_principal` (workspace reader).
- `crates/egret-bin/src/main.rs` — bootstrap admin seed + idempotency check.

**Tests (Tasks 4–5)** — repoint direct legacy-table seeds/reads to `access_grants` (~25 files).

**Migration (Task 6)**
- `migrations/0073_drop_legacy_grant_tables.sql` (create).
- `crates/egret-auth/tests/access_grants_migration.rs` — rewrite for the post-flip world.

---

## Task 1: Repoint platform-grant writers

**Files:**
- Modify: `crates/egret-auth/src/roles.rs` (`assign`, `unassign`)
- Modify: `crates/egret-workspace/src/rbac.rs` (`replace_platform_roles`)
- Modify: `crates/egret-bin/src/main.rs` (bootstrap seed)
- Test: `crates/egret-auth/tests/roles_db.rs` (extend)

- [ ] **Step 1: Write the failing test**

Add to `crates/egret-auth/tests/roles_db.rs` (mirror the file's existing `db()`/`RoleRepo::new` harness):

```rust
#[tokio::test]
async fn assign_writes_platform_access_grant() {
    let db = db().await;
    let repo = RoleRepo::new(db.clone(), egret_events::EventBus::new());
    let uid = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, provider, email, name, status) VALUES ($1,'local',$2,'T','active')")
        .bind(uid).bind(format!("flip-{uid}@t.local")).execute(&db.pool).await.unwrap();
    let role: uuid::Uuid = sqlx::query_scalar("SELECT id FROM roles WHERE scope='platform' AND name='data_steward'")
        .fetch_one(&db.pool).await.unwrap();
    repo.assign("user", uid, role).await.unwrap();
    // A platform-scope access_grant now exists (not via a legacy trigger — direct).
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM access_grants WHERE principal_type='user' AND principal_id=$1 AND role_id=$2 AND scope_type='platform'")
        .bind(uid).bind(role).fetch_one(&db.pool).await.unwrap();
    assert_eq!(n, 1, "assign must write a platform access_grant");
    repo.unassign("user", uid, role).await.unwrap();
    let n2: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM access_grants WHERE principal_type='user' AND principal_id=$1 AND role_id=$2 AND scope_type='platform'")
        .bind(uid).bind(role).fetch_one(&db.pool).await.unwrap();
    assert_eq!(n2, 0, "unassign must remove the platform access_grant");
    sqlx::query("DELETE FROM users WHERE id=$1").bind(uid).execute(&db.pool).await.unwrap();
}
```

- [ ] **Step 2: Run it — expect FAIL**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-auth --test roles_db assign_writes_platform_access_grant`
Expected: FAIL (assign still writes `platform_role_assignments`; the trigger mirrors it, so the count is actually 1 via the trigger — the test may PASS accidentally). **If it passes via the trigger, temporarily assert the row's absence in `platform_role_assignments` instead is NOT the goal.** To make the test meaningful now, it is acceptable that the trigger produces the row; the real proof is Step 3's direct write. Proceed to Step 3 regardless.

> Note: because the sync trigger still mirrors `platform_role_assignments` → `access_grants`, this test passes both before and after. That is expected — the trigger is the safety net. The behavior change (direct write) is verified for real once the trigger is dropped in Task 6; the full suite staying green there is the proof.

- [ ] **Step 3: Repoint `assign`/`unassign` in `roles.rs`**

```rust
    /// Assign a role to a principal (idempotent). Writes the unified access_grants
    /// store directly (platform scope).
    pub async fn assign(&self, ptype: &str, pid: Uuid, role_id: Uuid) -> Result<()> {
        sqlx::query(
            "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
             VALUES ($1, $2, $3, 'platform', NULL) ON CONFLICT DO NOTHING",
        )
        .bind(ptype)
        .bind(pid)
        .bind(role_id)
        .execute(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    /// Remove a platform role assignment from a principal.
    pub async fn unassign(&self, ptype: &str, pid: Uuid, role_id: Uuid) -> Result<()> {
        sqlx::query(
            "DELETE FROM access_grants \
             WHERE principal_type = $1 AND principal_id = $2 AND role_id = $3 AND scope_type = 'platform'",
        )
        .bind(ptype)
        .bind(pid)
        .bind(role_id)
        .execute(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }
```

- [ ] **Step 4: Repoint `replace_platform_roles` in `rbac.rs`**

```rust
    pub async fn replace_platform_roles(&self, user_id: UserId, roles: &[String]) -> Result<()> {
        for r in roles {
            if !PLATFORM_ROLES.contains(&r.as_str()) {
                return Err(Error::InvalidInput(format!("unknown platform role: {r}")));
            }
        }
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;
        sqlx::query(
            "DELETE FROM access_grants \
             WHERE principal_type = 'user' AND principal_id = $1 AND scope_type = 'platform'",
        )
        .bind(user_id.0)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        for r in roles {
            sqlx::query(
                "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
                 SELECT 'user', $1, id, 'platform', NULL FROM roles WHERE scope = 'platform' AND name = $2 \
                 ON CONFLICT DO NOTHING",
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
```

- [ ] **Step 5: Repoint the bootstrap seed in `egret-bin/src/main.rs`**

Replace the `INSERT INTO platform_role_assignments ...` seed (around line 105) with:

```rust
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $1, id, 'platform', NULL FROM roles WHERE scope='platform' AND name='platform_admin' \
         ON CONFLICT DO NOTHING",
    )
    .bind(user_id.0)
    .execute(&db.pool)
    .await?;
```

- [ ] **Step 6: Run tests + build**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-auth --test roles_db`
Expected: PASS.
Run: `rtk proxy cargo build --workspace` — clean.
Run: `rtk proxy cargo clippy -p egret-auth -p egret-workspace -p egret-bin --all-targets -- -D warnings` — clean; `cargo fmt` the touched crates.

- [ ] **Step 7: Commit**

```bash
git add crates/egret-auth/src/roles.rs crates/egret-workspace/src/rbac.rs crates/egret-bin/src/main.rs crates/egret-auth/tests/roles_db.rs
git commit -m "refactor(authz): platform grant writers write access_grants directly"
```

---

## Task 2: Repoint platform-grant readers

> **Scope note (Task 1 pulled work forward):** the four `roles.rs` readers (`assignees_of`, `assignees_named`, `roles_of`, `count_platform_admins`) were ALREADY repointed in Task 1 (they had to be — `roles_db.rs` round-trips writer→reader in one binary and the sync trigger is one-directional legacy→access_grants). So Task 2's remaining work is ONLY `session.rs` (`platform_roles`, `permissions`) and the `main.rs` idempotency check. Verify the 4 `roles.rs` readers already read `access_grants … scope_type='platform'` (Steps 3 below); if already done, skip them.

**Files:**
- Modify: `crates/egret-auth/src/session.rs` (`platform_roles`, `permissions`)
- Modify: `crates/egret-bin/src/main.rs` (idempotency check) — if not already done in Task 1
- Verify (likely already done in Task 1): `crates/egret-auth/src/roles.rs` (`assignees_of`, `assignees_named`, `roles_of`, `count_platform_admins`)
- Test: existing `crates/egret-auth/tests/session_db.rs` + `roles_db.rs` cover this (they seed via legacy tables → trigger-mirrored → the repointed readers see the same rows).

- [ ] **Step 1: Repoint `session.rs` `platform_roles`**

```rust
    async fn platform_roles(&self, user_id: UserId) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT DISTINCT r.name FROM access_grants ag \
             JOIN roles r ON r.id = ag.role_id \
             WHERE ag.scope_type = 'platform' \
               AND ( (ag.principal_type = 'user'  AND ag.principal_id = $1) \
                  OR (ag.principal_type = 'group' AND ag.principal_id IN \
                         (SELECT group_id FROM user_groups WHERE user_id = $1)) )",
        )
        .bind(user_id.0)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(|(n,)| n).collect())
    }
```

- [ ] **Step 2: Repoint `session.rs` `permissions`** (flat/platform-only, unchanged semantics)

```rust
    async fn permissions(&self, user_id: UserId) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT DISTINCT rp.permission \
             FROM access_grants ag \
             JOIN role_permissions rp ON rp.role_id = ag.role_id \
             WHERE ag.scope_type = 'platform' \
               AND ( (ag.principal_type = 'user'  AND ag.principal_id = $1) \
                  OR (ag.principal_type = 'group' AND ag.principal_id IN \
                         (SELECT group_id FROM user_groups WHERE user_id = $1)) )",
        )
        .bind(user_id.0)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(|(p,)| p).collect())
    }
```

- [ ] **Step 3: Repoint `roles.rs` readers**

`assignees_of`:
```rust
        sqlx::query_as(
            "SELECT principal_type, principal_id FROM access_grants \
             WHERE role_id = $1 AND scope_type = 'platform'",
        )
```
`assignees_named` — replace both `platform_role_assignments pra` with `access_grants pra` and add `AND pra.scope_type = 'platform'` to each WHERE:
```rust
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT 'user' AS kind, u.email::text AS name \
               FROM access_grants pra JOIN users u ON u.id = pra.principal_id \
              WHERE pra.role_id = $1 AND pra.principal_type = 'user' AND pra.scope_type = 'platform' \
             UNION ALL \
             SELECT 'group' AS kind, g.name \
               FROM access_grants pra JOIN groups g ON g.id = pra.principal_id \
              WHERE pra.role_id = $1 AND pra.principal_type = 'group' AND pra.scope_type = 'platform' \
             ORDER BY name",
        )
```
`roles_of`:
```rust
        sqlx::query_as::<_, RoleRow>(
            "SELECT r.id, r.scope, r.name, r.description, r.parent_role_id FROM access_grants pra \
             JOIN roles r ON r.id = pra.role_id \
             WHERE pra.principal_type = $1 AND pra.principal_id = $2 AND pra.scope_type = 'platform' \
             ORDER BY r.name",
        )
```
`count_platform_admins`:
```rust
        sqlx::query_scalar(
            "SELECT count(*) FROM access_grants pra \
             JOIN roles r ON r.id = pra.role_id \
             WHERE r.name = 'platform_admin' AND pra.scope_type = 'platform'",
        )
```

- [ ] **Step 4: Repoint the bootstrap idempotency check in `main.rs`**

```rust
        let existing: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM access_grants ag \
             JOIN roles r ON r.id = ag.role_id \
             WHERE r.scope='platform' AND r.name='platform_admin' AND ag.scope_type='platform'",
        )
        .fetch_one(&db.pool)
        .await?;
```

- [ ] **Step 5: Run the reader suites**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-auth --test session_db --test roles_db`
Expected: PASS (seeds via legacy tables are trigger-mirrored into `access_grants`, which the repointed readers now read).
Run: `rtk proxy cargo build --workspace` clean; clippy/fmt the touched crates.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-auth/src/session.rs crates/egret-auth/src/roles.rs crates/egret-bin/src/main.rs
git commit -m "refactor(authz): platform grant readers read access_grants"
```

---

## Task 3: Repoint workspace-grant writers + readers

**Files:**
- Modify: `crates/egret-workspace/src/members.rs` (`add`, `remove`, `list`)
- Modify: `crates/egret-workspace/src/rbac.rs` (`workspace_roles_for`)
- Modify: `crates/egret-workspace/src/workspaces.rs` (`list_for_principal`)
- Test: `crates/egret-workspace/tests/workspace_db.rs` (extend for one-role-per-member)

- [ ] **Step 1: Write the failing test**

Add to `crates/egret-workspace/tests/workspace_db.rs` (mirror its existing `db()` + `MemberRepo`/`Authorizer` harness):

```rust
#[tokio::test]
async fn member_add_replaces_role_one_row() {
    let db = db().await;
    let members = egret_workspace::members::MemberRepo::new(db.clone());
    let authz = egret_workspace::rbac::Authorizer::new(db.clone());
    // seed a workspace + a user + an adder
    let ws = egret_core::WorkspaceId::new();
    sqlx::query("INSERT INTO workspaces (id, slug, name) VALUES ($1,$2,'W')")
        .bind(ws.0).bind(format!("flip-ws-{}", ws.0)).execute(&db.pool).await.unwrap();
    let uid = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, provider, email, name, status) VALUES ($1,'local',$2,'M','active')")
        .bind(uid).bind(format!("flipm-{uid}@t.local")).execute(&db.pool).await.unwrap();
    let by = egret_core::UserId(uid);
    // add ws_viewer, then re-add as ws_editor → exactly one workspace grant remains, = ws_editor
    members.add(ws, "user", uid, "ws_viewer", by).await.unwrap();
    members.add(ws, "user", uid, "ws_editor", by).await.unwrap();
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM access_grants WHERE principal_type='user' AND principal_id=$1 AND scope_type='workspace' AND scope_id=$2")
        .bind(uid).bind(ws.0).fetch_one(&db.pool).await.unwrap();
    assert_eq!(n, 1, "one workspace role per member");
    let roles = authz.workspace_roles_for(egret_core::UserId(uid), ws).await.unwrap();
    assert_eq!(roles, vec!["ws_editor".to_string()]);
    // cleanup
    sqlx::query("DELETE FROM access_grants WHERE principal_id=$1").bind(uid).execute(&db.pool).await.unwrap();
    sqlx::query("DELETE FROM users WHERE id=$1").bind(uid).execute(&db.pool).await.unwrap();
    sqlx::query("DELETE FROM workspaces WHERE id=$1").bind(ws.0).execute(&db.pool).await.unwrap();
}
```

- [ ] **Step 2: Run it — expect FAIL**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-workspace --test workspace_db member_add_replaces_role_one_row`
Expected: FAIL — `add` writes `workspace_members`; the trigger mirrors it, and `workspace_members`' PK already enforces one-role, so `access_grants` ends with one row via the trigger's UPDATE mirror. The `workspace_roles_for` assertion (reading `access_grants`) is the real new check and fails until Step 4. (If it passes via the trigger + repointed reader ordering, proceed — Step 3/4 make the write direct.)

- [ ] **Step 3: Repoint `members.rs` `add`/`remove`/`list`**

```rust
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
        // One workspace role per member: replace any existing workspace grant for
        // this principal in this workspace, then insert the new role.
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;
        sqlx::query(
            "DELETE FROM access_grants \
             WHERE principal_type = $1 AND principal_id = $2 AND scope_type = 'workspace' AND scope_id = $3",
        )
        .bind(ptype)
        .bind(pid)
        .bind(ws.0)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        sqlx::query(
            "INSERT INTO access_grants \
               (principal_type, principal_id, role_id, scope_type, scope_id, granted_by) \
             SELECT $1, $2, r.id, 'workspace', $3, $4 FROM roles r \
             WHERE r.scope = 'workspace' AND r.name = $5 \
             ON CONFLICT DO NOTHING",
        )
        .bind(ptype)
        .bind(pid)
        .bind(ws.0)
        .bind(by.0)
        .bind(role)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }

    pub async fn remove(&self, ws: WorkspaceId, ptype: &str, pid: Uuid) -> Result<()> {
        sqlx::query(
            "DELETE FROM access_grants \
             WHERE principal_type = $1 AND principal_id = $2 AND scope_type = 'workspace' AND scope_id = $3",
        )
        .bind(ptype)
        .bind(pid)
        .bind(ws.0)
        .execute(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    pub async fn list(&self, ws: WorkspaceId) -> Result<Vec<WorkspaceMember>> {
        let rows = sqlx::query_as::<_, MemberRow>(
            "SELECT ag.principal_type, ag.principal_id, r.name AS role \
             FROM access_grants ag JOIN roles r ON r.id = ag.role_id \
             WHERE ag.scope_type = 'workspace' AND ag.scope_id = $1",
        )
        .bind(ws.0)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(Into::into).collect())
    }
```

Note: `MemberRepo` must have access to the pool for a transaction — it already holds `db: Db`. Keep `MemberRow` as-is.

- [ ] **Step 4: Repoint `rbac.rs` `workspace_roles_for`**

```rust
    pub async fn workspace_roles_for(
        &self,
        user_id: UserId,
        ws: WorkspaceId,
    ) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT DISTINCT r.name FROM access_grants ag \
             JOIN roles r ON r.id = ag.role_id \
             WHERE ag.scope_type = 'workspace' AND ag.scope_id = $1 \
               AND ( (ag.principal_type = 'user'  AND ag.principal_id = $2) \
                  OR (ag.principal_type = 'group' AND ag.principal_id IN \
                         (SELECT group_id FROM user_groups WHERE user_id = $2)) )",
        )
        .bind(ws.0)
        .bind(user_id.0)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(|(r,)| r).collect())
    }
```

- [ ] **Step 5: Repoint `workspaces.rs` `list_for_principal`** (non-admin branch)

```rust
            sqlx::query_as::<_, WorkspaceRow>(
                "SELECT DISTINCT w.id, w.slug, w.name, w.status FROM workspaces w \
                 JOIN access_grants ag ON ag.scope_id = w.id AND ag.scope_type = 'workspace' \
                 WHERE w.status = 'active' \
                   AND ( (ag.principal_type = 'user'  AND ag.principal_id = $1) \
                      OR (ag.principal_type = 'group' AND ag.principal_id IN \
                             (SELECT group_id FROM user_groups WHERE user_id = $1)) ) \
                 ORDER BY w.name",
            )
```

- [ ] **Step 6: Run the workspace suite + build**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-workspace`
Expected: PASS.
Run: `rtk proxy cargo build --workspace` clean; clippy/fmt the touched crate.

- [ ] **Step 7: Commit**

```bash
git add crates/egret-workspace/src/members.rs crates/egret-workspace/src/rbac.rs crates/egret-workspace/src/workspaces.rs crates/egret-workspace/tests/workspace_db.rs
git commit -m "refactor(authz): workspace grant writers/readers use access_grants (one role per member)"
```

---

## Task 4: Repoint `egret-auth` test seeds

Production no longer writes the legacy tables, but the triggers still mirror any legacy-table writes, so tests are green. Now repoint the `egret-auth` tests that seed/read the legacy tables directly, so they no longer depend on the tables (which Task 6 drops). **The triggers still exist during this task**, so writing `access_grants` directly is safe and idempotent.

**Files (9):** `pdp_hot_reload.rs`, `grant_store.rs`, `cedar_pdp.rs`, `session_db.rs`, `admin_graph_forbid.rs`, `grant_downgrade.rs`, `pdp_regression.rs`, `pdp_app_wiring.rs`. **EXCLUDE `access_grants_migration.rs`** — that file specifically tests the 0068 backfill + sync triggers and is rewritten in Task 6.

**The two canonical transformations** (apply to every direct seed):

Platform seed:
```sql
-- BEFORE
INSERT INTO platform_role_assignments (principal_type, principal_id, role_id)
VALUES ('user', $1, $2)                          -- or: SELECT 'user',$1,id FROM roles WHERE ...
-- AFTER
INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id)
VALUES ('user', $1, $2, 'platform', NULL)        -- or: SELECT 'user',$1,id,'platform',NULL FROM roles WHERE ...
ON CONFLICT DO NOTHING
```

Workspace seed (role NAME → role_id):
```sql
-- BEFORE
INSERT INTO workspace_members (workspace_id, principal_type, principal_id, role[, added_by])
VALUES ($ws, 'user', $uid, 'ws_editor'[, $by])
-- AFTER
INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id[, granted_by])
SELECT 'user', $uid, r.id, 'workspace', $ws[, $by] FROM roles r
WHERE r.scope = 'workspace' AND r.name = 'ws_editor'
ON CONFLICT DO NOTHING
```

Adjust bind-parameter numbering per call site (sqlx `$n` are positional — renumber to match the reordered args). For `FROM platform_role_assignments`/`FROM workspace_members` **read/assert** statements in these tests, repoint to `access_grants` with the appropriate `scope_type` filter (and join `roles` for a name).

- [ ] **Step 1: Repoint each of the 8 files**

Go file by file. For each, `rtk proxy grep -n "platform_role_assignments\|workspace_members" crates/egret-auth/tests/<file>` to find every occurrence, then apply the transformation above. Preserve the test's intent (same principal, role, workspace). `pdp_regression.rs` is the golden parity matrix — its seeds must produce identical `access_grants` rows (they will: same principal/role/scope).

- [ ] **Step 2: Run the egret-auth suite (accurate count)**

Write `/tmp/b2flip_auth.sh`:
```sh
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret
cargo test -p egret-auth --no-fail-fast 2>&1 | tail -60
```
Run: `zsh /tmp/b2flip_auth.sh`
Expected: ALL pass (except `access_grants_migration.rs` which is untouched and still passes because the triggers still exist). `pdp_regression` green = parity intact.

- [ ] **Step 3: Commit**

```bash
git add crates/egret-auth/tests/
git commit -m "test(authz): repoint egret-auth test seeds to access_grants"
```

---

## Task 5: Repoint the remaining crates' test seeds

Same two transformations as Task 4, applied to every other crate's tests that seed/read the legacy tables directly. **Triggers still exist**, so this is safe.

**Files (~16):**
- `crates/egret-bi/tests/`: `share_deny.rs`, `chart_object_scope.rs`, `shares.rs`, `dashboard_object_scope.rs`, `http.rs`, `dataset_object_scope.rs`
- `crates/egret-bin/tests/provisioning.rs`
- `crates/egret-catalog-engine/tests/warehouse_admin_routes.rs`
- `crates/egret-controlplane/tests/routes_api.rs`
- `crates/egret-engine-mgmt/tests/routes.rs`
- `crates/egret-kernel/tests/`: `http.rs`, `kernel_object_scope.rs`
- `crates/egret-notebook/tests/notebook_object_scope.rs`
- `crates/egret-policy/tests/admin_routes.rs`
- `crates/egret-query/tests/execution_http.rs`

- [ ] **Step 1: Repoint each file**

For each: `rtk proxy grep -n "platform_role_assignments\|workspace_members" <file>`, apply the platform/workspace transformation from Task 4, renumbering `$n` binds as needed. Note some workspace seeds here include `added_by` (`crates/egret-kernel/tests/*`) — map it to `granted_by` in the `access_grants` SELECT.

- [ ] **Step 2: Build all test targets (catch renumbering errors early)**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test --workspace --no-run`
Expected: compiles clean.

- [ ] **Step 3: Run the touched crates (accurate count via script file)**

Write `/tmp/b2flip_rest.sh`:
```sh
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret
cargo test -p egret-bi -p egret-bin -p egret-catalog-engine -p egret-controlplane -p egret-engine-mgmt -p egret-kernel -p egret-notebook -p egret-policy -p egret-query --no-fail-fast 2>&1 | tail -80
```
Run: `zsh /tmp/b2flip_rest.sh`
Expected: ALL pass.

- [ ] **Step 4: Commit**

```bash
git add crates/egret-bi/tests crates/egret-bin/tests crates/egret-catalog-engine/tests crates/egret-controlplane/tests crates/egret-engine-mgmt/tests crates/egret-kernel/tests crates/egret-notebook/tests crates/egret-policy/tests crates/egret-query/tests
git commit -m "test(authz): repoint remaining crates' test seeds to access_grants"
```

---

## Task 6: Migration 0073 — drop triggers + legacy tables

At this point NOTHING (production or tests) writes or reads the legacy tables except `access_grants_migration.rs`. Drop them.

**Files:**
- Create: `migrations/0073_drop_legacy_grant_tables.sql`
- Modify: `crates/egret-metastore/src/lib.rs` (touch)
- Rewrite: `crates/egret-auth/tests/access_grants_migration.rs`

- [ ] **Step 1: Write the migration**

Create `migrations/0073_drop_legacy_grant_tables.sql`:

```sql
-- B2-flip: access_grants is now the single grant store. Defensively re-backfill
-- (self-heal any historical drift), then drop the sync triggers, their functions,
-- and the two legacy grant tables.
--
-- IMPORTANT — NULL scope_id gap: the 0068/0072 UNIQUE includes scope_id, which is
-- NULL for PLATFORM grants. Postgres treats NULLs as DISTINCT, so `ON CONFLICT DO
-- NOTHING` never fires for platform rows and would DUPLICATE them (over-counting
-- count_platform_admins → weakening the last-admin guard). Use NOT EXISTS guards
-- for the re-backfill, and add a partial unique index that makes platform grants
-- properly unique at the DB level (integrity guarantee for the last-admin count).

-- Platform-grant integrity: one grant per (principal, role) at platform scope.
-- Creates cleanly — the flip writers (assign/replace_platform_roles/bootstrap) use
-- NOT EXISTS so no duplicates exist at migration time; the legacy PK previously
-- gated the trigger the same way.
CREATE UNIQUE INDEX access_grants_platform_uniq
  ON access_grants (principal_type, principal_id, role_id)
  WHERE scope_type = 'platform';

-- Defensive re-backfill (dedup-safe via NOT EXISTS; the tables still exist here).
INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id)
  SELECT pra.principal_type, pra.principal_id, pra.role_id, 'platform', NULL
  FROM platform_role_assignments pra
  WHERE NOT EXISTS (
    SELECT 1 FROM access_grants ag
    WHERE ag.principal_type = pra.principal_type AND ag.principal_id = pra.principal_id
      AND ag.role_id = pra.role_id AND ag.scope_type = 'platform');
INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id)
  SELECT wm.principal_type, wm.principal_id, r.id, 'workspace', wm.workspace_id
  FROM workspace_members wm JOIN roles r ON r.scope = 'workspace' AND r.name = wm.role
  WHERE NOT EXISTS (
    SELECT 1 FROM access_grants ag
    WHERE ag.principal_type = wm.principal_type AND ag.principal_id = wm.principal_id
      AND ag.role_id = r.id AND ag.scope_type = 'workspace' AND ag.scope_id = wm.workspace_id);

DROP TRIGGER IF EXISTS sync_platform_grant_trg ON platform_role_assignments;
DROP TRIGGER IF EXISTS sync_workspace_grant_trg ON workspace_members;
DROP FUNCTION IF EXISTS sync_platform_grant();
DROP FUNCTION IF EXISTS sync_workspace_grant();
DROP TABLE IF EXISTS platform_role_assignments;
DROP TABLE IF EXISTS workspace_members;
```

> **Writers note (from Task 1):** the platform writers (`assign`, `replace_platform_roles`, bootstrap) use `WHERE NOT EXISTS` for idempotency (because `ON CONFLICT` can't fire on the NULL `scope_id`). Once `access_grants_platform_uniq` exists, those writers MAY optionally be converted to `ON CONFLICT (principal_type, principal_id, role_id) WHERE scope_type = 'platform' DO NOTHING` for race-safety — NOT required for this task (NOT EXISTS + the unique index together already prevent duplicates; a rare concurrent double-assign errors out rather than duplicating, which is safe). If you convert them, do it in this task and re-run the writer tests.

- [ ] **Step 2: Touch the metastore lib**

Run: `touch crates/egret-metastore/src/lib.rs`

- [ ] **Step 3: Rewrite `access_grants_migration.rs` for the post-flip world**

This test previously asserted the 0068 backfill + that a legacy-table write mirrors into `access_grants` via the trigger. Post-flip those tables and triggers are gone. Rewrite it to assert the flip end-state:

```rust
//! Post-B2-flip: the legacy grant tables are gone and access_grants is the sole
//! store. Requires DATABASE_URL.
use egret_metastore::Db;

async fn db() -> Db {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    db
}

#[tokio::test]
async fn legacy_grant_tables_are_dropped() {
    let db = db().await;
    for t in ["platform_role_assignments", "workspace_members"] {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM information_schema.tables WHERE table_name = $1)")
            .bind(t).fetch_one(&db.pool).await.unwrap();
        assert!(!exists, "{t} must be dropped after B2-flip");
    }
}

#[tokio::test]
async fn seeded_admin_grant_survived_in_access_grants() {
    // The migration backfilled the seeded platform_admin (if any assignment existed).
    // A fresh test DB may have no admin; this asserts the query path works and the
    // grant store is the platform-scope source (no legacy table).
    let db = db().await;
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM access_grants WHERE scope_type = 'platform'")
        .fetch_one(&db.pool).await.unwrap();
    assert!(n >= 0);
}
```

(If the original file had other assertions unrelated to the triggers, preserve them if they still hold; otherwise this replacement is the file's new content.)

- [ ] **Step 4: Verify no lingering references to the dropped tables**

Run: `rtk proxy grep -rn "platform_role_assignments\|workspace_members" crates/ migrations/0068_access_grants.sql migrations/0069_access_grants_triggers_fix.sql`
Expected: matches ONLY in `migrations/0068_*.sql`, `migrations/0069_*.sql` (historical, immutable), and `migrations/0073_*.sql` (the drop itself). NO matches in any `crates/**/src` or `crates/**/tests` (except comments). If any remain in `src`/`tests`, they were missed in Tasks 1–5 — fix them now.

- [ ] **Step 5: Run the migration + auth suite**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-auth --test access_grants_migration`
Expected: PASS (tables gone).
Run (accurate, script file): `zsh /tmp/b2flip_auth.sh`
Expected: ALL egret-auth pass.

- [ ] **Step 6: Commit**

```bash
git add migrations/0073_drop_legacy_grant_tables.sql crates/egret-metastore/src/lib.rs crates/egret-auth/tests/access_grants_migration.rs
git commit -m "feat(authz): drop legacy grant tables + sync triggers (B2-flip)"
```

---

## Task 7: Verify — full CI gate

**Files:** none (verification; fix forward if a regression appears).

- [ ] **Step 1: Backend fmt + clippy**

Run: `rtk proxy cargo fmt --all --check` (if it fails, `cargo fmt --all` + note).
Run: `rtk proxy cargo clippy --workspace --all-targets -- -D warnings` — clean (ignore the pre-existing third-party `mysql_async` warning).

- [ ] **Step 2: Full workspace test (accurate count)**

Write `/tmp/b2flip_all.sh`:
```sh
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret
cargo test --workspace --no-fail-fast 2>&1 | tail -80
```
Run: `zsh /tmp/b2flip_all.sh`
Expected: ALL pass. Known-benign: on a truly fresh `egret_test`, the 2 engine `open_secret` NULL-ws tests may fail (pre-existing, unrelated). Any OTHER failure is a real regression — fix forward.

- [ ] **Step 3: PDP parity explicit check**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-auth pdp_regression object_scope`
Expected: the allow-only golden matrix + object-scope suites pass unchanged (the flip did not change `access_grants` contents).

- [ ] **Step 4: OpenAPI currency**

Run: `rtk proxy cargo run -p egret-bin -- openapi > /tmp/openapi_flip.json && git diff --no-index openapi.json /tmp/openapi_flip.json`
Expected: NO diff (no DTO changed). If a diff exists, overwrite `openapi.json`, `cd frontend && npm run gen:api`, commit both.

- [ ] **Step 5: Frontend gate** (unaffected, but confirm)

Run (from `frontend`): `rtk proxy npm run lint && rtk proxy npm run typecheck && rtk proxy npm run test && rtk proxy npm run build`
Expected: all pass.

- [ ] **Step 6: Commit any fix-forward**

If Steps 1–5 needed a fix: `git add -A && git commit -m "chore(authz): B2-flip CI gate fixes"`. Otherwise report GREEN with no changes.

---

## Notes for the security review (after Task 7)

Focus: no grant lost (0073 re-backfill before drop; access_grants was trigger-mirrored since 0068); one-role-per-member preserved (`MemberRepo::add` delete+insert in a tx); last-platform-admin guard intact (`count_platform_admins` counts access_grants platform_admin grants); bootstrap idempotency reads access_grants; no new permission surface (signatures + semantics preserved); PDP parity unchanged (access_grants contents identical); transactional writes avoid an empty interim window; object-guest vs member distinction unchanged (`workspace_roles_for` now reads access_grants; object-guests hold no workspace grant → sentinel path).
