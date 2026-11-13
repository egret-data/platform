# Governance SP3c-1 — Role principals → `role:<name> holders` managed groups

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace every `kind='role'` principal reference in the neutral permission model with a managed group `role:<name> holders` whose membership mirrors the role's holders, so the compiled engine output is byte-identical and role principals disappear from the enforcement input.

**Architecture:** Add a `source='managed'` group class. For each role referenced by a data grant, rule exemption or engine grant, keep a managed group `role:<name> holders` in sync with the role's holder users (reusing the existing holder-resolution queries). A reversible, equivalence-gated migration rewrites the `role` refs to `group` refs pointing at those managed groups. The role-holder recompile chokepoint is extended to keep the managed groups fresh when holders change.

**Tech Stack:** Rust (sqlx, axum) in `egret-perm-compile` / `egret-perm` / `egret-auth`; Postgres migrations; no frontend change expected (verify `explain` payload stays byte-stable).

## Global Constraints

- **Byte-identical compiled output.** `role` and `group` principal refs both expand to the same member usernames at compile time (`members::expand_enumerated_principals`, `members.rs:783`), so rewriting a `role` ref to a `role:<name> holders` group whose membership mirrors the role's holders MUST leave every engine's compiled policy byte-identical. A golden/equivalence diff that is non-empty is a failure.
- **`explain` unchanged** for every affected principal (per spec "done when"). Prove with the equivalence-probe pattern, not by eyeballing.
- **Managed group naming is exactly `role:<name> holders`** (literal, one space each side of `holders`), `<name>` = the role name verbatim.
- **Transitional (concepts D):** once this ships, no NEW grant/rule/engine-grant may target a `role:<name> holders` group directly, and no new `kind='role'` principal may be written. Enforce both at the write seam.
- **Reversible with provenance.** The rewrite records enough to undo it exactly; undo is hash-guarded and refuses (409) if the current rows diverge from what was migrated. Reuse the *patterns* in `migrate.rs` (`grant_hash`/`rule_hash`/`canonical_json`, `read_undo_state` FOR UPDATE), NOT `perm.migrated_from` (that table is `policy_id`-keyed and excludes clearance/roles).
- **Only referenced roles are migrated.** Measured on the demo stack: exactly one role (`data_steward`) is referenced (1 row in `perm.grant_principals`, 0 in `rule_exemptions`, 0 in `engine_grant_principals`). The code must be general over any referenced role, but adds no handling for the 9 unreferenced roles.
- **All `user_groups` writes route through `PermGroupRecompiler`** (`group_membership.rs:98`) so a membership change recompiles the affected profiles.
- **Migration numbers are a shared namespace.** Current max is `0219`; new migrations are `0220+` and must be renumbered above `main` and any open PR at execution time.
- **CI gates:** `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `committed_openapi_json_is_current` (regen `egret openapi > openapi.json`), generated-client digest (`frontend/scripts/write-api-digest.mjs` / `npm run gen:api`), DB-gated Rust tests.
- **Worktree-local target:** `CARGO_TARGET_DIR=target-sp3c`. Commits are Conventional Commits ending `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Stage only changed files; do not push.
- **DB-gated tests** connect via `DATABASE_URL=postgres://egret:egret@localhost:5432/egret`; run single-threaded when timing-sensitive (shared dev DB flakes). No StarRocks engine is created in this package, so the fork-image gotcha does not apply here (it applies to 3c-2).

## File Structure

- `migrations/0220_managed_group_source.sql` (new) — allow `groups.source='managed'`.
- `migrations/0221_role_principal_migration.sql` (new) — provenance table `perm.role_principal_migration` for the reversible rewrite.
- `crates/egret-perm-compile/src/role_holder_group.rs` (new) — managed-group name helper + holder-mirror sync (`sync_role_holder_group_in_tx`, `role_holder_group_name`).
- `crates/egret-perm-compile/src/role_membership.rs` (modify, ~line 266) — extend the recompile chokepoint to also re-sync holder groups, and reconcile its role/group arms.
- `crates/egret-perm-compile/src/role_principal_migrate.rs` (new) — the reversible, equivalence-gated rewrite + undo (`migrate_role_principals_in_tx`, `undo_role_principals_in_tx`).
- `crates/egret-perm-compile/src/routes.rs` (modify) — admin endpoints `POST /perm/migrate/role-principals` + `.../undo`; and the write-seam guards (reject new `kind='role'` and new refs to a `role:* holders` group).
- `crates/egret-perm-compile/src/principal_refs.rs` (modify, ~line 31-96) — accept the managed-group ref as a valid group principal.
- Tests: `crates/egret-perm-compile/tests/role_principal_migrate.rs` (new, DB-gated).

---

### Task 1: Allow `source='managed'` groups

**Files:**
- Create: `migrations/0220_managed_group_source.sql`
- Test: `crates/egret-perm-compile/tests/role_principal_migrate.rs` (new; first test only)

**Interfaces:**
- Consumes: `public.groups(id uuid, name text UNIQUE, source text)` with existing `CHECK (source IN ('local','oidc'))` from `migrations/0002_identity.sql:18`.
- Produces: `groups.source` additionally accepts `'managed'`.

- [ ] **Step 1: Write the migration**

```sql
-- 0220_managed_group_source.sql
-- Managed groups (role:<name> holders, ws:<slug> members) are maintained by egret,
-- not by local admin or OIDC. Widen the source check to admit them.
ALTER TABLE public.groups DROP CONSTRAINT IF EXISTS groups_source_check;
ALTER TABLE public.groups
  ADD CONSTRAINT groups_source_check CHECK (source IN ('local','oidc','managed'));
```

- [ ] **Step 2: Write the failing test**

```rust
// crates/egret-perm-compile/tests/role_principal_migrate.rs
use egret_test_db::{self, with_test_db};
use uuid::Uuid;

#[tokio::test]
async fn a_managed_group_source_is_accepted() {
    let Some(pool) = egret_test_db::pool().await else { return };
    let id = Uuid::new_v4();
    let r = sqlx::query("INSERT INTO public.groups (id, name, source) VALUES ($1,$2,'managed')")
        .bind(id)
        .bind(format!("role:data_steward holders {id}"))
        .execute(&pool)
        .await;
    assert!(r.is_ok(), "managed source must be accepted: {r:?}");
}
```

- [ ] **Step 3: Run it to confirm it fails before the migration is applied**

Run: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret CARGO_TARGET_DIR=target-sp3c cargo test -p egret-perm-compile --test role_principal_migrate a_managed_group_source_is_accepted -- --test-threads=1`
Expected: FAIL (check constraint violation) if run against a DB without 0220; PASS once migrations apply. (The test harness applies migrations to a scratch DB, so after Step 1 the test passes — confirm it is the migration, not the test, doing the work by temporarily renaming 0220 if in doubt.)

- [ ] **Step 4: Run it to confirm it passes with the migration**

Run: same command. Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add migrations/0220_managed_group_source.sql crates/egret-perm-compile/tests/role_principal_migrate.rs
git commit -m "feat(perm): allow managed-source groups"
```

---

### Task 2: Mirror a role's holders into its `role:<name> holders` managed group

**Files:**
- Create: `crates/egret-perm-compile/src/role_holder_group.rs`
- Modify: `crates/egret-perm-compile/src/lib.rs` (add `pub(crate) mod role_holder_group;`)
- Test: `crates/egret-perm-compile/tests/role_principal_migrate.rs`

**Interfaces:**
- Consumes: `members::role_holders_via(conn, &[String]) -> Result<Vec<MemberRow>, CompileError>` (`members.rs:566`, direct user holders) and `members::role_group_holders_via(conn, &[String])` (`members.rs:595`, holders via groups). A `MemberRow` carries the holder's user id + username (confirm fields in `members.rs`). `public.user_groups(user_id, group_id)`.
- Produces:
  - `pub(crate) fn role_holder_group_name(role: &str) -> String` → `format!("role:{role} holders")`.
  - `pub(crate) async fn sync_role_holder_group_in_tx(tx: &mut Transaction<'_, Postgres>, role: &str) -> Result<Uuid, CompileError>` — upserts the managed group `source='managed'`, reconciles `user_groups` to exactly the role's holder user ids (insert missing, delete extra), returns the group id. Idempotent.

- [ ] **Step 1: Write the holder-mirror module**

```rust
// crates/egret-perm-compile/src/role_holder_group.rs
use crate::members;
use crate::CompileError;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

/// The managed group that mirrors a role's holders. Naming is load-bearing
/// (SP3c-1 global constraints) and must match principal_refs validation.
pub(crate) fn role_holder_group_name(role: &str) -> String {
    format!("role:{role} holders")
}

/// Upsert the `role:<name> holders` managed group and reconcile its
/// `user_groups` membership to exactly the role's holder user ids. Returns the
/// group id. Idempotent; safe to call on every holder change.
pub(crate) async fn sync_role_holder_group_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    role: &str,
) -> Result<Uuid, CompileError> {
    let name = role_holder_group_name(role);
    // Upsert the managed group; keep source='managed' on conflict.
    let group_id: Uuid = sqlx::query_scalar(
        "INSERT INTO public.groups (id, name, source) VALUES (gen_random_uuid(), $1, 'managed') \
         ON CONFLICT (name) DO UPDATE SET source = 'managed' RETURNING id",
    )
    .bind(&name)
    .fetch_one(&mut **tx)
    .await
    .map_err(CompileError::from)?;

    // Target holder user ids: direct user holders UNION holders via groups.
    // role_holders_via / role_group_holders_via return the holder member rows;
    // collect their user ids. (Confirm MemberRow's id field name in members.rs.)
    let roles = [role.to_string()];
    let mut want: std::collections::BTreeSet<Uuid> = std::collections::BTreeSet::new();
    for m in members::role_holders_via(&mut **tx, &roles).await? {
        want.insert(m.user_id);
    }
    for m in members::role_group_holders_via(&mut **tx, &roles).await? {
        want.insert(m.user_id);
    }

    // Reconcile user_groups to `want`.
    sqlx::query(
        "DELETE FROM public.user_groups \
         WHERE group_id = $1 AND user_id <> ALL($2)",
    )
    .bind(group_id)
    .bind(want.iter().copied().collect::<Vec<_>>())
    .execute(&mut **tx)
    .await
    .map_err(CompileError::from)?;

    for uid in &want {
        sqlx::query(
            "INSERT INTO public.user_groups (user_id, group_id) VALUES ($1,$2) \
             ON CONFLICT (user_id, group_id) DO NOTHING",
        )
        .bind(uid)
        .bind(group_id)
        .execute(&mut **tx)
        .await
        .map_err(CompileError::from)?;
    }
    Ok(group_id)
}
```

> **Implementer note:** verify the exact `MemberRow` field name (the brief shows holder rows carry a user id + username; adjust `m.user_id` to the real field). If `role_holders_via` returns usernames rather than ids, resolve to ids via `users` or change the reconcile to key on username. Keep the "mirror exactly" semantics.

- [ ] **Step 2: Write the failing test**

```rust
#[tokio::test]
async fn the_holder_group_mirrors_the_roles_holders() {
    let Some(pool) = egret_test_db::pool().await else { return };
    // Arrange: a role with one direct user holder (use the test seed helpers;
    // see clearance_triggers.rs for the seed pattern — create a user, a role,
    // and an access_grants row principal_type='user' effect='allow').
    let (role, user_id) = seed_role_with_one_user_holder(&pool).await;

    // Act
    let mut tx = pool.begin().await.unwrap();
    let gid = egret_perm_compile::role_holder_group::sync_role_holder_group_in_tx(&mut tx, &role)
        .await
        .expect("sync");
    tx.commit().await.unwrap();

    // Assert: the managed group exists, source='managed', and holds exactly that user.
    let (name, source): (String, String) =
        sqlx::query_as("SELECT name, source FROM public.groups WHERE id=$1")
            .bind(gid).fetch_one(&pool).await.unwrap();
    assert_eq!(name, format!("role:{role} holders"));
    assert_eq!(source, "managed");
    let members: Vec<Uuid> =
        sqlx::query_scalar("SELECT user_id FROM public.user_groups WHERE group_id=$1")
            .bind(gid).fetch_all(&pool).await.unwrap();
    assert_eq!(members, vec![user_id]);
}
```

- [ ] **Step 3: Run it to verify it fails**

Run: `... cargo test -p egret-perm-compile --test role_principal_migrate the_holder_group_mirrors -- --test-threads=1`
Expected: FAIL (module/function not found) before Step 1 is wired into `lib.rs`.

- [ ] **Step 4: Wire the module and run to verify it passes**

Add `pub(crate) mod role_holder_group;` to `crates/egret-perm-compile/src/lib.rs` (and `pub mod` if the test calls it as `egret_perm_compile::role_holder_group::...`; prefer exposing a thin `pub` re-export rather than widening visibility broadly). Re-run. Expected: PASS.

- [ ] **Step 5: Add an idempotency + removal assertion**

```rust
#[tokio::test]
async fn re_syncing_reconciles_added_and_removed_holders() {
    let Some(pool) = egret_test_db::pool().await else { return };
    let (role, u1) = seed_role_with_one_user_holder(&pool).await;
    let mut tx = pool.begin().await.unwrap();
    let gid = egret_perm_compile::role_holder_group::sync_role_holder_group_in_tx(&mut tx, &role).await.unwrap();
    tx.commit().await.unwrap();
    // Remove the holder, add a different one, re-sync.
    let u2 = revoke_u1_grant_u2(&pool, &role, u1).await;
    let mut tx = pool.begin().await.unwrap();
    egret_perm_compile::role_holder_group::sync_role_holder_group_in_tx(&mut tx, &role).await.unwrap();
    tx.commit().await.unwrap();
    let members: Vec<Uuid> = sqlx::query_scalar("SELECT user_id FROM public.user_groups WHERE group_id=$1")
        .bind(gid).fetch_all(&pool).await.unwrap();
    assert_eq!(members, vec![u2]);
}
```

Run the file; expected PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-perm-compile/src/role_holder_group.rs crates/egret-perm-compile/src/lib.rs crates/egret-perm-compile/tests/role_principal_migrate.rs
git commit -m "feat(perm-compile): mirror a role's holders into a managed role:<name> holders group"
```

---

### Task 3: Keep holder groups fresh at the role-membership recompile chokepoint

**Files:**
- Modify: `crates/egret-perm-compile/src/role_membership.rs` (around `recompile_role_referencing_profiles`, `role_membership.rs:266` and its query `:271-275`)
- Test: `crates/egret-perm-compile/tests/role_principal_migrate.rs`

**Interfaces:**
- Consumes: `role_membership::recompile_role_referencing_profiles(pool)` (`role_membership.rs:266`), which today UNIONs role refs from `perm.grant_principals` (kind='role'), `perm.engine_grant_principals` (kind='role') and `classification_policies.required_roles`. `sync_role_holder_group_in_tx` (Task 2). `PermGroupRecompiler` (`group_membership.rs:98`).
- Produces: after a role's holders change, the role's `role:<name> holders` managed group membership is re-synced before/as part of the recompile, so group-ref consumers see fresh membership.

- [ ] **Step 1: Write the failing test (holder change propagates to the managed group)**

```rust
#[tokio::test]
async fn a_holder_change_resyncs_the_managed_group_via_the_chokepoint() {
    let Some(pool) = egret_test_db::pool().await else { return };
    let (role, _u1) = seed_role_with_one_user_holder(&pool).await;
    // Create the managed group once.
    { let mut tx = pool.begin().await.unwrap();
      egret_perm_compile::role_holder_group::sync_role_holder_group_in_tx(&mut tx, &role).await.unwrap();
      tx.commit().await.unwrap(); }
    // Grant the role to a second user, then invoke the chokepoint.
    let u2 = grant_role_to_new_user(&pool, &role).await;
    egret_perm_compile::role_membership::recompile_role_referencing_profiles(&pool).await.unwrap();
    // The managed group now contains u2.
    let present: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM public.user_groups ug \
         JOIN public.groups g ON g.id=ug.group_id \
         WHERE g.name=$1 AND ug.user_id=$2)")
        .bind(format!("role:{role} holders")).bind(u2)
        .fetch_one(&pool).await.unwrap();
    assert!(present, "chokepoint must resync the holder group");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run the test; expected FAIL (chokepoint does not yet sync managed groups).

- [ ] **Step 3: Extend the chokepoint**

In `recompile_role_referencing_profiles`, before (or alongside) recompiling, enumerate the roles that have a `role:<name> holders` managed group (or that are referenced) and call `sync_role_holder_group_in_tx` for each inside the same transaction, then let the existing recompile run. Keep the existing three-arm reference query for now; Task 4/5 reconcile the arms once role refs are rewritten. Minimal diff:

```rust
// inside recompile_role_referencing_profiles, within the tx, before recompiling:
let managed_roles: Vec<String> = sqlx::query_scalar(
    "SELECT substring(name from 'role:(.*) holders$') FROM public.groups \
     WHERE source='managed' AND name LIKE 'role:% holders'",
).fetch_all(&mut *tx).await?;
for role in &managed_roles {
    crate::role_holder_group::sync_role_holder_group_in_tx(&mut tx, role).await?;
}
```

> **Implementer note:** match the function's real transaction/connection shape (it may take `pool` and open its own tx). Keep everything in one transaction so a failure rolls back cleanly. Do not double-recompile.

- [ ] **Step 4: Run it to verify it passes**

Run the test; expected PASS. Then run the full `role_principal_migrate` file single-threaded; all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-perm-compile/src/role_membership.rs crates/egret-perm-compile/tests/role_principal_migrate.rs
git commit -m "feat(perm-compile): resync role holder groups at the recompile chokepoint"
```

---

### Task 4: Reversible rewrite of role refs → holder-group refs (+ provenance)

**Files:**
- Create: `migrations/0221_role_principal_migration.sql`
- Create: `crates/egret-perm-compile/src/role_principal_migrate.rs`
- Modify: `crates/egret-perm-compile/src/lib.rs` (`pub(crate) mod role_principal_migrate;`)
- Modify: `crates/egret-perm-compile/src/principal_refs.rs` (accept the managed-group ref)
- Test: `crates/egret-perm-compile/tests/role_principal_migrate.rs`

**Interfaces:**
- Consumes: `perm.grant_principals(grant_id, kind, ref, mode)`, `perm.rule_exemptions(rule_id, kind, ref, mode)`, `perm.engine_grant_principals(engine_grant_id, kind, ref)` (kinds from `PrincipalKind`, `model.rs:76`); `role_holder_group::{role_holder_group_name, sync_role_holder_group_in_tx}` (Task 2); the equivalence probe helpers (`equivalence::derive_probes`, `equivalence.rs:314`); hash helpers patterned on `migrate.rs` (`canonical_json` `:676`).
- Produces:
  - `pub async fn migrate_role_principals_in_tx(tx, migrated_by: &str) -> Result<RolePrincipalMigration, PermHttpError>` — for each distinct referenced role: ensure the holder group (Task 2), rewrite each `kind='role'` row to `kind='group'`, `ref=role_holder_group_name(role)`; record provenance; run the equivalence gate (compiled output byte-identical before/after) and refuse on divergence; return counts + ids.
  - `pub async fn undo_role_principals_in_tx(tx, migration_id: i64) -> Result<(), PermHttpError>` — hash-guarded reverse (group ref → role ref), refusing 409 if current rows diverge from the recorded set.
  - `RolePrincipalMigration { id: i64, roles: Vec<String>, rewritten: usize }`.

- [ ] **Step 1: Write the provenance migration**

```sql
-- 0221_role_principal_migration.sql
-- Reversible record of the role-principal -> role:<name> holders group rewrite.
-- Separate from perm.migrated_from (that table is policy_id-keyed and excludes
-- clearance/roles). One row per migration run.
CREATE TABLE perm.role_principal_migration (
    id           bigserial PRIMARY KEY,
    roles        text[]      NOT NULL,          -- roles that were rewritten
    rewrites     jsonb       NOT NULL,          -- [{table, key, role, group_name, prev_ref, prev_kind}]
    row_hashes   text[]      NOT NULL,          -- canonical hash per rewritten row, for undo guard
    migrated_by  text        NOT NULL,
    migrated_at  timestamptz NOT NULL DEFAULT now()
);
```

- [ ] **Step 2: Write the failing test (byte-identical compile + role refs gone)**

```rust
#[tokio::test]
async fn migrating_role_refs_leaves_the_compile_byte_identical_and_removes_role_kind() {
    let Some(pool) = egret_test_db::pool().await else { return };
    // Arrange: a service with a Trino grant whose principal is kind='role' ref='data_steward',
    // and data_steward has holders. Seed via the test helpers (seed_grant with a ('role','data_steward') principal).
    let svc = seed_service_with_role_grant(&pool, "data_steward").await;
    // Capture the compiled Ranger output before.
    let before = compile_service_to_ranger_json(&pool, svc).await;

    // Act
    let mut tx = pool.begin().await.unwrap();
    let m = egret_perm_compile::role_principal_migrate::migrate_role_principals_in_tx(&mut tx, "tester")
        .await.expect("migrate");
    tx.commit().await.unwrap();

    // Assert: no kind='role' rows remain anywhere.
    for (schema_tbl, _) in [("perm.grant_principals",""),("perm.rule_exemptions",""),("perm.engine_grant_principals","")] {
        let n: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {schema_tbl} WHERE kind::text='role'"))
            .fetch_one(&pool).await.unwrap();
        assert_eq!(n, 0, "{schema_tbl} still has role refs");
    }
    assert!(m.roles.contains(&"data_steward".to_string()));
    // Assert: compiled output is byte-identical.
    let after = compile_service_to_ranger_json(&pool, svc).await;
    assert_eq!(before, after, "compile must be byte-identical after the rewrite");
}
```

- [ ] **Step 3: Run it to verify it fails**

Run single-threaded; expected FAIL (module/function not found).

- [ ] **Step 4: Implement the rewrite + provenance + equivalence gate**

Write `role_principal_migrate.rs`:
1. `SELECT DISTINCT ref FROM perm.grant_principals WHERE kind='role'` UNION the other two tables → the referenced roles.
2. For each role: `let gid_name = role_holder_group_name(role); sync_role_holder_group_in_tx(tx, role).await?;`
3. Build the equivalence probes (`equivalence::derive_probes`) from the service definition BEFORE rewriting; capture the before-IR/compile.
4. Rewrite in each table: `UPDATE perm.<tbl> SET kind='group', ref=$group_name WHERE kind='role' AND ref=$role` (preserve `mode` where the table has it). Collect each affected row's prior `(table, key, role, prev_kind, prev_ref)` into `rewrites` and its `canonical_json` hash into `row_hashes`.
5. Recompile the affected services; re-run the probes; if any verdict diverges, return an error (the tx rolls back).
6. `INSERT INTO perm.role_principal_migration (roles, rewrites, row_hashes, migrated_by) ... RETURNING id`.

Add to `principal_refs.rs` (`:31-96`): a `group` ref whose name matches `^role:.* holders$` is a valid managed-group principal (it expands via `groups.name` → members like any group). Do not special-case its expansion — the existing group arm already expands by name.

- [ ] **Step 5: Run it to verify it passes**

Run the test single-threaded; expected PASS (role refs gone, compile byte-identical).

- [ ] **Step 6: Write the undo test + implement undo**

```rust
#[tokio::test]
async fn undo_restores_role_refs_when_rows_are_untouched_and_refuses_when_changed() {
    let Some(pool) = egret_test_db::pool().await else { return };
    let svc = seed_service_with_role_grant(&pool, "data_steward").await;
    let mut tx = pool.begin().await.unwrap();
    let m = egret_perm_compile::role_principal_migrate::migrate_role_principals_in_tx(&mut tx, "tester").await.unwrap();
    tx.commit().await.unwrap();
    // Untouched undo restores the role ref.
    let mut tx = pool.begin().await.unwrap();
    egret_perm_compile::role_principal_migrate::undo_role_principals_in_tx(&mut tx, m.id).await.unwrap();
    tx.commit().await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM perm.grant_principals WHERE kind::text='role' AND ref='data_steward'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(n, 1, "undo restores the role ref");
}
```

Implement `undo_role_principals_in_tx`: `SELECT ... FOR UPDATE` the provenance row, recompute each current row's canonical hash, refuse `PermHttpError` 409 if any differs, else reverse each rewrite (`kind='group' ref=<group>` → recorded `prev_kind/prev_ref`), recompile, delete the provenance row.

- [ ] **Step 7: Run the file; all pass**

Run: `... cargo test -p egret-perm-compile --test role_principal_migrate -- --test-threads=1`. Expected: all PASS.

- [ ] **Step 8: Commit**

```bash
git add migrations/0221_role_principal_migration.sql crates/egret-perm-compile/src/role_principal_migrate.rs crates/egret-perm-compile/src/lib.rs crates/egret-perm-compile/src/principal_refs.rs crates/egret-perm-compile/tests/role_principal_migrate.rs
git commit -m "feat(perm-compile): reversible role-principal -> holder-group rewrite with equivalence gate"
```

---

### Task 5: Admin endpoints + transitional write-seam guards

**Files:**
- Modify: `crates/egret-perm-compile/src/routes.rs` (register routes near `:185`; guards in `write_grant` `:1326` / `write_rule` `:1689` and the engine-grant write path)
- Modify: `openapi.json` (regenerate), `frontend/src/api/*` (regenerate client)
- Test: `crates/egret-perm-compile/tests/role_principal_migrate.rs` (endpoint + guard tests)

**Interfaces:**
- Consumes: `migrate_role_principals_in_tx` / `undo_role_principals_in_tx` (Task 4); existing auth pattern (`policy:write` or the admin scope used by `migrate.rs`'s endpoints — match it).
- Produces:
  - `POST /api/control-plane/perm/migrate/role-principals` → runs the migration, returns `{ id, roles, rewritten }`.
  - `POST /api/control-plane/perm/migrate/role-principals/undo` with `{ id }` → undoes it.
  - Write-seam guards: a new grant/rule/engine-grant principal with `kind='role'` is refused (422); a new principal that is a `group` ref matching `^role:.* holders$` is refused (422) — these groups are migration targets only.

- [ ] **Step 1: Write the failing guard test**

```rust
#[tokio::test]
async fn writing_a_new_role_principal_is_refused() {
    let Some(pool) = egret_test_db::pool().await else { return };
    let svc = seed_empty_service(&pool).await;
    // Attempt to create a grant with a kind='role' principal through the write seam.
    let err = try_write_grant_with_principal(&pool, svc, ("role","data_steward","grant")).await;
    assert!(err.is_err(), "new role principals must be refused post-SP3c-1");
}

#[tokio::test]
async fn writing_a_new_ref_to_a_holder_group_is_refused() {
    let Some(pool) = egret_test_db::pool().await else { return };
    let svc = seed_empty_service(&pool).await;
    let err = try_write_grant_with_principal(&pool, svc, ("group","role:data_steward holders","grant")).await;
    assert!(err.is_err(), "holder groups are migration targets only");
}
```

- [ ] **Step 2: Run to verify they fail**

Run single-threaded; expected FAIL (no guard yet).

- [ ] **Step 3: Add the guards**

In the principal-validation used by `write_grant`/`write_rule`/engine-grant write: reject `kind='role'` with a clear message ("role principals are migrated to managed groups; grant to a group instead"); reject a `group` ref matching `^role:.* holders$` ("role:<name> holders groups are managed migration targets; grant to the role's intended group directly"). Return the crate's 422 error type.

- [ ] **Step 4: Run to verify they pass**

Run single-threaded; expected PASS.

- [ ] **Step 5: Register the endpoints + `#[utoipa::path]`**

Mirror `migrate.rs`'s endpoint registration (handler + `#[utoipa::path]` + `ApiDoc` paths). Add an endpoint smoke test that calls the migrate route on a seeded service and asserts 200 + role refs gone (reuse Task 4 assertions via HTTP).

- [ ] **Step 6: Regenerate openapi + client**

Run:
```bash
CARGO_TARGET_DIR=target-sp3c cargo run -q --bin egret -- openapi > openapi.json
cd frontend && npm run gen:api
```
Confirm `git diff` shows only the new endpoints + their generated client, and that a re-run of `node frontend/scripts/write-api-digest.mjs` leaves no further change.

- [ ] **Step 7: Full gate check**

Run: `CARGO_TARGET_DIR=target-sp3c cargo fmt --check` and `cargo clippy -p egret-perm-compile --all-targets -- -D warnings`; run the `role_principal_migrate` test file single-threaded. All clean/pass.

- [ ] **Step 8: Commit**

```bash
git add crates/egret-perm-compile/src/routes.rs openapi.json frontend/src/api crates/egret-perm-compile/tests/role_principal_migrate.rs
git commit -m "feat(perm-compile): role-principal migration endpoints + transitional write guards"
```

---

### Task 6: Verify `explain` is unchanged for affected principals

**Files:**
- Test: `crates/egret-perm-compile/tests/role_principal_migrate.rs`
- Reference (no change expected): `crates/egret-perm-compile/src/routes.rs` `explain_users` (`:2658`), `routes/people.rs::explain_principal` (`:422`)

**Interfaces:**
- Consumes: the `explain` path (`POST /perm/explain` and `/perm/explain/principal`). The migration (Task 4).
- Produces: a test proving `explain`'s per-principal verdict JSON is byte-identical before and after the migration, for a member of the migrated role.

- [ ] **Step 1: Write the test**

```rust
#[tokio::test]
async fn explain_is_byte_identical_across_the_role_migration() {
    let Some(pool) = egret_test_db::pool().await else { return };
    let svc = seed_service_with_role_grant(&pool, "data_steward").await;
    let member = a_holder_of(&pool, "data_steward").await; // username
    let before = explain_principal_json(&pool, svc, &member).await;
    { let mut tx = pool.begin().await.unwrap();
      egret_perm_compile::role_principal_migrate::migrate_role_principals_in_tx(&mut tx, "tester").await.unwrap();
      tx.commit().await.unwrap(); }
    let after = explain_principal_json(&pool, svc, &member).await;
    assert_eq!(before, after, "explain must not change for a migrated role's member");
}
```

- [ ] **Step 2: Run it**

Run single-threaded. Expected: PASS. If it fails, the rewrite changed effective access or the explain attribution — investigate (do not "fix" the test); the migration must be access-preserving.

- [ ] **Step 3: Commit**

```bash
git add crates/egret-perm-compile/tests/role_principal_migrate.rs
git commit -m "test(perm-compile): explain unchanged across the role-principal migration"
```

---

### Task 7: Frontend — stop offering `role` principals in the grant dialogs

**Files:**
- Modify: `frontend/src/components/perm/PrincipalPicker.tsx` (~line 127: the `kinds` list for non-exempt use; ~line 270-286: the role field + "Users, groups or roles" label)
- Modify: `frontend/src/routes/governance/GrantDialog.tsx` (uses `<PrincipalPicker>` with no `exempt` prop, ~line 260)
- Modify: `frontend/src/routes/governance/engines/tabs/EngineGrantDialog.tsx` (`<PrincipalPicker includeOnly>`, ~line 272; label ~line 270)
- Test: `frontend/src/components/perm/__tests__/PrincipalPicker.test.tsx` (or the dialogs' existing vitest; add a case)

**Interfaces:**
- Consumes: the backend write guard (Task 5) now rejects `kind='role'` and `group` refs matching `^role:.* holders$` with a 400. Rule authoring uses the picker in `exempt` mode, which already does not offer `role` — leave it alone.
- Produces: the grant and engine-grant authoring flows no longer offer `role` as a principal kind, so a user cannot create a request the backend will reject.

- [ ] **Step 1: Write the failing test**

```tsx
// Assert the non-exempt PrincipalPicker no longer offers "role".
import { render, screen } from "@testing-library/react";
import { PrincipalPicker } from "../PrincipalPicker";

test("non-exempt picker does not offer the role kind", () => {
  render(<PrincipalPicker value={[]} onChange={() => {}} />); // non-exempt default
  // The kind selector must list user + group, never role.
  expect(screen.queryByRole("option", { name: /role/i })).toBeNull();
});
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cd frontend && npx vitest run src/components/perm/__tests__/PrincipalPicker.test.tsx`
Expected: FAIL (role is still offered).

- [ ] **Step 3: Remove the `role` option**

In `PrincipalPicker.tsx`, drop `"role"` from the non-exempt `kinds` array (~:127) and remove the role free-text branch (~:270-286). Update the label "Users, groups or roles" → "Users or groups" in the picker and in `EngineGrantDialog.tsx` (~:270). Do NOT touch the `exempt` path (rule exemptions) or the `role` TYPE in `frontend/src/api` (still needed to render existing/explain data). Leave read/explain rendering of role principals intact.

- [ ] **Step 4: Run it to verify it passes + run the dialog tests**

Run: `cd frontend && npx vitest run src/components/perm src/routes/governance`
Expected: PASS; no dialog test regressions.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/perm/PrincipalPicker.tsx frontend/src/routes/governance/GrantDialog.tsx frontend/src/routes/governance/engines/tabs/EngineGrantDialog.tsx frontend/src/components/perm/__tests__/PrincipalPicker.test.tsx
git commit -m "feat(ui): stop offering role principals in the grant and engine-grant dialogs"
```

---

### Task 8: Docs — role principals are migrated to managed groups

**Files:**
- Create/modify: in the docs repo `egret-data/docs` (Platform → Data governance section): a short note that role principals are migrated to managed `role:<name> holders` groups, that new grants/engine-grants no longer offer a `role` kind (use a group), and that the migration is reversible. This is prepared on the docs branch `docs/gov-sp3c-roles` (committed later in the docs worktree, not in this platform worktree).

**Interfaces:**
- Consumes: the user-visible changes from Task 5 (new endpoints, role authoring removed) and Task 7 (picker change).
- Produces: a docs PR in `egret-data/docs` paired with this platform PR; the PR's Docs box is ticked.

- [ ] **Step 1: Draft the docs note** in the docs repo worktree (handled by the controller after the code tasks, alongside opening the docs PR). Content: what changed (role → managed group), why (one grant model direction), how to grant role-based access now (grant to a group), and that the migration is reversible via the admin endpoint. No screenshot needed unless the picker change is shown.

- [ ] **Step 2: Commit on `docs/gov-sp3c-roles`** in the docs repo (Conventional Commit, co-author Claude Opus 5.5).

> Note: Task 8 is executed by the controller in the `egret-data/docs` worktree after the platform code tasks are green, not by a platform-worktree implementer. It is listed here for completeness and for the PR's Docs checkbox.

---

## Out of scope (later SP3c packages)

- Clearance → withhold rule + `perm.rule_kind='withhold'` schema + clearance provenance (SP3c-2).
- Implicit workspace grant → explicit `ws:<slug> members` grant (SP3c-3; reuses the managed-group machinery from Task 1–3).
- Removing the now-dead `required_roles`-on-clearance holder-resolution path (follows SP3c-2).
- Any frontend change: expected none here; Task 6 proves `explain` is stable. If a reviewer finds a user-visible change, file it for the package that introduces it + docs/screenshots per the repo docs rule.

## Self-Review

- **Spec coverage (SP3c point 2):** role refs in grant_principals / rule_exemptions / engine_grant_principals → Tasks 4; managed group mirrors holders → Tasks 2–3; transitional no-new-role-ref → Task 5; `explain` unchanged → Task 6; decision ③ "same machinery" → Tasks 2–3 build the managed-group machinery reused by SP3c-3. ✅
- **Placeholder scan:** implementer notes flag the two spots needing a live signature check (`MemberRow` field; the chokepoint's tx shape) — these are verification notes, not placeholders; the code is otherwise concrete. ✅
- **Type consistency:** `role_holder_group_name` / `sync_role_holder_group_in_tx` / `migrate_role_principals_in_tx` / `undo_role_principals_in_tx` / `RolePrincipalMigration` are named identically wherever referenced. ✅
