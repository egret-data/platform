# Governance SP3d — per-EP migration-complete gate — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give each enforcement point (EP = a `policy.authz_services` row) a *migration-complete* verdict — a pure read over the EP's own rows that is true only when no unconvertible operator policy remains (SP3b) **and** its role/workspace/clearance input is migrated (SP3c) — surfaced over HTTP and persisted on each compile.

**Architecture:** Mirror the OPA switch's `stored_state_refusals_via` (one definition, "empty = ready"). A pure `ep_gate(...) -> GateVerdict` takes the four already-computed inputs; an async scoper `ep_gate_via[_tx]` composes them from the EP's rows, scoped through `catalogs_for_service_via(ep)`. The verdict is surfaced read-only at `GET /perm/policies/services/{id}/migration-gate` and its `complete` flag is persisted on `perm.compile_runs` at the one seam every compile reaches (`recompile_locked_service_in_tx`). The gate **never fails a compile** — the old model still enforces until migrated (no fail-open), so the verdict is a status signal, not a refusal.

**Tech Stack:** Rust (axum, sqlx/Postgres), `egret-perm-compile` + `egret-perm` + `egret-policy` crates, utoipa openapi, orval-generated TS client.

## Global Constraints

- Branch `feat/gov-sp3d`, stacked on `feat/gov-sp3c-3`; **rebase onto `main` after #861 merges** before the final review. Worktree `/Users/1pro-l4r8/Documents/github/data-portal/egret-sp3d`.
- `CARGO_TARGET_DIR=target-sp3d` for every cargo command (shared target → E0432). Never `git add -A` — `target-sp3d/` is NOT in `.gitignore`; stage files by explicit path.
- Commits end `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. PR body ends `🤖 Generated with [Claude Code](https://claude.com/claude-code)`. Conventional Commits. `Closes #N` only in PR body. Plain `git commit` (no `-c` identity override). No secrets.
- Latest migration on the stack is `0227`; SP3d adds `0228+`. Migration numbers are a shared namespace — renumber above `main` AND any open PR at merge time.
- **Every DB-gated test takes a `static SERIAL: tokio::sync::Mutex<()>` and scopes its assertions to its own EP/catalog/workspace (never a global `count(*)`), so it passes under default `cargo test` parallelism** (SP3c-1/2/3 flake lesson).
- The gate reads the reversible migration state (`perm.migrated_from`, `perm.clearance_migration`, `perm.ws_assignment_migration`); it never closes, deletes, or writes them.
- **Out of scope** (deferred follow-up, needs the demo stack): the `mechanism` switch, engine reconfigure, live Trino/StarRocks checks, badge screenshots, and deleting legacy inputs. No new "old model / new model" column or per-EP model/version field (spec §SP3d).
- Docs for the new endpoint go in `egret-data/docs` (Platform), bilingual, and the PR ticks the Docs box.

---

### Task 1: Gate verdict types + pure `ep_gate`

**Files:**
- Create: `crates/egret-perm-compile/src/ep_gate.rs`
- Modify: `crates/egret-perm-compile/src/lib.rs` (register `pub mod ep_gate;`)

**Interfaces:**
- Consumes: `crate::classify::EpMigrationStatus` (fields `complete: bool`, `pending: usize`, `blocked: Vec<BlockedPolicy>`).
- Produces: `GateVerdict`, `GateRefusal`, `pub fn ep_gate(mig: &EpMigrationStatus, role_refs: i64, ws_grants: i64, unmigrated_clearance_tags: Vec<String>) -> GateVerdict`.

- [ ] **Step 1: Write the failing unit tests** in `ep_gate.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::classify::EpMigrationStatus;

    fn done() -> EpMigrationStatus {
        EpMigrationStatus { complete: true, migrated: 3, pending: 0, blocked: vec![] }
    }

    #[test]
    fn all_inputs_clean_is_complete() {
        let v = ep_gate(&done(), 0, 0, vec![]);
        assert!(v.complete);
        assert!(v.refusals.is_empty());
    }

    #[test]
    fn a_pending_operator_policy_blocks_completion() {
        let mig = EpMigrationStatus { complete: false, migrated: 1, pending: 2, blocked: vec![] };
        let v = ep_gate(&mig, 0, 0, vec![]);
        assert!(!v.complete);
        assert_eq!(v.refusals, vec![GateRefusal::OperatorPolicies { pending: 2, blocked: 0 }]);
    }

    #[test]
    fn role_workspace_and_clearance_each_produce_their_refusal_in_order() {
        let v = ep_gate(&done(), 2, 1, vec!["pii".into(), "phi".into()]);
        assert!(!v.complete);
        assert_eq!(
            v.refusals,
            vec![
                GateRefusal::RolePrincipals { count: 2 },
                GateRefusal::WorkspaceGrants { count: 1 },
                GateRefusal::ClearanceTags { tags: vec!["pii".into(), "phi".into()] },
            ]
        );
    }

    #[test]
    fn each_refusal_has_a_nonempty_message() {
        for r in [
            GateRefusal::OperatorPolicies { pending: 1, blocked: 1 },
            GateRefusal::RolePrincipals { count: 1 },
            GateRefusal::WorkspaceGrants { count: 1 },
            GateRefusal::ClearanceTags { tags: vec!["pii".into()] },
        ] {
            assert!(!r.message().is_empty());
        }
    }
}
```

- [ ] **Step 2: Run the tests, verify they fail** (types undefined).

Run: `CARGO_TARGET_DIR=target-sp3d cargo test -p egret-perm-compile --lib ep_gate -- --nocapture`
Expected: FAIL to compile (`GateRefusal`, `ep_gate` not found).

- [ ] **Step 3: Implement the types + pure function:**

```rust
//! SP3d: the per-EP migration-complete gate. A pure verdict over an EP's own
//! rows — true only when no unconvertible operator policy remains (SP3b) and the
//! EP's role/workspace/clearance input is migrated (SP3c). The scoper that reads
//! those inputs lives in `ep_gate_scope`; this module decides nothing from the
//! database. Modeled on `egret_policy::opa_guard::stored_state_refusals`
//! ("empty = ready").

use crate::classify::EpMigrationStatus;

/// One reason an EP is not yet migration-complete. Order is stable (operator →
/// role → workspace → clearance) so the surfaced list and the tests agree.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GateRefusal {
    /// Operator (hand-written) policies the EP still carries (SP3b).
    OperatorPolicies { pending: usize, blocked: usize },
    /// `kind='role'` principal refs the EP still names (SP3c-1).
    RolePrincipals { count: i64 },
    /// Implicit workspace grants the EP still carries (SP3c-3).
    WorkspaceGrants { count: i64 },
    /// Gated clearance tags with no `withhold` rule yet (SP3c-2, installation-wide).
    ClearanceTags { tags: Vec<String> },
}

impl GateRefusal {
    pub fn message(&self) -> String {
        match self {
            Self::OperatorPolicies { pending, blocked } => format!(
                "{pending} operator policies await migration and {blocked} cannot be migrated"
            ),
            Self::RolePrincipals { count } => {
                format!("{count} role principal refs remain; migrate them to managed groups")
            }
            Self::WorkspaceGrants { count } => format!(
                "{count} implicit workspace grants remain; migrate them to `ws:<slug> members` groups"
            ),
            Self::ClearanceTags { tags } => format!(
                "{} gated clearance tags have no withhold rule yet: {}",
                tags.len(),
                tags.join(", ")
            ),
        }
    }
}

/// Whether an EP's migration is complete, and if not, why.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
pub struct GateVerdict {
    pub complete: bool,
    pub refusals: Vec<GateRefusal>,
}

/// Decide the verdict from the four already-computed inputs. Pure.
pub fn ep_gate(
    mig: &EpMigrationStatus,
    role_refs: i64,
    ws_grants: i64,
    unmigrated_clearance_tags: Vec<String>,
) -> GateVerdict {
    let mut refusals = Vec::new();
    if !mig.complete {
        refusals.push(GateRefusal::OperatorPolicies {
            pending: mig.pending,
            blocked: mig.blocked.len(),
        });
    }
    if role_refs > 0 {
        refusals.push(GateRefusal::RolePrincipals { count: role_refs });
    }
    if ws_grants > 0 {
        refusals.push(GateRefusal::WorkspaceGrants { count: ws_grants });
    }
    if !unmigrated_clearance_tags.is_empty() {
        refusals.push(GateRefusal::ClearanceTags { tags: unmigrated_clearance_tags });
    }
    GateVerdict { complete: refusals.is_empty(), refusals }
}
```

Add `pub mod ep_gate;` to `lib.rs` next to `pub mod classify;` (with a one-line doc comment: "SP3d: the per-EP migration-complete gate — see the module doc.").

- [ ] **Step 4: Run the tests, verify they pass.**

Run: `CARGO_TARGET_DIR=target-sp3d cargo test -p egret-perm-compile --lib ep_gate`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit.**

```bash
git add crates/egret-perm-compile/src/ep_gate.rs crates/egret-perm-compile/src/lib.rs
git commit -m "feat(perm): add the pure per-EP migration-complete gate verdict (SP3d)"
```

---

### Task 2: Async scoper `ep_gate_via` / `ep_gate_via_tx`

**Files:**
- Modify: `crates/egret-perm-compile/src/ep_gate.rs` (add the scoper + its DB test)

**Interfaces:**
- Consumes: `crate::classify::{classify_service, ep_migration_status}`; `egret_policy::catalogs_for_service_via(conn, service_id) -> Vec<CatalogRow>` (field `.id: Uuid`); `egret_perm::clearance`/`classification_policies` tables; `egret_perm_compile::clearance_migrate::withhold_rule_name(tag_key) -> String`.
- Produces: `pub async fn ep_gate_via_tx(tx: &mut Transaction<'_, Postgres>, service_id: i64) -> Result<GateVerdict, egret_perm::CompileError>` and `pub async fn ep_gate_via(conn: &mut PgConnection, service_id: i64) -> Result<GateVerdict, egret_perm::CompileError>`.

**Scoping rules (verify each query against the live schema before trusting it):**
- **role_refs** (per-EP): count distinct `kind='role'` principal rows across the EP's grants, engine-grants, and reachable rule-exemptions. EP catalogs = `catalogs_for_service_via(service_id)`.
- **ws_grants** (per-EP): count `perm.grants` rows on the EP's catalogs with `origin='workspace_assignment'` OR a `kind='workspace_members'` principal.
- **clearance** (installation-wide — clearance migration is one global run writing global withhold rules): every gated `classification_policies` tag (gated = `cardinality(required_roles)>0 OR cardinality(required_groups)>0`) whose tag key has no `perm.rules kind='withhold'` rule named `withhold_rule_name(tag_key)`. **The gate cannot read withhold-ness from the compiler's loaded rules — `list_rules_for_catalogs_via` excludes `kind='withhold'` — so query `perm.rules` directly.**

- [ ] **Step 1: Write the failing DB test** (append to `ep_gate.rs` tests module). Seed one EP + catalog, a role grant, a ws-assignment grant, and a gated clearance with no withhold rule; assert the three refusals; then migrate each input and assert `complete`. Use the `#[tokio::test]` + `egret_test_db::url()` skip pattern and a module `static SERIAL: tokio::sync::Mutex<()>`. (Model the seeding on `tests/ws_assignment_migrate.rs` and `crates/egret-perm-compile/src/clearance_migrate.rs` tests — reuse their helpers' shapes: `access_grants` needs a non-null `role_id`; a workspace member is a `(user, workspace, allow)` grant.)

```rust
// sketch — the implementer fills seeding from the referenced tests
#[tokio::test]
async fn an_ep_with_legacy_inputs_is_incomplete_then_complete_after_migration() {
    let Some(dsn) = egret_test_db::url().await else { return };
    let _guard = SERIAL.lock().await;
    // ... connect, migrate, seed EP + catalog + a kind='role' grant, a
    // origin='workspace_assignment' grant, a gated classification_policies row.
    let v = ep_gate_via(&mut conn, service_id).await.expect("gate");
    assert!(!v.complete);
    assert!(v.refusals.iter().any(|r| matches!(r, GateRefusal::RolePrincipals { .. })));
    assert!(v.refusals.iter().any(|r| matches!(r, GateRefusal::WorkspaceGrants { .. })));
    assert!(v.refusals.iter().any(|r| matches!(r, GateRefusal::ClearanceTags { .. })));
    // ... run role_principal_migrate + ws_assignment_migrate + clearance_migrate
    let v = ep_gate_via(&mut conn, service_id).await.expect("gate");
    assert!(v.complete, "{:?}", v.refusals);
}
```

- [ ] **Step 2: Run it, verify it fails** (`ep_gate_via` not found).

Run: `CARGO_TARGET_DIR=target-sp3d DATABASE_URL=<throwaway> cargo test -p egret-perm-compile --lib ep_gate::tests::an_ep_with_legacy_inputs`
Expected: FAIL to compile.

- [ ] **Step 3: Implement the scoper.** `ep_gate_via` acquires nothing (takes a conn); `ep_gate_via_tx` runs on a transaction. Both compute the four inputs then call `ep_gate`. Example `ep_gate_via` body:

```rust
use egret_perm::CompileError;
use sqlx::{PgConnection, Postgres, Transaction};

pub async fn ep_gate_via(
    conn: &mut PgConnection,
    service_id: i64,
) -> Result<GateVerdict, CompileError> {
    let classified = crate::classify::classify_service(conn, service_id)
        .await
        .map_err(CompileError::from_store)?; // match the real error mapping
    let mig = crate::classify::ep_migration_status(&classified);

    let catalogs: Vec<uuid::Uuid> =
        egret_policy::catalogs_for_service_via(conn, service_id)
            .await
            .map_err(CompileError::from_store)?
            .into_iter()
            .map(|c| c.id)
            .collect();

    let role_refs: i64 = sqlx::query_scalar(
        "SELECT \
           (SELECT count(*) FROM perm.grants g \
              JOIN perm.grant_principals p ON p.grant_id = g.id AND p.kind = 'role' \
             WHERE g.catalog_id = ANY($1)) \
         + (SELECT count(*) FROM perm.engine_grant_principals p \
              JOIN perm.engine_grants eg ON eg.id = p.engine_grant_id \
              JOIN engine_mgmt.engines e ON e.id = eg.engine_id \
             WHERE e.authz_service_id = $2 AND p.kind = 'role') \
         + (SELECT count(*) FROM perm.rule_exemptions x \
              JOIN perm.rules r ON r.id = x.rule_id \
             WHERE x.kind = 'role' \
               AND (r.selector_kind = 'tag' OR r.catalog_id = ANY($1)))",
    )
    .bind(&catalogs)
    .bind(service_id)
    .fetch_one(&mut *conn)
    .await
    .map_err(CompileError::from_db)?;

    let ws_grants: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT g.id) FROM perm.grants g \
           LEFT JOIN perm.grant_principals p ON p.grant_id = g.id AND p.kind = 'workspace_members' \
          WHERE g.catalog_id = ANY($1) \
            AND (g.origin = 'workspace_assignment' OR p.grant_id IS NOT NULL)",
    )
    .bind(&catalogs)
    .fetch_one(&mut *conn)
    .await
    .map_err(CompileError::from_db)?;

    let unmigrated_clearance_tags: Vec<String> = sqlx::query_scalar(
        "SELECT t.key FROM public.classification_policies cp \
           JOIN public.tags t ON t.id = cp.tag_id \
          WHERE (cardinality(cp.required_roles) > 0 OR cardinality(cp.required_groups) > 0) \
            AND NOT EXISTS ( \
              SELECT 1 FROM perm.rules r \
               WHERE r.kind = 'withhold' AND r.name = 'withhold:' || t.key) \
          ORDER BY t.key",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(CompileError::from_db)?;
    // NB: use clearance_migrate::withhold_rule_name(&t.key) for the exact name
    // rather than the inline 'withhold:' || t.key — verify its real format.

    Ok(ep_gate(&mig, role_refs, ws_grants, unmigrated_clearance_tags))
}
```

`ep_gate_via_tx` is the same body reading through `&mut **tx`. Factor the shared logic into a private `async fn gate_inputs(conn: &mut PgConnection, service_id) -> Result<(EpMigrationStatus, i64, i64, Vec<String>), CompileError>` so both entries call it with `conn` / `&mut **tx`.

**Verify before trusting:** the exact `CompileError` constructors (`from_db`/`from_store`/`from`), `catalogs_for_service_via`'s return field name, `tags` column name (`key` vs `name`), `classification_policies` column names, `withhold_rule_name`'s format, and that `perm.grants.origin`/`origin_workspace_id` exist. Fix the queries to match.

- [ ] **Step 4: Run the DB test, verify it passes** (on a throwaway Postgres: `docker run -d --name ep-gate-pg -e POSTGRES_USER=egret -e POSTGRES_PASSWORD=egret -e POSTGRES_DB=egret -p 5468:5432 postgres:16`, then `DATABASE_URL=postgres://egret:egret@localhost:5468/egret`).

Expected: PASS. Also run `cargo clippy -p egret-perm-compile --all-targets -- -D warnings`.

- [ ] **Step 5: Commit.**

```bash
git add crates/egret-perm-compile/src/ep_gate.rs
git commit -m "feat(perm): read the per-EP migration gate inputs from the EP's rows (SP3d)"
```

---

### Task 3: `GET /perm/policies/services/{id}/migration-gate` endpoint

**Files:**
- Modify: `crates/egret-perm-compile/src/routes.rs` (route registration ~:201; new handler modeled on `service_migration_status` ~:5137; add `GateVerdict`/`GateRefusal` to the openapi `components`/`paths`)
- Modify: `crates/egret-server/.../openapi.json` (regenerated, not hand-edited)
- Modify: `frontend/src/api/**` generated client + digest (regenerated)

**Interfaces:**
- Consumes: `crate::ep_gate::{ep_gate_via, GateVerdict}`, `PermState`, `Auth`, `PermHttpError`, `egret_core::authz::require`.
- Produces: HTTP `GET /perm/policies/services/{service_id}/migration-gate → 200 Json<GateVerdict>`.

- [ ] **Step 1: Write the failing route test** in `crates/egret-perm-compile/tests/routes.rs` (model on the existing `service_migration_status` test if present; otherwise on a nearby `GET` test). Seed an EP with a role grant → assert 200 and `body["complete"] == false` with a `role_principals` refusal; require auth (`policy:read`). Use the serial + own-data pattern.

- [ ] **Step 2: Run it, verify it fails** (route 404).

Run: `CARGO_TARGET_DIR=target-sp3d DATABASE_URL=<throwaway> cargo test -p egret-perm-compile --test routes migration_gate`
Expected: FAIL (404 / handler missing).

- [ ] **Step 3: Implement the handler + register the route + annotate openapi.** Copy `service_migration_status` verbatim, rename, change the body to `ep_gate_via`, and register the route next to `.../migration-status`:

```rust
#[utoipa::path(
    get,
    path = "/perm/policies/services/{service_id}/migration-gate",
    params(("service_id" = i64, Path, description = "The enforcement point (profile) id")),
    responses(
        (status = 200, description = "Whether the EP's migration is complete, and if not, why", body = crate::ep_gate::GateVerdict),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:read permission", body = ErrorBody),
        (status = 404, description = "No such policy profile", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn service_migration_gate(
    State(st): State<PermState>,
    Auth(p): Auth,
    Path(service_id): Path<i64>,
) -> Result<Json<crate::ep_gate::GateVerdict>, PermHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:read").await?;
    let mut conn = st.perm.pool().acquire().await.map_err(PermHttpError::db)?;
    Ok(Json(crate::ep_gate::ep_gate_via(&mut conn, service_id).await?))
}
```

Register in the router builder beside the `migration-status` route, add the handler to the `#[openapi(paths(...))]` list and `GateVerdict`/`GateRefusal` to `components(schemas(...))`. Map `CompileError → PermHttpError` the way the crate already does (verify `?` works, else add a `From`).

- [ ] **Step 4: Regenerate openapi + client, run the drift gates.**

```bash
CARGO_TARGET_DIR=target-sp3d cargo run -p egret-server --bin egret -- openapi > crates/egret-server/.../openapi.json   # exact path/command per repo
cd frontend && npm run gen:api            # never bare orval — it strips encodeURIComponent
```

Run the route test + `committed_openapi_json_is_current` + the client-digest check. Expected: PASS.

- [ ] **Step 5: Commit.**

```bash
git add crates/egret-perm-compile/src/routes.rs crates/egret-server/**/openapi.json frontend/src/api frontend/**/*digest*
git commit -m "feat(perm): expose GET services/{id}/migration-gate (SP3d)"
```

---

### Task 4: Persist `migration_complete` on `perm.compile_runs` at the compile seam

**Files:**
- Create: `migrations/0228_compile_run_migration_complete.sql`
- Modify: `crates/egret-perm-compile/src/compile_runs.rs` (`record_compile_ok_in_tx`, `CompileRun`)
- Modify: `crates/egret-perm-compile/src/trigger.rs` (`recompile_locked_service_in_tx` seam ~:1270)

**Interfaces:**
- Consumes: `crate::ep_gate::ep_gate_via_tx`.
- Produces: `perm.compile_runs.migration_complete boolean` (nullable: NULL = not yet computed), set on every successful compile.

- [ ] **Step 1: Write the migration.**

```sql
-- SP3d: the per-EP migration-complete verdict, recomputed on each compile.
-- Nullable: NULL means "not computed since this column was added".
ALTER TABLE perm.compile_runs ADD COLUMN migration_complete boolean;
COMMENT ON COLUMN perm.compile_runs.migration_complete IS
  'SP3d: true when the EP has no un-migrated operator policy, role/workspace principal, or gated clearance. Recomputed each compile.';
```

- [ ] **Step 2: Write the failing DB test** in `compile_runs.rs` (or `trigger.rs` tests): compile an EP carrying a `kind='role'` grant → row `migration_complete = Some(false)`; migrate the role input + recompile → `Some(true)`. Serial + own-EP scoped.

- [ ] **Step 3: Run it, verify it fails** (column/field missing).

Run: `CARGO_TARGET_DIR=target-sp3d DATABASE_URL=<throwaway> cargo test -p egret-perm-compile migration_complete`
Expected: FAIL.

- [ ] **Step 4: Thread the verdict through.** In `recompile_locked_service_in_tx`, after `apply_diff_in_tx` and before/at the `record_compile_ok_in_tx` call, compute the verdict and pass its `complete` in. The gate must **not** abort the compile — on an error computing the gate, log and store `NULL` rather than failing (the compile already succeeded). Change `record_compile_ok_in_tx` to take `migration_complete: Option<bool>` and set it in both the INSERT column list and the `ON CONFLICT DO UPDATE SET`. Add `migration_complete: Option<bool>` to `CompileRun`.

```rust
// in recompile_locked_service_in_tx, replacing the record_compile_ok_in_tx call:
let migration_complete = match crate::ep_gate::ep_gate_via_tx(tx, service_id).await {
    Ok(v) => Some(v.complete),
    Err(e) => {
        tracing::warn!(service_id, error = %e, "migration gate verdict skipped; compile stands");
        None
    }
};
crate::compile_runs::record_compile_ok_in_tx(tx, service_id, &summary, migration_complete).await?;
```

Update the INSERT to add `migration_complete` (bind `$7`) and the `DO UPDATE SET` to `migration_complete = EXCLUDED.migration_complete`, and update the `CompileRun` `SELECT` in `failed_compile_runs_via` only if it needs the column (it does not). Update any other caller of `record_compile_ok_in_tx` (search the workspace) to pass the new arg.

- [ ] **Step 5: Run the test, verify it passes.** Also `cargo test -p egret-perm-compile compile_runs` to confirm the existing `compile_runs_refuses_...` test still passes, plus `cargo clippy`.

- [ ] **Step 6: Commit.**

```bash
git add migrations/0228_compile_run_migration_complete.sql crates/egret-perm-compile/src/compile_runs.rs crates/egret-perm-compile/src/trigger.rs
git commit -m "feat(perm): recompute and persist the migration gate on each compile (SP3d)"
```

---

### Task 5: Surface the gate in the Governance overview + docs

**Files:**
- Modify: `crates/egret-perm-compile/src/compile_runs.rs` (a `migration_incomplete_count_via` reader) + the governance-overview assembler that already uses `failed_compile_runs_via` (find it: `rg "failed_compile_runs_via" crates/`)
- Modify: the overview response struct + its openapi + regenerated client
- Create: `egret-data/docs/.../sp3d-migration-gate.md` (+ the bilingual counterpart), linked from the Platform governance section

**Interfaces:**
- Consumes: `perm.compile_runs.migration_complete`.
- Produces: an overview field `migration_incomplete: usize` (count of EPs whose last compile has `migration_complete = false`).

- [ ] **Step 1: Write the failing test** for `migration_incomplete_count_via` (and the overview field if the overview has an integration test): with one incomplete + one complete EP, the count is scoped and correct. Serial + own data.

- [ ] **Step 2: Run it, verify it fails.**

Run: `CARGO_TARGET_DIR=target-sp3d DATABASE_URL=<throwaway> cargo test -p egret-perm-compile migration_incomplete`
Expected: FAIL.

- [ ] **Step 3: Implement the reader + wire it into the overview.**

```rust
/// EPs whose last compile recorded an incomplete migration (SP3d).
pub async fn migration_incomplete_count_via(conn: &mut PgConnection) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT count(*) FROM perm.compile_runs WHERE migration_complete = false",
    )
    .fetch_one(conn)
    .await
}
```

Add the field to the overview response struct, set it from this reader where `failed_compile_runs_via` is already called, and add it to the openapi schema. Regenerate openapi + client. (A frontend badge rendering this number, the mechanism switch, live checks, and screenshots are the deferred follow-up — do not build them here.)

- [ ] **Step 4: Run the tests + drift gates** (`committed_openapi_json_is_current`, client digest, `cargo clippy`, `cargo fmt --check`).

- [ ] **Step 5: Write the docs** (bilingual): what "migration complete" means per EP, the four conditions, the `GET .../migration-gate` endpoint and its `GateVerdict`/`GateRefusal` shape, and that the overview now counts incomplete EPs. Note the deferred follow-up (switch + live checks). Tick the Docs box in the PR.

- [ ] **Step 6: Commit.**

```bash
git add crates/egret-perm-compile/src/compile_runs.rs crates/<overview>.rs crates/egret-server/**/openapi.json frontend/src/api frontend/**/*digest* egret-data/docs/...
git commit -m "feat(perm): count EPs with an incomplete migration in the governance overview (SP3d)"
```

---

## Self-Review

- **Spec coverage:** SP3d gate = "no unconvertible operator policy (SP3b) AND role/workspace/clearance input migrated (SP3c), read over the EP's own rows, re-checked on each compile" → Task 1 (verdict), Task 2 (the four reads), Task 4 (re-checked + persisted each compile). Surface → Task 3 (endpoint) + Task 5 (overview). Out-of-scope (switch, live checks, screenshots, legacy delete) explicitly excluded in Global Constraints.
- **Type consistency:** `GateVerdict`/`GateRefusal`/`ep_gate` defined in Task 1 and used unchanged in Tasks 2–4; `record_compile_ok_in_tx` gains `migration_complete: Option<bool>` in Task 4 and every caller is swept there.
- **Known verification debt the implementer must close (flagged, not placeheld):** exact `CompileError` constructors, `catalogs_for_service_via` field name, `tags`/`classification_policies` column names, `withhold_rule_name` format, the openapi-regen command/path, and the overview assembler location. Each task says "verify against the live schema" with the `rg` to run.
