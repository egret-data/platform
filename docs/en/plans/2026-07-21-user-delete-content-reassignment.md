# User-Delete Content Reassignment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `DELETE /users/{id}` succeed for a user who owns content — auto-null the 14 nullable attribution FKs and reassign the NOT-NULL `notebooks.owner_user_id` to the deleting admin.

**Architecture:** A migration flips the 14 nullable `REFERENCES users(id)` FKs to `ON DELETE SET NULL` (DB handles them automatically). `UserRepo::hard_delete` gains a `reassign_notebooks_to` param and reassigns the target's notebooks before deleting the user; the existing `23503`→409 becomes a rare safety net.

**Tech Stack:** Rust (sqlx/Postgres), `egret-metastore`/`egret-auth`/`egret-controlplane`; Playwright e2e.

**Spec:** `docs/en/specs/2026-07-21-user-delete-content-reassignment-design.md`

**DB for tests:** `.env` has `DATABASE_URL=postgres://egret:egret@localhost:5432/egret` — export it for DB tests. Migrations live at repo-root `migrations/`.

**Authoritative FK list (verified from the live schema — names + schemas are exact):**
- Group B → SET NULL (14, all nullable): `analytics.charts.created_by`, `analytics.dashboards.created_by`, `analytics.datasets.created_by`, `analytics.folders.created_by`, `analytics.saved_queries.created_by`, `engine_mgmt.engine_workspaces.assigned_by`, `public.access_requests.decided_by`, `public.catalog_tag_assignments.assigned_by`, `public.classification_policies.created_by`, `public.tag_assignments.assigned_by`, `public.tags.created_by`, `public.workspaces.created_by`, `query.databases.created_by`, `query.queries.user_id`. Constraint names are `<table>_<col>_fkey`.
- Group C → reassign (1, NOT NULL): `analytics.notebooks.owner_user_id`.

---

## Task 1: Migration — `ON DELETE SET NULL` on the 14 nullable FKs

**Files:**
- Create: `migrations/0093_user_delete_set_null.sql`
- Modify: `crates/egret-metastore/src/lib.rs` (touch to re-embed)
- Test: `crates/egret-auth/tests/user_delete_setnull.rs` (new) — or add to an existing egret-auth DB test file

- [ ] **Step 1: Write the migration**

Create `migrations/0093_user_delete_set_null.sql`:

```sql
-- User hard-delete: attribution columns (who created/assigned/decided) get their
-- FK to users(id) flipped to ON DELETE SET NULL, so deleting a user preserves the
-- content and just drops the attribution. True ownership (notebooks.owner_user_id,
-- NOT NULL) is handled separately by reassignment in UserRepo::hard_delete.
ALTER TABLE analytics.charts
  DROP CONSTRAINT charts_created_by_fkey,
  ADD CONSTRAINT charts_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id) ON DELETE SET NULL;
ALTER TABLE analytics.dashboards
  DROP CONSTRAINT dashboards_created_by_fkey,
  ADD CONSTRAINT dashboards_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id) ON DELETE SET NULL;
ALTER TABLE analytics.datasets
  DROP CONSTRAINT datasets_created_by_fkey,
  ADD CONSTRAINT datasets_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id) ON DELETE SET NULL;
ALTER TABLE analytics.folders
  DROP CONSTRAINT folders_created_by_fkey,
  ADD CONSTRAINT folders_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id) ON DELETE SET NULL;
ALTER TABLE analytics.saved_queries
  DROP CONSTRAINT saved_queries_created_by_fkey,
  ADD CONSTRAINT saved_queries_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id) ON DELETE SET NULL;
ALTER TABLE engine_mgmt.engine_workspaces
  DROP CONSTRAINT engine_workspaces_assigned_by_fkey,
  ADD CONSTRAINT engine_workspaces_assigned_by_fkey FOREIGN KEY (assigned_by) REFERENCES users(id) ON DELETE SET NULL;
ALTER TABLE public.access_requests
  DROP CONSTRAINT access_requests_decided_by_fkey,
  ADD CONSTRAINT access_requests_decided_by_fkey FOREIGN KEY (decided_by) REFERENCES users(id) ON DELETE SET NULL;
ALTER TABLE public.catalog_tag_assignments
  DROP CONSTRAINT catalog_tag_assignments_assigned_by_fkey,
  ADD CONSTRAINT catalog_tag_assignments_assigned_by_fkey FOREIGN KEY (assigned_by) REFERENCES users(id) ON DELETE SET NULL;
ALTER TABLE public.classification_policies
  DROP CONSTRAINT classification_policies_created_by_fkey,
  ADD CONSTRAINT classification_policies_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id) ON DELETE SET NULL;
ALTER TABLE public.tag_assignments
  DROP CONSTRAINT tag_assignments_assigned_by_fkey,
  ADD CONSTRAINT tag_assignments_assigned_by_fkey FOREIGN KEY (assigned_by) REFERENCES users(id) ON DELETE SET NULL;
ALTER TABLE public.tags
  DROP CONSTRAINT tags_created_by_fkey,
  ADD CONSTRAINT tags_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id) ON DELETE SET NULL;
ALTER TABLE public.workspaces
  DROP CONSTRAINT workspaces_created_by_fkey,
  ADD CONSTRAINT workspaces_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id) ON DELETE SET NULL;
ALTER TABLE query.databases
  DROP CONSTRAINT databases_created_by_fkey,
  ADD CONSTRAINT databases_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id) ON DELETE SET NULL;
ALTER TABLE query.queries
  DROP CONSTRAINT queries_user_id_fkey,
  ADD CONSTRAINT queries_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE SET NULL;
```

BEFORE finalizing: run `grep -rn "0092\|0091" migrations/` to confirm `0093` is the next free number, and re-verify each constraint name against the live DB (the names above are from `pg_constraint`, but confirm none drifted):
```
psql "$DATABASE_URL" -tAc "SELECT n.nspname||'.'||t.relname, c.conname FROM pg_constraint c JOIN pg_class t ON t.oid=c.conrelid JOIN pg_namespace n ON n.oid=t.relnamespace WHERE c.contype='f' AND pg_get_constraintdef(c.oid) LIKE '%REFERENCES users(id)%' AND pg_get_constraintdef(c.oid) NOT LIKE '%CASCADE%' ORDER BY 1;"
```
If any name differs, use the real one.

- [ ] **Step 2: Touch the metastore lib to re-embed**

Run: `touch crates/egret-metastore/src/lib.rs` (the embedded `sqlx::migrate!` only re-embeds on recompile — known staleness gotcha).

- [ ] **Step 3: Write the failing test — a direct user DELETE nulls a dashboard's created_by**

Create `crates/egret-auth/tests/user_delete_setnull.rs` (mirror the connect()/skip idiom from `crates/egret-auth/tests/groups_db.rs`; `#[sqlx::test]` auto-applies all migrations incl. 0093):

```rust
//! Migration 0093: deleting a user SET-NULLs attribution columns (here: a
//! dashboard's created_by), so content survives the user's removal.
#[sqlx::test(migrations = "../../migrations")]
async fn deleting_user_nulls_dashboard_created_by(pool: sqlx::PgPool) {
    // seed a user + a workspace + a dashboard created_by that user
    let uid = sqlx::query_scalar::<_, uuid::Uuid>(
        "INSERT INTO users (provider, email, name) VALUES ('local','u@egret.local','U') RETURNING id")
        .fetch_one(&pool).await.unwrap();
    // a dashboard needs a workspace_id — seed the minimum a dashboard row requires
    // (inspect analytics.dashboards' NOT NULL columns via \d and fill them).
    let wsid = seed_workspace(&pool, uid).await; // helper: insert a workspace
    let did = sqlx::query_scalar::<_, uuid::Uuid>(
        "INSERT INTO analytics.dashboards (workspace_id, name, created_by) VALUES ($1,'D',$2) RETURNING id")
        .bind(wsid).bind(uid).fetch_one(&pool).await.unwrap();
    // delete the user directly (no notebook owned → no NOT-NULL block)
    sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.unwrap();
    // the dashboard survives, created_by is now NULL
    let created_by: Option<uuid::Uuid> = sqlx::query_scalar(
        "SELECT created_by FROM analytics.dashboards WHERE id = $1").bind(did).fetch_one(&pool).await.unwrap();
    assert!(created_by.is_none(), "created_by SET NULL after user delete");
    let _ = did;
}
```

IMPORTANT: inspect the real required columns of `analytics.dashboards` and `public.workspaces` (`psql "$DATABASE_URL" -c "\d analytics.dashboards"` / `\d public.workspaces`) and fill every NOT-NULL column in the INSERTs — do not guess. Write a small `seed_workspace` helper accordingly. If `workspaces.created_by` is also getting SET NULL you can seed it with the same user or NULL.

- [ ] **Step 4: Run to verify it passes (migration applied)**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret; cargo test -p egret-auth deleting_user_nulls_dashboard_created_by`
Expected: PASS (the migration made the delete succeed + nulled the column). If it fails with a FK violation on `DELETE FROM users`, some OTHER non-cascade/non-setnull FK is still blocking — re-run the pg_constraint query and add that table to the migration.

- [ ] **Step 5: Commit**

```bash
git add migrations/0093_user_delete_set_null.sql crates/egret-metastore/src/lib.rs crates/egret-auth/tests/user_delete_setnull.rs
git commit -m "feat(users): ON DELETE SET NULL on attribution FKs so user-delete preserves content

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: `hard_delete` reassigns notebooks + flip the tests

**Files:**
- Modify: `crates/egret-auth/src/users.rs` (`hard_delete` signature + notebook reassign)
- Modify: `crates/egret-controlplane/src/access/mod.rs` (`delete_user` passes caller; reword 409)
- Modify: `crates/egret-controlplane/tests/routes_api.rs` (flip the content-owner test)
- Modify: `crates/egret-auth/tests/*` if `hard_delete` has a direct unit test (Task-4/#305 added `hard_delete_removes_user_and_dependents`)

- [ ] **Step 1: Change `hard_delete`'s signature + reassign notebooks**

In `crates/egret-auth/src/users.rs`, change `hard_delete(&self, id: UserId)` to take the reassignee, and add the notebook UPDATE as the FIRST statement in the tx (note the `analytics.` schema qualifier — notebooks is NOT in `public`):

```rust
    /// Hard-delete `target` and dependents in one transaction. `target`'s notebooks
    /// are reassigned to `reassign_notebooks_to` (the deleting admin) because
    /// `notebooks.owner_user_id` is NOT NULL; attribution columns (created_by/
    /// assigned_by/…) are auto-nulled by their `ON DELETE SET NULL` FKs (mig 0093);
    /// cascade tables (sessions/user_groups/…) auto-clean. `access_grants` is swept
    /// explicitly (polymorphic principal_id, no FK).
    pub async fn hard_delete(&self, target: UserId, reassign_notebooks_to: UserId) -> Result<()> {
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;
        sqlx::query("UPDATE analytics.notebooks SET owner_user_id = $2 WHERE owner_user_id = $1")
            .bind(target.0).bind(reassign_notebooks_to.0)
            .execute(&mut *tx).await.map_err(db_err)?;
        sqlx::query("DELETE FROM sessions WHERE user_id = $1").bind(target.0)
            .execute(&mut *tx).await.map_err(db_err)?;
        sqlx::query("DELETE FROM user_groups WHERE user_id = $1").bind(target.0)
            .execute(&mut *tx).await.map_err(db_err)?;
        sqlx::query("DELETE FROM access_grants WHERE principal_type = 'user' AND principal_id = $1").bind(target.0)
            .execute(&mut *tx).await.map_err(db_err)?;
        let res = sqlx::query("DELETE FROM users WHERE id = $1").bind(target.0)
            .execute(&mut *tx).await;
        if let Err(e) = res {
            if e.as_database_error().and_then(|d| d.code()).as_deref() == Some("23503") {
                return Err(egret_core::Error::Conflict(
                    "cannot delete: this user owns content in a table that isn't auto-handled \
                     on delete — a new FK to users(id) needs an ON DELETE rule.".into(),
                ));
            }
            return Err(db_err(e));
        }
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }
```

(Keep the exact `db_err`/pool patterns already in the file. This preserves the 23503→409 catch as a safety net, now with the reworded message.)

- [ ] **Step 2: Update the `delete_user` handler to pass the caller**

In `crates/egret-controlplane/src/access/mod.rs` `delete_user`, change the call:

```rust
    st.users.hard_delete(user_id, p.user_id).await?;
```

(The self-guard in `guard_not_last_admin` already ran, so `p.user_id != user_id`.)

- [ ] **Step 3: Fix the direct `hard_delete` unit test signature**

The #305 test `hard_delete_removes_user_and_dependents` calls `hard_delete(uid)` — update it to `hard_delete(uid, admin_uid)` (seed a second user as the reassignee). Its existing assertions (user gone, group membership gone) still hold.

- [ ] **Step 4: Flip the content-owner route test to 204 + reassignment**

In `crates/egret-controlplane/tests/routes_api.rs`, the #305 test `deleting_a_content_owner_conflicts_not_500` asserts a dashboard-owner delete → 409. That now returns 204. Rename + rewrite it to `deleting_a_content_owner_reassigns_and_succeeds`:
- seed a target user owning a **dashboard** (`analytics.dashboards.created_by`) AND a **notebook** (`analytics.notebooks.owner_user_id` — needs a workspace + the notebook's NOT-NULL cols; inspect `\d analytics.notebooks`);
- `DELETE /users/{id}` as an admin → assert **204**;
- assert the user is gone, the dashboard survives with `created_by IS NULL`, and the notebook survives with `owner_user_id = <admin id>`.

- [ ] **Step 5: Run to verify all pass**

Run: `export DATABASE_URL=...; cargo test -p egret-auth -p egret-controlplane` (the `hard_delete` tests + the flipped route test).
Expected: PASS. `cargo clippy -p egret-auth -p egret-controlplane --all-targets -- -D warnings` clean; `cargo fmt --all -- --check` clean.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-auth/src/users.rs crates/egret-controlplane/src/access/mod.rs crates/egret-controlplane/tests/routes_api.rs crates/egret-auth/tests/
git commit -m "feat(users): reassign notebooks to the deleting admin; content-owning delete now succeeds

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3 (optional): Live e2e — content-owner delete succeeds

**Files:**
- Modify: `frontend/tests/e2e/local-user-mgmt-kind.spec.ts`

- [ ] **Step 1: Add a content-owner delete test**

Extend the spec: admin creates a local user → that user creates a notebook via the notebook API (grep existing specs / `notebook-*-kind.spec.ts` for the create-notebook call + the `ws` route) → admin `DELETE /api/control-plane/users/{id}` → assert **204** and the notebook still exists (now owned by the admin — `GET` the notebook or list notebooks). Reuse the existing `local-user-mgmt-kind.spec.ts` helpers (admin login, create user, local login). Keep it in the same `describe`.

- [ ] **Step 2: Verify discoverable + commit**

Run: `cd frontend && npx tsc --noEmit && npx playwright test local-user-mgmt-kind --list` → lists the new test.
```bash
git add frontend/tests/e2e/local-user-mgmt-kind.spec.ts
git commit -m "test(users): live e2e — deleting a notebook owner reassigns + succeeds

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Final verification (before finishing the branch)

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test -p egret-auth -p egret-controlplane` (DATABASE_URL set)
- [ ] `psql "$DATABASE_URL" -tAc "…pg_constraint query…"` shows the 14 columns now `ON DELETE SET NULL` and only `notebooks.owner_user_id` remains a plain FK
- [ ] Live (optional): rebuild backend + redeploy + `just e2e-kind-local-user` green (incl. the content-owner delete)
