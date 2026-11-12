# Governance SP3b — Per-EP migration-completeness status Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expose, per enforcement point, whether any operator (manual) policy has not yet been migrated into the permission model — the signal SP3d's per-EP gate will consume and the badge the operator reads on the Migrate tab.

**Architecture:** The Migrate flow (classify operator policy → grant/rule, `perm.migrated_from` provenance, undo with hash guards, equivalence-gate probes) already exists from SP4d-3. SP3b adds **one pure rollup** over the existing classifier's output — `ep_migration_status(&[ClassifiedPolicy])` — surfaced through a new read-only endpoint and a status badge on the Migrate tab. It migrates nothing, changes no compiled artifact, and adds no new classification logic.

**Tech Stack:** Rust (axum, utoipa, sqlx) in `egret-perm-compile`; React/TypeScript (orval-generated client) in `frontend`.

## Global Constraints

- **Builds on SP3a**, which is unmerged (PR #854). Branch off `feat/gov-sp3a-ep-framing`, not `main`. (See memory [[branch-off-unmerged-feature]].)
- **Additive and read-only.** SP3b reports status only; it migrates nothing. The compiled Ranger/OPA artifact, `explain`, and every golden are **unchanged** — no fixture is re-blessed in this package. If a golden changes, something is wrong.
- **Decision ② (no speculative typed privileges):** the demo stack has exactly one operator policy — a plain access policy (`legacy: analysts read finance_pg`, catalog `finance_pg`, `select`, user `member-sso`) that the existing classifier already converts to a grant. **No operator policy has an engine-capability shape, so SP3b builds NO operator-policy → typed-engine-privilege mapping.** That mapping is deferred until a real such shape appears (noted in the spec's SP3b scope).
- **Only `operator` policies count.** `compiler` and `infra` rows are egret's own and are never migration candidates (the classifier already refuses them); the rollup skips them.
- **`complete` is fail-closed:** an EP is complete only when *every* operator policy is migrated (`pending == 0` and no `blocked`). A blocked (unconvertible) or pending operator policy both leave the EP incomplete.
- **Reuse the existing classifier** (`classify::classify_service`); do not add or duplicate classification logic.
- **`ep_migration_status` is the pure function SP3d's per-EP compile gate will call.** SP3b exposes it; SP3b does **not** wire it into the compile gate (that is SP3d).
- CI gates that must stay green: `cargo fmt --check`, `cargo clippy`, `committed_openapi_json_is_current`, the generated-client digest test, and the DB-gated Rust tests.
- Build/test with a worktree-local target dir: `CARGO_TARGET_DIR=<worktree>/target-sp3b` (shared target across worktrees causes E0432 — memory [[shared-cargo-target-across-checkouts]]).
- Commit co-author trailer: `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Plain `git commit` (configured identity — memory [[git-commit-identity]]). Stage only changed files; never push.
- User-visible change (the badge) needs a docs update with a screenshot (`egret-data/docs`, Platform section).

---

## File Structure

- `crates/egret-perm-compile/src/classify.rs` — add `EpMigrationStatus`, `BlockedPolicy`, and the pure `ep_migration_status` rollup beside the existing classification types. One responsibility (classification + its rollups) already lives here.
- `crates/egret-perm-compile/src/routes.rs` — add the `service_migration_status` handler, its `#[utoipa::path]`, route registration, and the two schemas in the `ApiDoc`.
- `openapi.json` (repo root) — regenerated, committed.
- `frontend/src/api/generated/**` — regenerated client (orval), committed.
- `frontend/src/routes/governance/engines/tabs/MigrateTab.tsx` — the status badge.
- `frontend/src/routes/governance/engines/__tests__/MigrateTab.test.tsx` — badge tests.

---

### Task 1: The `ep_migration_status` rollup (backend, pure)

**Files:**
- Modify: `crates/egret-perm-compile/src/classify.rs` (add types + function near `ProposedObjects`, around line 158)
- Test: in the `#[cfg(test)]` module of `crates/egret-perm-compile/src/classify.rs`

**Interfaces:**
- Consumes: `ClassifiedPolicy` (existing, `classify.rs:73`) — fields `id: i64`, `name: String`, `managed_by: String`, `migratable: bool`, `migrated: Option<MigrationRecord>`, `blockers: Vec<String>`.
- Produces: `pub fn ep_migration_status(policies: &[ClassifiedPolicy]) -> EpMigrationStatus`; `pub struct EpMigrationStatus { complete: bool, migrated: usize, pending: usize, blocked: Vec<BlockedPolicy> }`; `pub struct BlockedPolicy { id: i64, name: String, reason: String }`.

- [ ] **Step 1: Write the failing tests**

Add to the `classify.rs` test module. These use a small builder so the cases read clearly.

```rust
#[cfg(test)]
mod ep_status_tests {
    use super::*;

    fn pol(id: i64, managed_by: &str, migratable: bool, migrated: bool, blocker: Option<&str>) -> ClassifiedPolicy {
        ClassifiedPolicy {
            id,
            name: format!("p{id}"),
            managed_by: managed_by.to_string(),
            enabled: true,
            migratable,
            blockers: blocker.into_iter().map(|s| s.to_string()).collect(),
            proposed: None,
            migrated: migrated.then(|| MigrationRecord {
                grant_ids: vec![],
                rule_ids: vec![],
                policy_name: format!("p{id}"),
                migrated_by: "someone".into(),
                migrated_at: "2026-11-13T00:00:00Z".into(),
            }),
            warnings: vec![],
        }
    }

    #[test]
    fn empty_profile_is_complete() {
        let s = ep_migration_status(&[]);
        assert!(s.complete);
        assert_eq!((s.migrated, s.pending, s.blocked.len()), (0, 0, 0));
    }

    #[test]
    fn one_pending_operator_policy_is_incomplete() {
        // The demo-stack shape: operator, convertible, not yet migrated.
        let s = ep_migration_status(&[pol(1, "operator", true, false, None)]);
        assert!(!s.complete);
        assert_eq!((s.migrated, s.pending, s.blocked.len()), (0, 1, 0));
    }

    #[test]
    fn one_migrated_operator_policy_is_complete() {
        let s = ep_migration_status(&[pol(1, "operator", false, true, None)]);
        assert!(s.complete);
        assert_eq!((s.migrated, s.pending, s.blocked.len()), (1, 0, 0));
    }

    #[test]
    fn a_blocked_operator_policy_is_listed_and_blocks_completeness() {
        let s = ep_migration_status(&[pol(7, "operator", false, false, Some("Clearance is not expressible here."))]);
        assert!(!s.complete);
        assert_eq!(s.blocked.len(), 1);
        assert_eq!(s.blocked[0].id, 7);
        assert_eq!(s.blocked[0].reason, "Clearance is not expressible here.");
    }

    #[test]
    fn compiler_and_infra_rows_are_ignored() {
        let s = ep_migration_status(&[
            pol(1, "compiler", false, false, Some("compiler-owned")),
            pol(2, "infra", false, false, Some("infra-owned")),
            pol(3, "operator", false, true, None),
        ]);
        assert!(s.complete);
        assert_eq!((s.migrated, s.pending, s.blocked.len()), (1, 0, 0));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `CARGO_TARGET_DIR=$PWD/target-sp3b cargo test -p egret-perm-compile ep_status_tests 2>&1 | tail -20`
Expected: FAIL — `cannot find function ep_migration_status` / `cannot find type EpMigrationStatus`.

- [ ] **Step 3: Add the types and the function**

Insert after `ProposedObjects` (around `classify.rs:158`). If `MigrationRecord`'s field set differs from the test builder above, align the builder to the real struct (it is defined in this same file, `classify.rs:131`) — the real struct wins.

```rust
/// One operator policy that cannot be migrated, for the per-EP rollup.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
pub struct BlockedPolicy {
    pub id: i64,
    pub name: String,
    /// The first blocker sentence from classification, for the operator.
    pub reason: String,
}

/// An enforcement point's migration standing, rolled up from its classified
/// policies. Only `operator` policies count — `compiler`/`infra` rows are
/// egret's own and never migration candidates. This is the signal SP3d's
/// per-EP gate consumes: `complete` is true only when every operator policy is
/// migrated (fail-closed — any pending or blocked policy leaves it false).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
pub struct EpMigrationStatus {
    pub complete: bool,
    /// Operator policies already migrated (recorded in `perm.migrated_from`).
    pub migrated: usize,
    /// Operator policies that are convertible but not yet migrated.
    pub pending: usize,
    /// Operator policies that cannot be migrated at all; each blocks
    /// completeness until the operator removes or rewrites it.
    pub blocked: Vec<BlockedPolicy>,
}

/// Roll up a profile's classified policies into its migration status.
/// A migrated operator policy is never `migratable` (see [`ClassifiedPolicy`]),
/// so `migrated` is checked first.
pub fn ep_migration_status(policies: &[ClassifiedPolicy]) -> EpMigrationStatus {
    let mut migrated = 0usize;
    let mut pending = 0usize;
    let mut blocked = Vec::new();
    for p in policies {
        if p.managed_by != "operator" {
            continue;
        }
        if p.migrated.is_some() {
            migrated += 1;
        } else if p.migratable {
            pending += 1;
        } else {
            blocked.push(BlockedPolicy {
                id: p.id,
                name: p.name.clone(),
                reason: p
                    .blockers
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "This policy cannot be migrated.".to_string()),
            });
        }
    }
    EpMigrationStatus {
        complete: pending == 0 && blocked.is_empty(),
        migrated,
        pending,
        blocked,
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `CARGO_TARGET_DIR=$PWD/target-sp3b cargo test -p egret-perm-compile ep_status_tests 2>&1 | tail -20`
Expected: PASS (5 tests).

- [ ] **Step 5: fmt + clippy**

Run: `CARGO_TARGET_DIR=$PWD/target-sp3b cargo fmt --check && CARGO_TARGET_DIR=$PWD/target-sp3b cargo clippy -p egret-perm-compile 2>&1 | tail -15`
Expected: no diff, no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-perm-compile/src/classify.rs
git commit -m "feat(perm-compile): roll a profile's operator policies up into a per-EP migration status

ep_migration_status partitions a profile's classified operator policies into
migrated / pending / blocked and reports complete only when every one is
migrated. Pure over the existing classifier output; compiler/infra rows are
skipped. This is the signal SP3d's per-EP gate will consume.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: The `migration-status` endpoint

**Files:**
- Modify: `crates/egret-perm-compile/src/routes.rs` (handler + `#[utoipa::path]` near the other classify handlers ~`routes.rs:5039`; route registration ~`routes.rs:199`; `ApiDoc` paths list ~`routes.rs:254` and schemas ~`routes.rs:256`)
- Modify (generated): `openapi.json`, `frontend/src/api/generated/**`

**Interfaces:**
- Consumes: `crate::classify::classify_service(&mut conn, service_id)` (existing), `crate::classify::ep_migration_status` (Task 1).
- Produces: `GET /api/control-plane/perm/policies/services/{service_id}/migration-status` → `EpMigrationStatus`.

- [ ] **Step 1: Add the handler + utoipa annotation**

Insert after `service_policy_warnings` (after `routes.rs:5110` area — place it next to the other read handlers). Mirror `classify_service_policies` exactly (same auth, same pool pattern).

```rust
/// SP3b: the enforcement point's migration standing — how many of its operator
/// policies are migrated, pending or blocked, and whether it is complete (every
/// operator policy migrated). Read-only; rolled up from the same classification
/// `GET .../classify` returns.
#[utoipa::path(
    get,
    path = "/api/control-plane/perm/policies/services/{service_id}/migration-status",
    tag = "perm",
    params(("service_id" = i64, Path, description = "policy.authz_services id")),
    responses(
        (status = 200, description = "The profile's migration standing: migrated/pending counts, the blocked operator policies, and whether it is complete", body = crate::classify::EpMigrationStatus),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:read permission", body = ErrorBody),
        (status = 404, description = "No such policy profile", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn service_migration_status(
    State(st): State<PermState>,
    Auth(p): Auth,
    Path(service_id): Path<i64>,
) -> Result<Json<crate::classify::EpMigrationStatus>, PermHttpError> {
    egret_core::authz::require(&*st.pdp, &p, "policy:read").await?;
    let mut conn = st.perm.pool().acquire().await.map_err(PermHttpError::db)?;
    let classified = crate::classify::classify_service(&mut conn, service_id).await?;
    Ok(Json(crate::classify::ep_migration_status(&classified)))
}
```

- [ ] **Step 2: Register the route**

In `router()`, after the `warnings` route (`routes.rs:199`), add:

```rust
        .route(
            "/perm/policies/services/{service_id}/migration-status",
            get(service_migration_status),
        )
```

- [ ] **Step 3: Register in the OpenAPI doc**

In the `ApiDoc` `paths(...)` list (`routes.rs:254`), add `service_migration_status` after `service_policy_warnings`. In `components(schemas(...))`, add:

```rust
        crate::classify::EpMigrationStatus,
        crate::classify::BlockedPolicy,
```

- [ ] **Step 4: Build + regenerate openapi.json**

Run (from the repo root; this is the project's documented way to refresh the committed OpenAPI — check `justfile`/`Makefile`/`xtask` for the exact recipe, e.g. `just openapi` or `cargo xtask openapi`):

```bash
CARGO_TARGET_DIR=$PWD/target-sp3b cargo build -p egret-perm-compile
# then the repo's openapi-regen recipe, which rewrites ./openapi.json
```

Then verify the gate the way CI does:

```bash
CARGO_TARGET_DIR=$PWD/target-sp3b cargo test committed_openapi_json_is_current 2>&1 | tail -5
```
Expected: PASS, and `git diff --stat openapi.json` shows the new path added.

- [ ] **Step 5: Regenerate the typed client**

Run (never bare orval — memory [[orval-regeneration-strips-url-encoding]]):

```bash
cd frontend && npm run gen:api && cd ..
```
Expected: `frontend/src/api/generated/perm/` gains a `serviceMigrationStatus` hook and `frontend/src/api/generated/model/` gains `epMigrationStatus.ts` + `blockedPolicy.ts`. Verify the generated-client digest test the way CI does (find it: `grep -ril "digest" frontend/ | head`), then run it.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-perm-compile/src/routes.rs openapi.json frontend/src/api/generated
git commit -m "feat(perm-compile): add the per-EP migration-status endpoint

GET .../services/{service_id}/migration-status returns the EP's migration
standing (migrated/pending/blocked, complete). Read-only; rolls up the existing
classification. OpenAPI + generated client regenerated.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: The migration-complete badge on the Migrate tab

**Files:**
- Modify: `frontend/src/routes/governance/engines/tabs/MigrateTab.tsx`
- Test: `frontend/src/routes/governance/engines/__tests__/MigrateTab.test.tsx`

**Interfaces:**
- Consumes: the generated `serviceMigrationStatus` query hook (Task 2) and its `EpMigrationStatus` model.
- Produces: a status badge rendered at the top of the Migrate tab.

- [ ] **Step 1: Read the existing tab**

Read `MigrateTab.tsx` and its test fully. Note how it already fetches the classify list and which query-hook + mock pattern the test uses, so the badge follows the same data-loading and test conventions (do not introduce a new fetch style).

- [ ] **Step 2: Write the failing tests**

Add to `MigrateTab.test.tsx`, mocking `serviceMigrationStatus` the same way the file mocks the classify query. Three cases:

```tsx
// complete → a "Migration complete" badge
it("shows migration complete when the EP has no remaining operator policy", async () => {
  // mock serviceMigrationStatus → { complete: true, migrated: 1, pending: 0, blocked: [] }
  // render MigrateTab; assert /migration complete/i is shown
});

// pending → shows the count still to migrate
it("shows the count still to migrate", async () => {
  // mock → { complete: false, migrated: 0, pending: 1, blocked: [] }
  // assert text /1 .*to migrate/i
});

// blocked → lists the blocked policy name + reason
it("lists a blocked operator policy with its reason", async () => {
  // mock → { complete: false, migrated: 0, pending: 0, blocked: [{ id: 7, name: "legacy x", reason: "Clearance is not expressible here." }] }
  // assert the name and the reason text appear
});
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cd frontend && npx vitest run src/routes/governance/engines/__tests__/MigrateTab.test.tsx 2>&1 | tail -20`
Expected: FAIL — the badge is not rendered yet.

- [ ] **Step 4: Render the badge**

At the top of the tab's content, call the `serviceMigrationStatus` hook for the current `service_id` and render:
- `complete`: a success badge "Migration complete — every manual policy is in the model".
- otherwise: a warning badge "N to migrate" (from `pending`) and, when `blocked.length > 0`, "M blocked" with a small list of `name — reason`.

Follow the existing badge/alert component the governance pages already use (match `WarningsTab`/the warnings surface if present); do not add a new UI primitive.

- [ ] **Step 5: Run the tests + typecheck**

Run: `cd frontend && npx vitest run src/routes/governance/engines/__tests__/MigrateTab.test.tsx 2>&1 | tail -20 && npx tsc --noEmit 2>&1 | tail -20`
Expected: PASS, no type errors.

- [ ] **Step 6: Live check**

With the demo stack up (admin@egret.local / Admin12345!, per memory [[egret-frontend-viz-verify]]), open the governance engines page → the `docs-legacy-trino` EP → Migrate tab. Confirm the badge reads "1 to migrate" (the one `legacy: analysts read finance_pg` operator policy). Screenshot to `data-portal/` for the docs task.

- [ ] **Step 7: Commit**

```bash
git add frontend/src/routes/governance/engines/tabs/MigrateTab.tsx frontend/src/routes/governance/engines/__tests__/MigrateTab.test.tsx
git commit -m "feat(ui): show a per-EP migration-complete badge on the Migrate tab

The badge reads the migration-status endpoint and tells the operator whether
the EP's manual policies are all in the model, how many remain, and which are
blocked and why.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Docs

**Files:**
- Modify: `egret-data/docs` — the Platform governance / enforcement-points page that covers the Migrate tab.

- [ ] **Step 1: Document the badge**

Add a short subsection under the Migrate tab docs: what "Migration complete" means (every operator policy migrated into the model), what "N to migrate" and "M blocked" mean, and that a blocked policy keeps the EP incomplete until the operator removes or rewrites it. Note (one line) that completeness is the signal the per-EP switch will use (forward reference to SP3d). Use the Task 3 screenshot. Every code fence gets a language.

- [ ] **Step 2: Commit (docs repo)**

```bash
git commit -m "docs(governance): the per-EP migration-complete badge on the Migrate tab

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Out of scope

- Operator-policy → typed-engine-privilege mapping (decision ②: nothing to map on the demo stack; deferred until a real such shape appears).
- Wiring `ep_migration_status` into the compile gate and switching the demo stack (SP3d).
- clearance → withhold, role → groups, implicit → explicit workspace grant (SP3c).
- Any change to migration execution, undo, the equivalence-gate probes, or the compiled artifact.

## Self-Review

1. **Spec coverage (SP3b "Done"):** "convertible policies become grant/rule/engine-privilege" → grant/rule already shipped (SP4d-3); engine-privilege has nothing to map (decision ②, documented). "unconvertible listed per-EP" → Task 1 `blocked` list + Task 3 badge. "EP migration not complete while any remain" → `complete` flag (Task 1), consumed by SP3d. "`explain` per-EP unchanged" → SP3b migrates nothing; compiled artifact and goldens untouched (Global Constraints). ✓
2. **Placeholder scan:** the openapi-regen recipe and the generated-client digest test name are the two "find the exact command" spots — both are discoverable one-liners with the grep given, not left as TODO. ✓
3. **Type consistency:** `ep_migration_status(&[ClassifiedPolicy]) -> EpMigrationStatus` used identically in Task 1 (defn), Task 2 (handler). `EpMigrationStatus`/`BlockedPolicy` field names match across Rust, OpenAPI schema registration, and the FE test mocks. The Task 1 test builder's `MigrationRecord` fields must be aligned to the real struct at `classify.rs:131` (called out in Step 3). ✓
