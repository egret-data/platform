# Governance SP5b — catalog merge (one grant store) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Fold the embedded Iceberg catalog's RBAC (`catalog.catalog_grants`/`catalog_roles`/`catalog_clients`) into the unified `perm.grants` + `perm.securable_owners` store, remove `catalog_roles`, replace the shared `engine` role with per-engine grants, and switch the embedded-catalog enforcement to `perm-eval` behind a reversible, equivalence-gated per-catalog switch — with zero allow/deny change (equivalence-proven) and the Ranger/OPA goldens byte-identical.

**Architecture:** A pure mapping library turns a catalog grant into perm grant rows on `(catalog_id, schema_name=namespace.join("."), table_name)` with faithful privilege→action sets; a reversible migrator folds a catalog's grants + owners into the store with content-hashed provenance; the request guard and the lakekeeper authorizer, when a per-catalog switch is `unified`, evaluate the caller's service-account principal via `perm-eval` (`decide_privilege` for Read/Write, reach for browse/listing) instead of `grants_for_role` + `catalog_grant_covers_by`; an equivalence gate proves parity before a switch and an undo/finalize pair makes it reversible while open.

**Tech Stack:** Rust, sqlx (Postgres), the SP1 neutral IR + `perm-eval` + capability matrix (SP5a).

## Global Constraints

- Branch `feat/gov-sp5b-catalog-merge` (worktree `../egret-sp5b`, off main `266e4730`). `CARGO_TARGET_DIR=target-sp5b` on EVERY cargo command. Throwaway Postgres `DATABASE_URL` — DB-gated tests MUST run, not skip.
- Chat VI; code/commits/docs EN. Conventional Commits; `Closes #N` only in PR body. Plain `git commit`; stage by explicit path (`target-sp5b/` not gitignored — never `git add -A`). Co-author `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. No secrets.
- Migration numbers are a shared namespace — number provisionally ABOVE main's highest (`0233`), renumber above main AND open PRs at merge.
- **Zero behaviour change, equivalence-proven.** The unified decision must equal the legacy `catalog_grant_covers_by` decision for every (principal, operation, securable) on the corpus AND the demo stack. The switch is refused if any pair diverges.
- **Byte-identical Ranger/OPA:** SP5b does NOT touch the IR securable schema or the Ranger/OPA compilers; re-run both mechanisms' goldens — zero diff, nothing re-blessed.
- Run `cargo fmt` + `cargo clippy -p <crate> --all-targets -- -D warnings` before each commit.
- **Reversible while open:** the migrate never mutates the source `catalog_roles`/`catalog_grants` (only the switch hides them from enforcement); undo removes the folded perm rows by recorded id with a hash guard; only `finalize` drops the legacy rows.

**Key code anchors (verified on main `266e4730`):**
- `crates/egret-catalog-engine/src/store/embedded.rs` — catalog RBAC store: `CatalogGrant` (:41), `grants_for_role` (:461), `list_grants`/`list_roles`/`list_clients`, `catalog_clients.service_account_id` (0230).
- `crates/egret-catalog-engine/src/grant_match.rs` — `grants_write` (:10: `write||manage`), `catalog_grant_covers_by` (:17: `is_write` single gate; `catalog`⇒all, `namespace`⇒ns+subtree, `table`⇒exact; reads also list the enclosing ns), `grant_sees_namespace` (:52: any privilege sees on/above/under).
- `crates/egret-bin/src/main.rs` — `EgretCatalogGuard::check` (:644), `grants_for_role` call (:727), write = `method IN POST/PUT/DELETE/PATCH`.
- `crates/egret-catalog-engine/src/authorizer.rs` — `EgretAuthorizer::client_may` (:189), `Caller::of` (:249), `fixed_answer` (System⇒allow).
- `crates/egret-engine-mgmt/src/engines.rs` — `ENGINE_ROLE_NAME="engine"` (:50), `ensure_engine_role` (:3196, adds catalog-wide `manage` :3214); `catalog_apply.rs attach_to_engine_inner` (:1155), `store_client_credential` (:741).
- `crates/egret-perm/src/owners.rs` — `OWNER_ACTIONS=[Browse,Read,Write,Create,Manage]` (:140), `set_owner_via` (:111), `list_owners_for_catalogs_via` (:72); `perm.securable_owners` (migration `0233`, one owner/securable, seeds nothing).
- `crates/egret-perm-eval/src/decide.rs` — `decide_privilege(user, DataPrivilege, resource)` (:406), `decide(user, operation, resource)` (:310), reach via `OpClass::Reach`; `Resource` enum (:21, name-based). `DataPrivilege {Read,Write}` (`trino_ops.rs:9`). `Snapshot::build(SnapshotInput)` (`snapshot.rs:146`).
- `crates/egret-perm/src/model.rs` — `Action {Browse,Read,Write,Create,Manage}` (:23); `principal_kind` ENUM `('user','group','role','workspace_members')` (migration `0154:8`) — **a service account is `kind='user'` with its `service_accounts.user_id`** (SA4 / migration `0229`). `perm.grant_principals` (`0154:46`).
- `crates/egret-perm-ir/src/matrix.rs` — the SP5a capability matrix (`ENTRIES`, `support(ep, EnforcePrivilege(p))`, `GATED=[Browse,Create,Manage]`).
- Reversible pattern: `perm.migrated_from` (migration `0181`, SHA-256 hashes + undo-refuses-if-changed), `perm.compile_runs` (`0198`); SP3d endpoints `POST /perm/migrate/finalize`/`undo` (`crates/egret-perm-compile/src/clearance_migrate.rs`, `role_principal_migrate.rs`).
- Owner-backfill creator source: `registry.catalogs.warehouse_id → catalog.warehouses.workspace_id → workspaces.created_by` (`0053`, `0003`; `created_by` nullable → fall back to platform-admins group).

---

### Task 1: Catalog→perm mapping library + capability-matrix target

**Files:**
- Create: `crates/egret-perm/src/catalog_map.rs` (pure mapping) + wire into `crates/egret-perm/src/lib.rs`.
- Modify: `crates/egret-perm-ir/src/matrix.rs` (add the `embedded-catalog` target entry enforcing `{browse,read,write}`).
- Test: `crates/egret-perm/src/catalog_map.rs` `#[cfg(test)]` + a matrix test.

**Interfaces:**
- Consumes: `Action` (`model.rs`), the catalog grant shape `(securable_type: &str, namespace: Option<&[String]>, table: Option<&str>, privilege: &str)`.
- Produces:
  ```rust
  pub struct MappedScope { pub schema_name: Option<String>, pub table_name: Option<String> }
  pub enum MapError { NamespaceLevelHasDot(String), UnknownSecurableType(String), UnknownPrivilege(String) }
  /// (securable_type, namespace, table) -> perm scope under a catalog_id.
  pub fn map_securable(securable_type: &str, namespace: Option<&[String]>, table: Option<&str>) -> Result<MappedScope, MapError>;
  /// catalog privilege -> the perm action set stored (read={read}, write={read,write}, manage={read,write,create,manage}).
  pub fn map_privilege(privilege: &str) -> Result<Vec<Action>, MapError>;
  ```
  Later tasks attach principals + catalog_id to build full `perm.grants` rows.

- [ ] **Step 1: Write failing tests** in `catalog_map.rs`:
  - `map_securable("catalog", None, None)` → `{schema:None, table:None}`.
  - `map_securable("namespace", Some(&["sales"]), None)` → `{schema:Some("sales"), table:None}`.
  - `map_securable("namespace", Some(&["sales","eu"]), None)` → `{schema:Some("sales.eu"), table:None}` (dot-join).
  - `map_securable("table", Some(&["sales","eu"]), Some("orders"))` → `{schema:Some("sales.eu"), table:Some("orders")}`.
  - `map_securable("namespace", Some(&["a.b"]), None)` → `Err(NamespaceLevelHasDot("a.b"))` (a single level containing a dot is refused).
  - `map_privilege("read")` → `[Read]`; `map_privilege("write")` → `[Read,Write]`; `map_privilege("manage")` → `[Read,Write,Create,Manage]`; case-insensitive; `map_privilege("bogus")` → `Err(UnknownPrivilege)`.
  - Matrix: `support("embedded-catalog", EnforcePrivilege(Browse|Read|Write))` is enforced; `Create|Manage` are NOT enforced (NotEnforced with a reason). Assert via the existing matrix accessor used by SP5a tests.
- [ ] **Step 2: Run, verify fail.** `DATABASE_URL=… CARGO_TARGET_DIR=target-sp5b cargo test -p egret-perm catalog_map` (no DB needed for the pure fns).
- [ ] **Step 3: Implement** `catalog_map.rs` (dot-join, dot-in-level refusal, privilege sets per the spec table) and the `embedded-catalog` matrix entry (mirror how an existing target declares `EnforcePrivilege` support; `Browse/Read/Write` supported, `Create/Manage` absent → GATED→NotEnforced).
- [ ] **Step 4: Run, verify pass.**
- [ ] **Step 5: Commit.** `feat(gov): catalog→perm grant mapping + embedded-catalog matrix target (SP5b)`

---

### Task 2: Provenance table + reversible fold migrator

**Files:**
- Create: `migrations/NNNN_catalog_rbac_migrated.sql` (NNNN above `0233`).
- Create: `crates/egret-perm-compile/src/catalog_rbac_migrate.rs` (the migrator + undo; mirror `clearance_migrate.rs`).
- Modify: `crates/egret-perm-compile/src/lib.rs` (expose it).
- Test: DB-gated tests in `catalog_rbac_migrate.rs`.

**Interfaces:**
- `perm.catalog_rbac_migrated` columns: `id bigserial PK`, `catalog_id uuid NOT NULL → registry.catalogs(id)`, `warehouse_id uuid NOT NULL`, `source_role_ids uuid[]`, `source_grant_ids uuid[]`, `source_hash text` (SHA-256 of the canonical catalog_roles+grants for the warehouse), `created_grant_ids uuid[]`, `created_owner_ids uuid[]`, `created_engine_grant_ids uuid[]`, `migrated_by text`, `migrated_at timestamptz DEFAULT now()`, UNIQUE `(catalog_id)`.
- `pub async fn fold_catalog(c: &mut PgConnection, catalog_id: Uuid, actor: &str) -> Result<FoldReport, _>` — idempotent (UNIQUE guards re-run; returns the existing report if already folded). Does NOT touch `catalog_roles`/`catalog_grants`.
- `pub async fn undo_fold(c: &mut PgConnection, catalog_id: Uuid) -> Result<(), _>` — deletes `created_*` rows by id, refusing (409-style error) if `source_hash` no longer matches the live catalog RBAC, then deletes the provenance row.

- [ ] **Step 1: Write the migration** creating `perm.catalog_rbac_migrated` with a comment citing SP5b. No data change in the SQL itself (the fold is done by the Rust migrator so it can compute hashes + attach principals).
- [ ] **Step 2: Write failing DB-gated tests** (seed a warehouse + registry catalog + 2 roles with grants + clients bound to service accounts, via the embedded store):
  - `fold_catalog` inserts `perm.grants` rows matching Task-1 mapping, with `grant_principals` = the service-account user_ids (kind `user`) of the clients bound to each role; records provenance; `created_grant_ids` non-empty.
  - Re-running `fold_catalog` is a no-op (same report, no duplicate perm rows).
  - A role with no client contributes no perm grant and is listed in `FoldReport.skipped_roles`.
  - `undo_fold` removes exactly the `created_*` rows and the provenance row; the catalog RBAC tables are untouched throughout.
  - `undo_fold` after mutating a source grant → `Err(SourceChanged)` (hash guard).
- [ ] **Step 3: Run, verify fail.**
- [ ] **Step 4: Implement** `fold_catalog`/`undo_fold`: read roles+grants+clients (`embedded::list_roles/list_grants/list_clients`), resolve each role's bound service-account user_ids, map each grant via Task 1, INSERT `perm.grants` + `perm.grant_principals` (kind `user`), compute `source_hash` (canonical sort of role+grant rows), record provenance. All in one transaction. (Engine/shared-role handling is Task 4 — here the `engine` role's grants fold like any other role for now and are reconciled in Task 4; note this in the report.)
- [ ] **Step 5: Run, verify pass.** `cargo test -p egret-perm-compile catalog_rbac_migrate`.
- [ ] **Step 6: Commit.** `feat(gov): reversible fold of catalog RBAC into perm.grants with provenance (SP5b)`

---

### Task 3: Owner backfill for catalog securables

**Files:**
- Modify: `crates/egret-perm-compile/src/catalog_rbac_migrate.rs` (add `backfill_owner`), called inside `fold_catalog`.
- Test: DB-gated tests.

**Interfaces:**
- `async fn backfill_owner(c, catalog_id, warehouse_id) -> Result<Option<Uuid>, _>` — if the catalog securable (catalog_id, schema=NULL, table=NULL) has no owner, set one: principal = `workspaces.created_by` of the warehouse's workspace where non-NULL (kind `user`), else the platform-admins group (kind `group`, the group ref used elsewhere — see `egret-auth/src/roles.rs`/`people.rs`). Idempotent via `set_owner_via` (`owners.rs:111`). Returns the created owner id (recorded in provenance `created_owner_ids`).

- [ ] **Step 1: Write failing DB-gated tests:**
  - Warehouse whose workspace has `created_by = U` → folding seeds a catalog-level owner = U (kind user); `list_owners_for_catalogs_via` returns exactly one owner for the catalog.
  - Warehouse whose workspace `created_by IS NULL` → owner = platform-admins group.
  - A catalog that already has an owner → backfill leaves it unchanged (idempotent); undo removes a backfilled owner but NOT a pre-existing one (track via provenance).
- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement** `backfill_owner`, wire into `fold_catalog`, record the owner id in provenance; `undo_fold` removes only owner ids it created.
- [ ] **Step 4: Run, verify pass.**
- [ ] **Step 5: Commit.** `feat(gov): backfill one owner per catalog securable (creator else platform-admins) (SP5b)`

---

### Task 4: Engine credential rework — retire the shared `engine` role

**Files:**
- Modify: `crates/egret-engine-mgmt/src/engines.rs` (`ensure_engine_role` → `ensure_engine_grant`), `crates/egret-engine-mgmt/src/catalog_apply.rs` (attach path).
- Create: `crates/egret-engine-mgmt/src/engine_grant_migrate.rs` (migrate existing attachments) or fold into the Task-2 migrator.
- Test: DB-gated tests.

**Interfaces:**
- New: on engine create / catalog attach, instead of `ensure_engine_role` + a shared-role client, ensure the engine's **service-account** principal (the attachment already mints a per-engine client; bind/create its service account) holds an explicit `perm.grants` allow of `{read,write,create,manage}` on the catalog securable (catalog-level). The per-engine client credential is unchanged.
- Migration of existing attachments: for each engine catalog attachment, ensure its client's service account has the per-engine grant; the shared `engine` role's catalog-wide `manage` grant is recorded in provenance and NOT carried forward as a shared grant. After all engines on a warehouse are migrated, the `engine` role has no remaining enforcement role (removed at finalize, Task 6).

- [ ] **Step 1: Write failing DB-gated tests:**
  - Attaching an engine to a catalog creates a per-engine service-account principal with a `{read,write,create,manage}` catalog-level grant in `perm.grants` (not a shared role).
  - Two engines on one catalog get TWO distinct per-engine grants (distinct principals), not one shared grant.
  - Migrating an existing attachment that used the shared `engine` role yields a per-engine grant for that engine and records the shared-role manage in provenance.
  - The enforced effect of `{read,write,create,manage}` at the embedded-catalog target equals the old shared `manage` (assert via Task-5 decision once available, or a focused `decide_privilege` Read+Write allow here).
- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement.** Replace `ensure_engine_role` usage; keep `store_client_credential`. Guard against breaking engine provisioning — follow the existing attach flow (`attach_to_engine_inner`), only swapping role→grant. Do NOT delete `ENGINE_ROLE_NAME`/`ensure_engine_role` yet if other code still references it in legacy mode; gate behind the switch so legacy-mode catalogs still work.
- [ ] **Step 4: Run, verify pass.**
- [ ] **Step 5: Commit.** `feat(gov): per-engine catalog grant replaces the shared engine role (SP5b)`

---

### Task 5: Per-catalog switch + unified decision path (guard + authorizer)

**Files:**
- Create: `migrations/NNNN_catalog_grant_switch.sql` (a `perm.catalog_grant_switch(catalog_id PK → registry.catalogs, source text CHECK IN ('catalog_rbac','unified') DEFAULT 'catalog_rbac', switched_at, switched_by)`).
- Create: `crates/egret-catalog-engine/src/unified_decide.rs` (build a catalog `Snapshot`; map op → decision).
- Modify: `crates/egret-bin/src/main.rs` (`EgretCatalogGuard::check`), `crates/egret-catalog-engine/src/authorizer.rs` (`client_may`).
- Test: DB-gated + decision shape tests.

**Interfaces:**
- `async fn catalog_grant_source(c, catalog_id) -> Source` (reads the switch; default `catalog_rbac`).
- `unified_decide`: `fn covers(snapshot: &Snapshot, principal_user: &str, catalog_name: &str, namespace: &[String], table: Option<&str>, is_write: bool) -> bool` — builds the `Resource` (`schema = namespace.join(".")`), calls `decide_privilege(principal, Write, …)` for writes, `decide_privilege(principal, Read, …)` for reads, and the **reach** path for namespace listing visibility (reproducing `grant_sees_namespace`). Returns the same boolean shape as `catalog_grant_covers_by`.
- A per-catalog `Snapshot` cache keyed by `catalog_id`, invalidated on grant/owner/rule change for that catalog (reuse the freshness signal the compile trigger uses, e.g. a version/updated_at per catalog).

- [ ] **Step 1: Write failing tests** (DB-gated where a Snapshot is built from seeded perm.grants):
  - For a corpus of folded grants (catalog/namespace/table × read/write, a sibling-not-covered probe, a nested namespace, a deny), `unified_decide::covers(...)` returns the SAME boolean as `catalog_grant_covers_by(...)` for the matching legacy grant set. (This is the equivalence corpus; Task 6 reuses it.)
  - Namespace listing: `covers(..., table=None, is_write=false)` on a table-grant holder is true for the enclosing namespace (reach), false for a sibling namespace — matching `grant_sees_namespace`.
  - Switch default is `catalog_rbac`; the guard uses legacy when `catalog_rbac`, unified when `unified` (inject a fake switch value).
- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement** `unified_decide` (Snapshot build mirrors the explain route's construction — `Snapshot::build` with the catalog's grants/owners), the switch read + cache, and the branch in `check`/`client_may`: resolve the caller's service-account user_id (the client's `service_account_id` → `service_accounts.user_id`), then legacy or unified per the switch. System principal stays allow-all (`fixed_answer`).
- [ ] **Step 4: Run, verify pass.** Include a p99 micro-measurement note (cached Snapshot vs legacy query) in the task report.
- [ ] **Step 5: Commit.** `feat(gov): unified perm-eval decision path behind a per-catalog switch (SP5b)`

---

### Task 6: Equivalence gate + migrate/undo/finalize endpoints + differential

**Files:**
- Create: `crates/egret-perm-compile/src/catalog_migrate_gate.rs` (the parity check).
- Modify: the perm migrate routes (where SP3d's `POST /perm/migrate/finalize`/`undo` live — `crates/egret-perm-compile/src/routes/` ; grep `migrate/finalize`) to add `POST /perm/catalog-migrate/{catalog}/plan|switch|undo|finalize`.
- Test: DB-gated + a documented live-check recipe appended to the task report.

**Interfaces:**
- `plan`: folds (Task 2/3/4) into the store WITHOUT switching, returns the equivalence report (sampled principals × objects: the warehouse's clients/service accounts × the namespaces/tables named in its catalog grants, plus a sibling probe) comparing `catalog_grant_covers_by` vs `unified_decide::covers`. Lists every divergence.
- `switch`: refuses (409) unless the latest `plan` equivalence is clean; else sets the switch to `unified`.
- `undo`: sets the switch back to `catalog_rbac` and calls `undo_fold` (hash-guarded).
- `finalize`: refuses if the switch is not `unified`; drops the warehouse's `catalog_grants` + `catalog_roles` (incl. the dead `engine` role) and marks the provenance finalized (no more undo). Admin-only (`platform_admin`).

- [ ] **Step 1: Write failing tests:**
  - `plan` on a seeded catalog returns a clean equivalence report; `switch` then succeeds; post-switch `unified_decide` is live.
  - Introduce a deliberate divergence (a legacy grant the mapping can't represent, e.g. a namespace level with a dot) → `plan` lists it and `switch` is refused.
  - `undo` after `switch` restores `catalog_rbac` and removes folded rows; legacy enforcement works again.
  - `finalize` after `switch` drops `catalog_roles`/`catalog_grants` for the warehouse and refuses a later `undo`.
  - Non-admin caller → 403.
- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement** the gate + endpoints, reusing SP3d's migrate-endpoint shape (auth, error codes, provenance). Append a **live-check recipe** to the task report: on the demo stack, fold+switch a catalog, run an engine query and an unprivileged client before/after, show matching allow/deny, undo, finalize.
- [ ] **Step 4: Run, verify pass.**
- [ ] **Step 5: Commit.** `feat(gov): equivalence-gated catalog migrate/undo/finalize endpoints (SP5b)`

---

### Task 7: OpenAPI/client regen + full gate + byte-identical

- [ ] **Step 1:** If the new `catalog-migrate` endpoints are in the OpenAPI surface, regenerate: `cargo run -p egret-bin -- openapi` then `npm run gen:api` (NEVER bare orval); confirm drift tests pass. If the endpoints are admin-internal and not in the generated client surface, note no regen needed.
- [ ] **Step 2: Byte-identical** — run the Ranger golden (`--features harness`) AND `EGRET_GOLDEN_MECHANISM=opa` golden; confirm zero diff, nothing re-blessed (SP5b does not touch the IR securable schema or the compilers).
- [ ] **Step 3: Full gate** — `cargo fmt --check`; `cargo clippy -p egret-perm -p egret-perm-ir -p egret-perm-compile -p egret-perm-eval -p egret-catalog-engine -p egret-engine-mgmt --all-targets -- -D warnings`; `cargo test` for those crates (DB-gated). The `routes.rs` TAG_FANOUT_LOCK / `ep_gate` flakes are shared-DB contention — if they are the only failures, re-run serially (`--test-threads=1`) on a quiet DB to confirm.
- [ ] **Step 4: Commit** any regenerated artifacts. `chore(gov): regenerate OpenAPI for catalog-migrate endpoints (SP5b)`

---

## Self-Review

- **Spec coverage:** mapping+matrix → T1; fold+provenance+undo → T2; owner backfill → T3; engine credential rework → T4; switch+unified decision → T5; equivalence gate+undo+finalize+live → T6; regen+byte-identical+gate → T7. Out-of-scope (iceberg-rest EP, global legacy-code removal, UI) named in the spec.
- **Equivalence:** T5 builds the corpus comparing `unified_decide` vs `catalog_grant_covers_by`; T6 makes it the switch gate (refuse on any divergence). The op→action mapping (reads→Read, all mutations→Write, listing→reach) is the one that preserves the legacy single `is_write` gate.
- **Reversibility:** the migrate never mutates source RBAC (T2); undo is hash-guarded (T2); only finalize drops legacy rows (T6) — "undo while the switch is open" holds.
- **Byte-identical:** T7 re-runs both mechanisms; SP5b adds a matrix target and a decision path but does not change the IR securable schema or the Ranger/OPA compilers.
- **Chokepoint / honesty:** the embedded-catalog target enforces `{browse,read,write}`; create/manage are never queried there (op-mapping) and recorded NotEnforced in the matrix (T1) so `explain`/docs tell the truth.
- **Open items resolved in-plan:** service-account principal is `kind='user'` with `service_accounts.user_id`; owner creator source is `workspaces.created_by` else platform-admins group; catalog decision uses `decide_privilege(Read|Write)` + reach (not the operation-string `decide`, which would hit "operation not modelled").
