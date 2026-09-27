# SP4b-1 — the backend the permission UI needs

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task.

**Goal:** give SP4b's UI two things SP4a's surface cannot express — an edit that is really an edit, and a way to see a change's blast radius before committing it.

**Spec:** `docs/en/specs/2026-09-27-permission-ui-sp4b-design.md`, decisions D1 and D2.

**Architecture:** both additions go through the compile path that already exists (`recompile_service_in_tx`), never a parallel one. The preview runs the *real* write inside a transaction it then rolls back, so a preview and the write it previews cannot disagree.

**Tech stack:** Rust workspace (axum, sqlx 0.9 — `query()` takes `&'static str`, never `format!` SQL), utoipa OpenAPI, orval client.

## Global Constraints

- **Build isolation is mandatory.** `export CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret-sp4a/target-check`. Never `.../egret/target`: another session builds the MAIN checkout there, so cargo links the wrong artefacts and reports `E0432 no X in the root` for symbols that plainly exist — a failure that looks exactly like a broken branch and is not one.
- **Never launch a background task and wait on it.** Six agents on this programme stalled doing exactly that and delivered nothing. Foreground only.
- sqlx 0.9: `query()` takes `&'static str`. No `format!`-built SQL.
- `COMPILER_GRANT_PREFIX` / `COMPILER_RULE_PREFIX` are defined **once** in `egret-policy/src/store.rs`. Never redefine those strings.
- OpenAPI must satisfy `committed_openapi_json_is_current`; the frontend client must be exactly `npm run gen:api` output — never bare `orval` — committed exactly as produced.
- `cargo build --workspace` and `cargo fmt --all -- --check` clean.
- Tests: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test` only — **never** the dev database `egret`. DB-gated tests skip silently without it, so always `env -u EGRET_ALLOW_DB_SKIP`. Run with `RUST_TEST_THREADS=4`: at default parallelism these suites exhaust Postgres connections and fail ~8 tests with `PoolTimedOut`, which is not behaviour. If Postgres is not running, `just up` first.
- **The established locking discipline**, which both tasks must follow: affected services are expanded through `services_and_tag_services_via` and locked **ascending**, with a `debug_assert!(service_ids.is_sorted())` at that gate. The expansion happens *before* the sort because a tag service's id can be lower than its profile's (observed live: 51 vs 52).
- The sweep must keep sourcing existing state from `policy.authz_policies` scoped by `service_id` plus the two compiler prefixes — **never** through the cascading `perm.compiled`. That was an SP4a Critical and must not regress.
- Every new test is verified by mutation: break the fix, watch that specific test die, restore with `cat backup > file` (`cp` is aliased to `cp -i` and silently refuses to overwrite), confirm `git status --porcelain` is clean. **Check *why* each mutation died** — a mutation dying for the wrong reason proves nothing, which has happened three times on this programme.

---

### Task 1: `PATCH` a grant and a rule, keeping the id

**Files:** Modify `crates/egret-perm-compile/src/routes.rs`, `crates/egret-perm/src/store.rs`.

**Why the id must survive:** an `explain` reason carries a `grant_id`, and the UI links from that reason to the grant. Delete-and-recreate breaks every such link on every edit, and costs two recompiles plus a window where the row does not exist.

- [ ] `PATCH /perm/grants/{id}` and `PATCH /perm/rules/{id}`, gated `policy:update`, taking the same body shape as the create routes. `404` for an unknown id.
- [ ] Run the **same** validation the create path runs (`validate_grant_shape` / `validate_rule_shape`) before anything is written. A patch that can produce an uncompilable row is the defect SP4a closed at create; do not reopen it at update.
- [ ] **THE SUBTLE PART — recompile the union of the OLD and NEW affected sets.** A patch can move a grant from one catalog to another. Recompiling only the new set leaves the old profile still serving the old policy, which is exactly the "a deleted grant's policy enforced forever" Critical SP4a fixed, reintroduced through a new door. Read the old row's affected set **before** the write, union it with the new one, expand and lock ascending, then recompile once for the whole union in the same transaction as the write.
- [ ] A compile failure must fail the request and roll the patch back. A stored-but-uncompiled grant is worse than a rejected one, because the UI will show it as active.
- [ ] Tests, and the third is the one that matters:
  - patching a grant's principals recompiles and the policy body changes, id unchanged
  - patching an uncompilable shape is refused and nothing is stored
  - **patching a grant's `catalog_id` reaps the policy from the OLD profile and creates it in the new one** — assert against `policy.authz_policies` for both services, not against the grant row. A test that checks only the new profile passes against the bug.

### Task 2: A preview that cannot disagree with the write

**Files:** Modify `crates/egret-perm-compile/src/routes.rs`.

**The design constraint that decides this task:** do **not** compute the affected set with a second, read-only implementation. Two implementations of "which profiles does this touch" will drift, and a preview that lies is worse than no preview — the user would trust it. Instead run the real create/patch inside a transaction and **roll it back**, returning what it would have done.

- [ ] `POST /perm/preview`, gated `policy:update` — it previews a write, it runs real compiler work, and it is useless to someone who cannot write. Accepts the same body as a create, plus an optional id to preview a patch instead.
- [ ] Returns the affected profiles (id and name, so the UI can say "Trino-prod" rather than a uuid) and, if the compile fails, the same error the write would have returned — including the ceiling messages, which already name the rule, the profile and the running total.
- [ ] Writes nothing. Not a policy, not a `perm` row, not a `policy_version` bump.
- [ ] Tests:
  - a preview of a valid grant reports the affected profiles and stores nothing — assert `policy.authz_policies`, `perm.grants` and the services' `policy_version` are all unchanged afterwards
  - a preview of an uncompilable grant returns the compile error and still stores nothing
  - **a preview and the real write agree**: preview a tag rule, then create it, and assert the affected set the preview reported equals the set the write reports. This is the test that protects the "cannot disagree" property; without it the rollback approach has no guard.
  - a preview of a **tag rule** reports more than one profile where more than one Trino service exists — the installation-wide fan-out is the reason this endpoint exists, so it must be visible in a test rather than assumed.

### Task 3: Regenerate the contract

**Files:** `openapi.json`, `frontend/src/api/generated/**`.

- [ ] Regenerate OpenAPI, confirm `committed_openapi_json_is_current` passes.
- [ ] `cd frontend && npm run gen:api` — never bare `orval`, which drops `encodeURIComponent` from generated URLs. Commit exactly what it produces, including any incidental churn, and say in the commit message what changed beyond the new routes.
- [ ] `npx tsc --noEmit` clean. No UI is built in this plan; this task only proves the contract the UI will consume is generated and typed.
