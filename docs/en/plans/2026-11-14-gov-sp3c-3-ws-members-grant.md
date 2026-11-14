# Governance SP3c-3 — Implicit workspace grant → explicit `ws:<slug> members` grant

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the implicit catalog-wide read a workspace member gets today (a `perm.grants` row `origin='workspace_assignment'` with a `kind='workspace_members'` principal) with an **explicit** READ grant to a managed group `ws:<slug> members`, so the enforcement input carries no `workspace_members` principal kind and workspace grants use the one group-based model.

**Architecture:** This is the SP3c-1 pattern (role → `role:<name> holders` group) applied to workspaces. A managed group `ws:<slug> members` (source='managed', created by SP3c-1's `groups.source='managed'`) mirrors the workspace's members; the `workspace_members` principal on each `origin='workspace_assignment'` grant is rewritten to a `kind='group'` ref to that group and the grant's `origin` becomes `explicit`. Because the `workspace_members` kind and a group ref both expand to the same usernames (`members::workspace_members_via` vs `group_members_via`), the compiled output is byte-identical and there is **no fail-open window** (an un-migrated `workspace_members` grant still expands and enforces). Decision ③: `ws:<slug> members` and `role:<name> holders` are both managed groups on the same ws-members machinery.

**Tech Stack:** Rust (sqlx) in `egret-perm-compile` / `egret-perm`; Postgres migrations.

## Global Constraints

- **Depends on SP3c-1** (managed `role:<name> holders` groups + `groups.source='managed'`, merged to main). Branch off `main`. Reuse the SP3c-1 machinery: `role_holder_group.rs` is the template (`sync_role_holder_group_in_tx`, `sync_role_holder_group_if_present_in_tx`, the chokepoint pattern, `refused_for_authoring`, the reversible `role_principal_migrate.rs` + equivalence gate + hash-guarded undo). Independent of SP3c-2 (clearance); do not touch clearance.
- **The implicit grant shape (fixed by code, not measured):** `egret_perm::assignment::sync_assignment_grants_in_tx` (`crates/egret-perm/src/assignment.rs:76`) writes, per bound workspace, one `perm.grants` row `effect='allow'`, `actions=['read']`, `origin='workspace_assignment'`, `origin_workspace_id=<ws>` + a `perm.grant_principals` row `kind='workspace_members'`, `ref=<workspace_id as string>`, `mode='include'`. Partial unique index `grants_one_per_assignment` on `(catalog_id, origin_workspace_id) WHERE origin='workspace_assignment'`. Called from `egret-catalog-registry set_catalog_workspaces_in_tx` atomically with `registry.catalog_workspaces` writes.
- **Managed group name is exactly `ws:<slug> members`** (literal, one space each side of `members`), `<slug>` = `workspaces.slug` verbatim. Membership mirrors `members::workspace_members_via` (`members.rs` — direct `access_grants scope='workspace'` + the group arm).
- **Byte-identical compiled output** for every migrated workspace grant (both principals expand to the same usernames). A golden/equivalence diff that is non-empty is a failure.
- **No fail-open:** the `workspace_members` principal kind STILL expands (not removed from the compiler), so an un-migrated `origin='workspace_assignment'` grant still enforces. The migration + authoring change together move every grant to the group ref; nothing is left unenforced at any point.
- **Transitional (concepts D):** after this ships, no NEW `kind='workspace_members'` principal may be authored and no new ref to a `ws:* members` group may be hand-authored (migration/authoring targets only) — enforce both at the write seam, exactly as SP3c-1 guards `kind='role'` and `role:* holders`.
- **Reversible with provenance** (own table `perm.ws_assignment_migration`, NOT `perm.migrated_from`). Undo restores `origin='workspace_assignment'` + the `workspace_members` principal, hash-guarded (409 on divergence).
- **`explain` unchanged** for every affected principal (a workspace member).
- All `user_groups` writes route through the SP3c-1 `PermGroupRecompiler` chokepoint machinery; a workspace-membership change re-syncs the `ws:<slug> members` group and recompiles.
- CI gates: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `committed_openapi_json_is_current`, generated-client digest, DB-gated Rust tests. **Every DB-gated test that migrates/compiles across catalogs or asserts cross-catalog state takes a process-wide `SERIAL` mutex and scopes its assertions to its own catalog/workspace (NO global `count(*)`), and must pass under DEFAULT `cargo test` parallelism** (learned from SP3c-1/SP3c-2 CI flakes).
- Migration numbers: current max on main is `0221` (SP3c-1's provenance) — but SP3c-2 (open PR #859) uses 0222-0224; **pick the next free number above main AND all open PRs at execution time** and reconcile at merge.
- `CARGO_TARGET_DIR=target-sp3c3`. Commits Conventional, co-author Claude Opus 5.5. Stage only changed files; do not push. (Local Postgres/Docker may be down — CI runs the DB-gated tests.)

## File Structure

- `crates/egret-perm-compile/src/ws_members_group.rs` (new) — `ws_members_group_name(slug)` + `sync_ws_members_group_in_tx` / `_if_present_in_tx` (mirror `role_holder_group.rs`).
- `crates/egret-perm-compile/src/ws_assignment_migrate.rs` (new) — `migrate_ws_assignments_in_tx` / `undo_ws_assignments_in_tx` + `WsAssignmentMigration`, equivalence-gated, hash-guarded undo (mirror `role_principal_migrate.rs`).
- `migrations/NNNN_ws_assignment_migration.sql` (new) — provenance table.
- `crates/egret-perm/src/assignment.rs` (modify) — `sync_assignment_grants_in_tx` now produces the explicit `ws:<slug> members` group grant (and ensures the group), not the `workspace_members` principal.
- `crates/egret-perm-compile/src/<workspace-membership chokepoint>.rs` (modify) — re-sync the `ws:<slug> members` group on a workspace-membership change.
- `crates/egret-perm-compile/src/principal_refs.rs` (modify) — the write guard rejects new `kind='workspace_members'` and new refs to a `ws:* members` group; existing refs still validate.
- `crates/egret-perm-compile/src/routes.rs` (modify) — admin endpoints `POST /perm/migrate/ws-assignments` (+/undo).
- Tests: `crates/egret-perm-compile/tests/ws_assignment_migrate.rs` (new, DB-gated).

---

### Task 1: `ws:<slug> members` managed group — name + holder-mirror sync

**Files:** Create `crates/egret-perm-compile/src/ws_members_group.rs`; Modify `lib.rs`; Test `crates/egret-perm-compile/tests/ws_assignment_migrate.rs`.

**Interfaces:**
- Consumes: `members::workspace_members_via(conn, &[Uuid]) -> Result<Vec<MemberRow>, CompileError>` (`members.rs`; `MemberRow` has `workspace_id`, `user_id: Uuid`, `username`); `workspaces(id, slug, name)`; `public.groups`/`public.user_groups`. Read `crates/egret-perm-compile/src/role_holder_group.rs` as the exact template.
- Produces:
  - `pub fn ws_members_group_name(slug: &str) -> String` → `format!("ws:{slug} members")`.
  - `pub async fn sync_ws_members_group_in_tx(tx, workspace_id: Uuid) -> Result<Uuid, CompileError>` — resolve the workspace's `slug`; upsert the managed group `source='managed'` (`ON CONFLICT (name) DO UPDATE SET source='managed' WHERE groups.source='managed' RETURNING id`, erroring with a name-collision if a non-managed group owns the name — mirror SP3c-1's adoption guard); reconcile `user_groups` to exactly `workspace_members_via([workspace_id])`'s user ids. Idempotent.
  - `pub async fn sync_ws_members_group_if_present_in_tx(tx, workspace_id) -> Result<bool, CompileError>` — syncs only if the managed group already exists (no-op pre-migration), mirroring SP3c-1.

- [ ] **Step 1: Write the sync module** (mirror `role_holder_group.rs` exactly, swapping role→workspace: resolve `slug` from `workspaces`, members from `workspace_members_via`, name from `ws_members_group_name`).

- [ ] **Step 2: Write the failing DB-gated test** `the_ws_members_group_mirrors_the_workspace_members` (take `SERIAL`; scope to the test's own workspace): seed a workspace with slug `s` and one member `u`; call `sync_ws_members_group_in_tx`; assert group `ws:s members`, source `managed`, members `[u]`. Add `re_syncing_reconciles_added_and_removed_members`.

- [ ] **Step 3-4:** Run it (fails before the module, passes after; wire `pub mod ws_members_group;` into `lib.rs`).

- [ ] **Step 5: Commit** `feat(perm-compile): mirror a workspace's members into a managed ws:<slug> members group`.

---

### Task 2: Keep `ws:<slug> members` fresh on a workspace-membership change

**Files:** Modify the workspace-membership recompile chokepoint (find it: grep for the recompile path triggered by a `access_grants scope='workspace'` change / the SP3c-1 `role_membership.rs` analog, and `PermGroupRecompiler`); Test `ws_assignment_migrate.rs`.

**Interfaces:**
- Consumes: `sync_ws_members_group_if_present_in_tx` (Task 1); the live path that writes `access_grants scope='workspace'` and recompiles (workspace add/remove-member, and the bulk workspace-roster writes — trace them exactly as SP3c-1 traced the three platform-role write paths).
- Produces: after a workspace's membership changes, the `ws:<slug> members` group is re-synced before the recompile, so group-ref grants see fresh membership. **Enumerate EVERY live path that mutates `access_grants scope='workspace'` and recompiles** (SP3c-1's final review found a missed sibling path — do the same sweep here) and route each through the guarded sync; leave pre-existing no-recompile paths as-is but note them.

- [ ] **Step 1:** Write a failing DB-gated test (SERIAL, own workspace): migrate a workspace grant (via Task 4 helper or a direct insert of the group ref), then change the workspace's membership via the LIVE path, and assert the `ws:<slug> members` `user_groups` + the recompiled grant reflect the change (fail-closed on remove).
- [ ] **Step 2-4:** Add the guarded sync at the chokepoint(s); run green.
- [ ] **Step 5: Commit** `feat(perm-compile): resync ws:<slug> members groups on workspace-membership change`.

---

### Task 3: `ws_assignment_migration` provenance table

**Files:** Create `migrations/NNNN_ws_assignment_migration.sql`; Test `ws_assignment_migrate.rs`.

**Interfaces:**
- Produces: `perm.ws_assignment_migration (id bigserial PK, catalogs uuid[], grant_ids uuid[], principal_hashes text[], original jsonb, migrated_by text, migrated_at timestamptz DEFAULT now())` — one row per run; `original` holds the pre-migration `(grant_id, origin, kind, ref)` tuples for exact restore.

- [ ] **Step 1-4:** Migration + a round-trip test. Use the next free migration number above main and open PRs.
- [ ] **Step 5: Commit** `feat(perm): ws-assignment migration provenance table`.

---

### Task 4: Reversible rewrite of implicit workspace grants → `ws:<slug> members` group grants (+ equivalence gate)

**Files:** Create `crates/egret-perm-compile/src/ws_assignment_migrate.rs`; Modify `lib.rs`, `principal_refs.rs`; Test `ws_assignment_migrate.rs`.

**Interfaces:**
- Consumes: `perm.grants` (`origin='workspace_assignment'`, `origin_workspace_id`), `perm.grant_principals` (`kind='workspace_members'`, `ref=<workspace_id>`); `ws_members_group::{ws_members_group_name, sync_ws_members_group_in_tx}` (Task 1); the SP3c-1 equivalence/undo patterns in `role_principal_migrate.rs` (`canonical_json`, `equivalence::derive_probes`, FOR UPDATE + 409). Read `role_principal_migrate.rs` as the template.
- Produces:
  - `pub async fn migrate_ws_assignments_in_tx(tx, migrated_by: &str) -> Result<WsAssignmentMigration, PermHttpError>` — for each `origin='workspace_assignment'` grant: ensure the `ws:<slug> members` group (Task 1, via the workspace's slug); rewrite its `kind='workspace_members'` principal to `kind='group'`, `ref=ws_members_group_name(slug)`, `mode='include'`; set the grant's `origin='explicit'` and clear/keep `origin_workspace_id` per the schema's NULL rules; record provenance; recompile affected; run the equivalence gate (compiled output byte-identical before/after — fingerprint each affected profile's compiled bodies, mirroring `role_principal_migrate.rs`); refuse on divergence → rollback.
  - `pub async fn undo_ws_assignments_in_tx(tx, migration_id: i64) -> Result<(), PermHttpError>` — FOR UPDATE provenance; re-hash; 409 on divergence; restore `origin='workspace_assignment'` + the `workspace_members` principal; recompile; delete provenance.
  - `WsAssignmentMigration { id: i64, catalogs: Vec<Uuid>, rewritten: usize }`.
  - `principal_refs.rs` accepts a `group` ref matching `^ws:.* members$` as a valid managed-group principal (existence-checked, like SP3c-1's `role:* holders` fix — NOT a blanket pattern accept; a non-existent one must classify `fails_open`, not silently empty).

- [ ] **Step 1: Write the failing test** `migrating_a_ws_assignment_is_byte_identical_and_removes_workspace_members` (SERIAL; own catalog/workspace): seed a catalog bound to a workspace with a member (the `workspace_members` grant exists); capture the compiled output; migrate; assert (a) no `kind='workspace_members'` principal remains for this catalog's grant, the grant is `origin='explicit'` with a `ws:<slug> members` group ref, and (b) the compiled output is byte-identical.
- [ ] **Step 2-5:** Implement the rewrite + provenance + gate + undo (undo-restore and undo-409 tests); mirror SP3c-1. Commit `feat(perm-compile): reversible workspace-assignment -> ws:<slug> members rewrite with equivalence gate`.

---

### Task 5: Authoring — `sync_assignment_grants_in_tx` produces the group grant

**Files:** Modify `crates/egret-perm/src/assignment.rs` (`sync_assignment_grants_in_tx`); Test `ws_assignment_migrate.rs` (+ an `egret-perm`/catalog-registry test if the binding path is reachable there).

**Interfaces:**
- Consumes: the catalog-binding path (`set_catalog_workspaces_in_tx` → `sync_assignment_grants_in_tx`); `ws_members_group::sync_ws_members_group_in_tx`.
- Produces: binding a catalog to a workspace now creates/keeps an **explicit** READ grant whose principal is the `ws:<slug> members` group (ensuring that group exists), instead of the `origin='workspace_assignment'` + `kind='workspace_members'` grant. Unbinding removes it. One grant per (catalog, workspace), idempotent. So a NEWLY bound catalog gets the migrated shape directly, and the Task-4 migration backfills pre-existing bindings — every workspace grant ends up as a group grant, with no `workspace_members` principal authored going forward.

> **Design note:** keep the partial-unique-index guarantee (one grant per catalog+workspace). Decide whether to keep `origin='workspace_assignment'`+`origin_workspace_id` as the dedup key on the new explicit grant (so unbind can still find it) or introduce an explicit-but-workspace-tagged origin; the implementer picks the minimal change that keeps bind/unbind idempotent AND lets undo/Task-4 coexist. State the choice in the task's report.

- [ ] **Step 1-5:** Failing test (bind a catalog to a workspace via the real path → a `ws:<slug> members` explicit group grant appears and enforces; unbind → it's gone), implement, green (SERIAL, own catalog). Commit `feat(perm): workspace binding grants ws:<slug> members directly`.

---

### Task 6: Transitional write guards + admin endpoints

**Files:** Modify `crates/egret-perm-compile/src/principal_refs.rs` (extend `refused_for_authoring` from SP3c-1), `crates/egret-perm-compile/src/routes.rs` (endpoints); regen `openapi.json` + client; Test `ws_assignment_migrate.rs`.

**Interfaces:**
- Produces: the write-seam guard additionally rejects a NEW `kind='workspace_members'` principal and a NEW `group` ref matching `^ws:.* members$` (migration/authoring targets only), without blocking the migration's raw UPDATEs, the authoring path, or a PATCH of an already-stored ref — exactly as SP3c-1 did for `role`. Admin endpoints `POST /api/control-plane/perm/migrate/ws-assignments` (+/undo) mirroring SP3c-1's role-principal endpoints (auth `policy:update`).

- [ ] **Step 1-5:** Guard tests (new workspace_members principal refused; new `ws:* members` ref refused), endpoint smoke test, regen openapi/client (digest clean), run the file under DEFAULT parallelism. Commit `feat(perm-compile): ws-assignment migration endpoints + transitional write guards`.

---

### Task 7: `explain` unchanged for workspace members

**Files:** Test `ws_assignment_migrate.rs`.

- [ ] **Step 1-3:** `explain_is_byte_identical_across_the_ws_assignment_migration` (SERIAL; own catalog/workspace): capture `explain` for a workspace member BEFORE, migrate, capture AFTER, assert byte-identical (assert the member's before-answer is a real result so it isn't vacuous). Commit `test(perm-compile): explain unchanged across the ws-assignment migration`.

---

## Out of scope (later SP3 / follow-ups)

- Per-EP gate + switch demo + live screenshots (SP3d).
- Removing the `workspace_members` principal KIND from the enum entirely (a later cleanup once no grant uses it; this package stops authoring it and migrates existing ones, but leaves the kind resolvable for safety/rollback).
- Docs: the workspace-assignment / catalog-binding behavior change (bind now grants a `ws:<slug> members` group) — update `egret-data/docs` + note the API `ws-assignments` migration endpoint.

## Self-Review

- **Spec coverage (SP3c point 3):** implicit `workspace_assignment`/`workspace_members` grant → explicit `ws:<slug> members` group grant (Tasks 4, 5); managed group mirrors workspace membership (Tasks 1-2); transitional no-new-workspace_members (Task 6); explain unchanged (Task 7); reversible provenance (Tasks 3-4); decision ③ same ws-members machinery (Tasks 1-2 reuse SP3c-1). ✅
- **Lessons baked in:** SERIAL + own-scope assertions on every DB-gated test (no global count); byte-identical gate fingerprints the compiled bodies; existence-checked `ws:* members` ref (no fail-open); sweep EVERY live workspace-membership write path (SP3c-1's missed-sibling lesson); no read-path switch needed (workspace_members kind still expands → no fail-open window).
- **Type consistency:** `ws_members_group_name` / `sync_ws_members_group_in_tx` / `migrate_ws_assignments_in_tx` / `undo_ws_assignments_in_tx` / `WsAssignmentMigration` named consistently; mirror the SP3c-1 names.
