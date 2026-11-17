# Governance SP5c — iceberg-rest EP Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the embedded Iceberg catalog (Lakekeeper) deny `READ` on any table carrying an active mask, row filter or withhold rule — to everyone but that rule's exemptions — behind a separate, opt-in, reversible per-catalog `iceberg-rest` switch, with grant decisions unchanged and the Ranger/OPA goldens byte-identical.

**Architecture:** A new catalog *enforcement* snapshot carries the catalog's rule + withhold statements and its classified objects; a `Snapshot::table_read_restricted` predicate (generalising the audited `metadata_refusal`) answers "does an active mask/row-filter/withhold hit this table for this non-exempt user?"; the request guard and the lakekeeper authorizer, when the catalog's new `iceberg-rest` switch is on, compose that predicate on top of SP5b's unchanged grant decision (`read = covers(read) AND NOT table_read_restricted`); the authorizer's `ReadData=false` then makes Lakekeeper vend no STS credential; a standalone capability-matrix table records what `iceberg-rest` enforces; plan/switch/undo endpoints gate the flip.

**Tech Stack:** Rust, sqlx (Postgres), the SP1 neutral IR + `perm-eval` + capability matrix (SP5a) + the SP5b per-catalog switch/gate pattern.

## Global Constraints

- Branch `feat/gov-sp5c-iceberg-rest-ep` (worktree `../egret-sp5c`, off main `aab83f678`). `CARGO_TARGET_DIR=target-sp5c` on EVERY cargo command. Throwaway Postgres `DATABASE_URL` — DB-gated tests MUST run, not skip (`env -u EGRET_ALLOW_DB_SKIP`).
- Chat VI; code/commits/docs EN. Conventional Commits; `Closes #N` only in PR body. Plain `git commit`; stage by explicit path (`target-sp5c/` not gitignored — never `git add -A`). Co-author `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. No secrets.
- Migration numbers are a shared namespace — number provisionally ABOVE main's highest (`0236`), so SP5c starts at `0237`; renumber above main AND open PRs at merge.
- **Deliberate behaviour change, bounded.** SP5c ADDS rule-denies; it never changes a grant decision. The grant decision (`unified_decide::covers`) and its grant-only snapshot (`build_catalog_snapshot_via`) are UNTOUCHED. Rule-denies apply only when the catalog's `iceberg-rest` switch is on.
- **Byte-identical Ranger/OPA:** SP5c touches neither the Ranger/OPA compilers nor the IR securable schema; re-run both mechanisms' goldens — zero diff, nothing re-blessed.
- **Two hook points.** A withheld table's READ must be denied at BOTH the request guard (`egret-bin/src/main.rs`) and the lakekeeper authorizer (`authorizer.rs client_may_unified`). The authorizer is the only place that stops credential vending.
- **Opt-in + reversible.** Default off (a catalog with no switch row enforces no rules). `undo` is the full reverse; there is no `finalize`.
- Run `cargo fmt` + `cargo clippy -p <crate> --all-targets -- -D warnings` before each commit. `npm run gen:api` (never bare orval) if the API surface changes. A permission/API change updates `egret-data/docs`.

**Key code anchors (verified on main `aab83f678`):**
- `crates/egret-perm-eval/src/decide.rs` — `metadata_refusal` (`:658`, the pattern to generalise), `restricts(&Effect)` (`:297`: Deny/Withhold-carrying-Read, Mask≠`MASK_NONE`, non-empty RowFilter, WithholdMask), `applies(&Audience,user)` (`:229`, `pub(crate)`), `candidates(catalog,schema,table,&classes)` (`:721`, `pub(crate)`), `target_hits`/`path_on_table`, `decide_privilege` (`:421`), `check_data` (`:572`, deny/withhold wins), `self.index.classes.get((cat,schema,table))` → `{table, columns}` tag sets.
- `crates/egret-perm-eval/src/values.rs` — `row_filters` (`:91`, the mirror), `table_classes`/`column_classes` (`:211`/`:220`).
- `crates/egret-perm-eval/src/snapshot.rs` — `ClassifiedColumn{catalog,schema,table,column:Option<String>,classification}` (`:14`), `SnapshotInput{service_id,key,statements,classified,lineage_principal}`, `Snapshot::build`.
- `crates/egret-perm-compile/src/opa/build.rs` — `build_catalog_snapshot_via(conn,catalog_id) -> Result<Option<Snapshot>,_>` (grants+owners only, `classified: Vec::new()`), `load_classified_columns_via(conn,service_id)` (`:66`, uses `egret_policy::tag_assignments_for_catalogs_via(conn,&catalog_ids)` — a catalog variant is `&[catalog_id]`), the synthetic `EpInput` (profile_id:0, Trino/Ranger).
- `crates/egret-perm-compile/src/ir_load.rs` — `load_rule_inputs(conn,service_id) -> (BTreeMap<Uuid,RuleInputs>, BTreeMap<Uuid,Lowering>)` (`:229`), `load_withhold_inputs(conn,...)` (`:436`); `egret_perm_ir::lower::rules::lower_rules`, `egret_perm_ir::lower::withhold::lower_withholds`.
- `crates/egret-catalog-engine/src/unified_decide.rs` — `Source{CatalogRbac,Unified}` (`:50`), `catalog_grant_source` (`:62`), `resolve_catalog_switch(conn,warehouse_id) -> (Option<(Uuid,String)>,Source)` (`:163`), `CatalogSnapshotCache` (`:359`) + `get` (`:385`, rebuilds on `version_of`), `covers(snapshot,user,catalog,ns,table,write)`, `service_account_username` (`:139`).
- `crates/egret-bin/src/main.rs` — `EgretCatalogGuard` (`:588`, holds `snapshot_cache`), `check` (`:731`), `CatalogDecider` (`:642`) with `covers(ns,table,write,eq)` (`:706`), the `None`-arm read path (`:995`), `is_write` (`:881`).
- `crates/egret-catalog-engine/src/authorizer.rs` — `client_may` (`:193`), `client_may_unified` (`:264`, per-ask `covers(...,write)`), `are_allowed_table_actions_impl` (`:470`), `ReadData`→`Ask{write:false}` (`:132`).
- `crates/egret-catalog-engine/src/catalog_migrate_gate.rs` — `plan`/`switch`/`undo`/`finalize` (`:395`/`:410`/`:445`/`:472`), `EquivalenceReport` (`:89`), `Divergence` (`:67`), `GateError` (`:124`) — the endpoint/gate shape to mirror.
- `crates/egret-catalog-engine/src/warehouse_admin/mod.rs` — the `catalog_admin_router` `.route("/perm/catalog-migrate/{catalog}/…", post(catalog_migrate_*))` block (`:324`), the `#[utoipa::path]` handlers (`:1668`), `PlatformAuth`.
- `crates/egret-perm-ir/src/matrix.rs` — `EMBEDDED_CATALOG`/`EMBEDDED_CATALOG_PRIVILEGES`/`embedded_catalog_support`/`embedded_catalog_privilege_gap` (`:713`–`:752`) — the standalone precedent; `Answer`/`Support`/`Privilege` types; `EpKind`/`ENTRIES` (DO NOT touch).
- `migrations/0235_catalog_grant_switch.sql` — the switch-table precedent. `migrations/0236_catalog_role_id_nullable.sql` — current highest.

---

### Task 1: Catalog enforcement snapshot + the withheld-read predicate

**Files:**
- Modify: `crates/egret-perm-compile/src/opa/build.rs` — add `build_catalog_enforcement_snapshot` + `load_classified_columns_for_catalog_via`.
- Modify: `crates/egret-perm-eval/src/values.rs` (or `decide.rs`) — add `pub fn Snapshot::table_read_restricted`.
- Test: `#[cfg(test)]` in `values.rs`/`decide.rs` (predicate, unit) + a DB test for the snapshot build in `crates/egret-perm-compile/tests/`.

**Interfaces:**
- Consumes: `candidates`, `restricts`, `applies`, `path_on_table`, `self.index.classes` (perm-eval, same crate); `load_rule_inputs`/`load_withhold_inputs` + `lower_rules`/`lower_withholds` + `tag_assignments_for_catalogs_via` (perm-compile).
- Produces:
  ```rust
  // egret-perm-eval
  impl Snapshot {
      /// True when an active mask, row filter or withhold hits (catalog, schema,
      /// table) — directly by path, or via a classification on the table or any
      /// of its columns — for `user`, who is in scope and not exempt. Generalises
      /// `metadata_refusal`: the whole-table control the iceberg-rest EP applies.
      pub fn table_read_restricted(&self, user: &str, catalog: &str, schema: &str, table: &str) -> bool;
  }
  // egret-perm-compile::opa::build
  /// The catalog's RULE + WITHHOLD statements + its classified objects — the
  /// snapshot `table_read_restricted` reads. NO grants (grants are decided by
  /// `build_catalog_snapshot_via`, unchanged). `Ok(None)`: no registry row.
  pub async fn build_catalog_enforcement_snapshot(conn: &mut sqlx::PgConnection, catalog_id: Uuid) -> Result<Option<Snapshot>, CompileError>;
  pub async fn load_classified_columns_for_catalog_via(conn: &mut sqlx::PgConnection, catalog_id: Uuid) -> Result<Vec<ClassifiedColumn>, CompileError>;
  ```

- [ ] **Step 1: Write failing predicate tests** in `crates/egret-perm-eval/src/values.rs` (`#[cfg(test)]`), building a `Snapshot` from `SnapshotInput` by hand (mirror the existing eval tests). Cases:
  - a `RowFilter` statement on `sales.eu.orders` targeting that table, audience `{public}` except `alice` → `table_read_restricted("bob", "sales", "eu", "orders") == true`, and `== false` for exempt `alice`.
  - a `Mask{mask_type:"MASK_NULL"}` statement on a COLUMN of `sales.eu.orders` (path `sales.eu.orders.ssn`) → `true` for a non-exempt user (whole-table deny), `false` for exempt.
  - a `Withhold{privileges:[Read]}` statement on the table → `true` for non-exempt, `false` for exempt.
  - a classification-selected mask: `ClassifiedColumn` tags column `ssn` with `pii`, a `Mask` statement targets `Classification{name:"pii"}` → `true`.
  - a table with only an `Allow`/grant statement → `false`.
  - a `Mask{mask_type:"MASK_NONE"}` (the no-op) → `false`.

- [ ] **Step 2: Run tests to verify they fail** (`table_read_restricted` not defined).
  Run: `CARGO_TARGET_DIR=target-sp5c cargo test -p egret-perm-eval table_read_restricted`

- [ ] **Step 3: Implement `table_read_restricted`** generalising `metadata_refusal` (decide.rs:658). Reference body:
  ```rust
  pub fn table_read_restricted(&self, user: &str, catalog: &str, schema: &str, table: &str) -> bool {
      let mut classes = std::collections::BTreeSet::new();
      if let Some(tc) = self.index.classes.get(&(
          catalog.to_lowercase(), schema.to_lowercase(), table.to_lowercase(),
      )) {
          classes.extend(tc.table.iter().cloned());
          for c in tc.columns.values() { classes.extend(c.iter().cloned()); }
      }
      for i in self.candidates(catalog, Some(schema), Some(table), &classes) {
          let s = &self.statements[i];
          if !applies(&s.audience, user) || !restricts(&s.effect) { continue; }
          let hit = match &s.target {
              Target::Path(p) => path_on_table(p, catalog, schema, table),
              Target::Classification { name } => classes.contains(&name.to_lowercase()),
              Target::EngineObject { .. } | Target::Infra(_) => false,
          };
          if hit { return true; }
      }
      false
  }
  ```
  (If `metadata_refusal`'s `index.classes` key-shape or `path_on_table` differ, match them exactly — this predicate must agree with `metadata_refusal` on which statements restrict a table.)

- [ ] **Step 4: Run predicate tests to verify they pass.**

- [ ] **Step 5: Write a failing DB test** for `build_catalog_enforcement_snapshot` in `crates/egret-perm-compile/tests/` (gate with the throwaway DB): seed a catalog with a table, a `perm.rules` mask on one of its columns (by classification), build the snapshot, assert `snapshot.table_read_restricted(non_exempt, catalog, schema, table)` is true and a sibling un-ruled table is false.

- [ ] **Step 6: Implement `load_classified_columns_for_catalog_via`** (copy `load_classified_columns_via` but pass `&[catalog_id]` to `tag_assignments_for_catalogs_via`), and **`build_catalog_enforcement_snapshot`**: build the synthetic `EpInput` exactly as `build_catalog_snapshot_via` does (profile_id:0, Trino/Ranger, `catalogs:{catalog_id}`); call `load_rule_inputs` + `lower_rules` and `load_withhold_inputs` + `lower_withholds` to produce the rule/withhold `Statement`s (scope to this catalog's objects as the OPA build does — rules are common per D9, `classified` limits which objects they hit); `strip_unenforced` as the grant build does; `Snapshot::build(SnapshotInput{ statements, classified: load_classified_columns_for_catalog_via(...), service_id:0, key:(0,0), lineage_principal: lineage_principal() })`. Do NOT include grant/owner statements.

- [ ] **Step 7: Run the DB test to verify it passes.**
  Run: `CARGO_TARGET_DIR=target-sp5c DATABASE_URL=… env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile build_catalog_enforcement_snapshot`

- [ ] **Step 8: Commit.**
  ```bash
  git add crates/egret-perm-eval/src/values.rs crates/egret-perm-compile/src/opa/build.rs crates/egret-perm-compile/tests/<file>.rs
  git commit -m "feat(gov): catalog enforcement snapshot + table_read_restricted predicate (SP5c)"
  ```

---

### Task 2: The per-catalog iceberg-rest switch (table, resolver, cache)

**Files:**
- Create: `migrations/0237_catalog_rule_enforcement_switch.sql`.
- Modify: `crates/egret-catalog-engine/src/unified_decide.rs` — `catalog_enforces_rules` resolver + `CatalogEnforcementSnapshotCache`.
- Test: `#[cfg(test)]` + DB test in `crates/egret-catalog-engine/`.

**Interfaces:**
- Produces:
  ```rust
  /// True when `catalog_id` has opted into iceberg-rest rule enforcement.
  /// A catalog with no row returns false (opt-in; shipping 0237 enables nothing).
  pub async fn catalog_enforces_rules(conn: &mut sqlx::PgConnection, catalog_id: Uuid) -> Result<bool, sqlx::Error>;
  /// Like `CatalogSnapshotCache` but caches the ENFORCEMENT snapshot
  /// (`build_catalog_enforcement_snapshot`), rebuilt on `version_of` change.
  pub struct CatalogEnforcementSnapshotCache { /* mirror CatalogSnapshotCache */ }
  impl CatalogEnforcementSnapshotCache { pub fn new() -> Self; pub async fn get(&self, conn, catalog_id) -> Result<Option<Arc<Snapshot>>, CompileError>; }
  ```

- [ ] **Step 1: Write the migration** `0237_catalog_rule_enforcement_switch.sql`:
  ```sql
  -- SP5c (iceberg-rest EP): the per-catalog, opt-in switch that turns on
  -- RULE enforcement (deny READ on a table carrying a mask / row filter /
  -- withhold) at the embedded catalog. INDEPENDENT of 0235's
  -- catalog_grant_switch: grant enforcement and rule enforcement switch
  -- separately. A catalog with no row enforces NO rules (opt-in), so shipping
  -- this migration enables nothing on its own. `undo` sets enforce=false.
  CREATE TABLE perm.catalog_rule_enforcement_switch (
      catalog_id   uuid PRIMARY KEY REFERENCES registry.catalogs(id) ON DELETE CASCADE,
      enforce      boolean NOT NULL DEFAULT false,
      switched_at  timestamptz,
      switched_by  text
  );
  ```

- [ ] **Step 2: Write a failing DB test** for `catalog_enforces_rules`: no row → false; row with `enforce=true` → true; `enforce=false` → false.

- [ ] **Step 3: Run it to verify it fails.**

- [ ] **Step 4: Implement `catalog_enforces_rules`** (mirror `catalog_grant_source`, `unified_decide.rs:62`):
  ```rust
  pub async fn catalog_enforces_rules(conn: &mut sqlx::PgConnection, catalog_id: Uuid) -> Result<bool, sqlx::Error> {
      let on: Option<bool> = sqlx::query_scalar(
          "SELECT enforce FROM perm.catalog_rule_enforcement_switch WHERE catalog_id = $1",
      ).bind(catalog_id).fetch_optional(&mut *conn).await?;
      Ok(on.unwrap_or(false))
  }
  ```

- [ ] **Step 5: Implement `CatalogEnforcementSnapshotCache`** by copying `CatalogSnapshotCache` (unified_decide.rs:359) verbatim, changing only the build call to `build_catalog_enforcement_snapshot`. (Keep the two caches separate: grant snapshot and enforcement snapshot invalidate on the same `version_of` but hold different statements.)

- [ ] **Step 6: Run tests to verify they pass.**

- [ ] **Step 7: Commit.**
  ```bash
  git add migrations/0237_catalog_rule_enforcement_switch.sql crates/egret-catalog-engine/src/unified_decide.rs crates/egret-catalog-engine/tests/<file>.rs
  git commit -m "feat(gov): per-catalog iceberg-rest rule-enforcement switch + enforcement snapshot cache (SP5c)"
  ```

---

### Task 3: Enforce the withheld-read deny at the guard and the authorizer

**Files:**
- Modify: `crates/egret-bin/src/main.rs` — `EgretCatalogGuard` (hold a `CatalogEnforcementSnapshotCache`), the READ path in `check`.
- Modify: `crates/egret-catalog-engine/src/authorizer.rs` — `EgretAuthorizer` (hold the cache), `client_may_unified`.
- Test: guard `#[cfg(test)]` in `main.rs`/`catalog_guard_body_tests.rs`; authorizer DB test.

**Interfaces:**
- Consumes: `catalog_enforces_rules`, `CatalogEnforcementSnapshotCache`, `Snapshot::table_read_restricted`, `service_account_username`.
- The deny rule at both sites, for a catalog whose switch is on, on a READ ask only:
  `if !write && catalog_enforces_rules(catalog_id) && enforcement_snapshot.table_read_restricted(username, catalog, schema, table) { deny }` — composed AFTER the existing `covers(read)` allow (deny a table the grant would otherwise allow).

- [ ] **Step 1: Write a failing guard test** in `crates/egret-catalog-engine/src/catalog_guard_body_tests.rs` (mirror the existing guard tests, which seed a catalog + switch): with the catalog on `unified` grants AND `iceberg-rest` enforce=on, a client with a read grant on a table that has a mask → the guard refuses the table read (`FORBIDDEN`); with enforce=off, the same read is allowed (proves opt-in); an exempt principal is allowed; a WRITE is unaffected by the predicate.

- [ ] **Step 2: Run it to verify it fails.**

- [ ] **Step 3: Thread the enforcement cache into `EgretCatalogGuard`** (`main.rs:588`): add `enforcement_cache: CatalogEnforcementSnapshotCache`, construct it where `snapshot_cache` is built. In `check`'s Unified read path (the `CatalogDecider::covers(..., write=false)` / `None`-arm at `main.rs:995`), after a grant-allow, if `catalog_enforces_rules(conn, catalog_id).await?` then load the enforcement snapshot and, for each read `Need` (ns→schema via the SP5b dot-join `namespace.join(".")`, table), if `snap.table_read_restricted(username, catalog_name, &schema, &table)` return the same refusal shape the guard uses. Only for reads; writes skip the predicate.

- [ ] **Step 4: Run the guard test to verify it passes.**

- [ ] **Step 5: Write a failing authorizer DB test** in `crates/egret-catalog-engine/` : `client_may` for a `ReadData` ask on a masked table, catalog on unified + enforce=on → returns `false` for that ask (so Lakekeeper vends no credential); enforce=off → `true`; a `WriteData` ask is unaffected by the predicate; an exempt principal → `true`.

- [ ] **Step 6: Run it to verify it fails.**

- [ ] **Step 7: Thread the cache into `EgretAuthorizer`** (`authorizer.rs:159`) and compose the predicate in `client_may_unified` (`authorizer.rs:264`): for each ask with `write=false`, after `covers(...,false)` returns true, if `catalog_enforces_rules` then `&& !enforcement_snapshot.table_read_restricted(username, catalog, schema, table)`. Resolve `schema` from the ask's `ns` by the same dot-join. Write asks unchanged.

- [ ] **Step 8: Run the authorizer test to verify it passes.**

- [ ] **Step 9: Run the full guard safety-net + authorizer suites** to confirm no legacy regression (the enforce=off default means every existing test is unaffected).
  Run: `CARGO_TARGET_DIR=target-sp5c DATABASE_URL=… env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-catalog-engine; cargo test -p egret-bin --bin egret`

- [ ] **Step 10: Commit.**
  ```bash
  git add crates/egret-bin/src/main.rs crates/egret-catalog-engine/src/authorizer.rs crates/egret-catalog-engine/src/catalog_guard_body_tests.rs crates/egret-catalog-engine/tests/<file>.rs
  git commit -m "feat(gov): deny READ on withheld tables at the guard and authorizer behind the iceberg-rest switch (SP5c)"
  ```

---

### Task 4: Equivalence-gated plan/switch/undo endpoints

**Files:**
- Create: `crates/egret-catalog-engine/src/iceberg_rest_gate.rs` (mirror `catalog_migrate_gate.rs`).
- Modify: `crates/egret-catalog-engine/src/warehouse_admin/mod.rs` — routes + `#[utoipa::path]` handlers + response DTOs.
- Modify: `crates/egret-catalog-engine/src/lib.rs` — `mod iceberg_rest_gate;` export.
- Test: `crates/egret-catalog-engine/tests/iceberg_rest_gate.rs` (DB).

**Interfaces:**
- Produces (mirror `catalog_migrate_gate`):
  ```rust
  pub struct RuleDeny { pub principal: String, pub catalog: String, pub schema: String, pub table: String }
  pub struct IcebergRestPlan {
      pub grants_identical: bool,            // false ⇒ switch refused
      pub grant_divergences: Vec<Divergence>,// reuse catalog_migrate_gate::Divergence; empty when identical
      pub new_denies: Vec<RuleDeny>,         // the intended allow→deny flips (reported, not refused)
  }
  pub async fn plan(store: &CatalogStore, enforcement: &CatalogEnforcementSnapshotCache, catalog_id: Uuid) -> Result<IcebergRestPlan, GateError>;
  pub async fn switch(store: &CatalogStore, enforcement: &CatalogEnforcementSnapshotCache, catalog_id: Uuid, actor: &str) -> Result<IcebergRestPlan, GateError>; // refuses if !grants_identical
  pub async fn undo(store: &CatalogStore, catalog_id: Uuid, actor: &str) -> Result<(), GateError>;
  ```

- [ ] **Step 1: Write a failing gate DB test** in `crates/egret-catalog-engine/tests/iceberg_rest_gate.rs`:
  - `plan` over a catalog with a masked table lists that table in `new_denies` for the principals who could read it and are not exempt, with `grants_identical == true` and empty `grant_divergences`.
  - `switch` on that catalog sets `catalog_rule_enforcement_switch.enforce = true`, filled `switched_at/by`; a subsequent read of the masked table is denied (end-to-end via `catalog_enforces_rules` + predicate).
  - `undo` sets `enforce = false`; the read is allowed again.
  - **Teeth:** construct a case where the grant decision WOULD differ (e.g. force a mismatch in the sample comparison) and assert `switch` refuses with `grants_identical == false` — the gate must never flip a catalog whose grant answers are not identical to SP5b's.

- [ ] **Step 2: Run it to verify it fails.**

- [ ] **Step 3: Implement `iceberg_rest_gate`.**
  - `grants_identical`/`grant_divergences`: reuse the SP5b sample population (authenticatable-client SAs × the catalog's named objects — mirror `catalog_migrate_gate`'s sampler) and compare the unified grant decision to itself across the switch (it is unchanged by SP5c, so this is a guard against accidental coupling; any divergence is a hard stop). If simplest, call the existing SP5b equivalence check and assert zero grant divergence.
  - `new_denies`: for each sampled `(SA-username, table)` where the grant allows READ, evaluate `enforcement_snapshot.table_read_restricted(username, catalog, schema, table)`; emit a `RuleDeny` when true and the principal is not exempt.
  - `switch`: refuse (`GateError`) unless `grants_identical`; else `INSERT … ON CONFLICT (catalog_id) DO UPDATE SET enforce=true, switched_at=now(), switched_by=$actor`.
  - `undo`: `UPDATE … SET enforce=false, switched_at=now(), switched_by=$actor` (or upsert enforce=false). Reversible, no provenance to hash (nothing was dropped).

- [ ] **Step 4: Add routes + handlers** in `warehouse_admin/mod.rs`, mirroring the `catalog-migrate` block (`:324`): `post(iceberg_rest_plan|iceberg_rest_switch|iceberg_rest_undo)` at `/perm/iceberg-rest/{catalog}/{plan|switch|undo}`, `PlatformAuth` (platform_admin-only, 403 otherwise), each with a `#[utoipa::path]` doc and a `ToSchema` response DTO. Export `mod iceberg_rest_gate` in `lib.rs`.

- [ ] **Step 5: Run the gate test + a route test** (platform_admin 200, non-admin 403) to verify they pass.

- [ ] **Step 6: Commit.**
  ```bash
  git add crates/egret-catalog-engine/src/iceberg_rest_gate.rs crates/egret-catalog-engine/src/warehouse_admin/mod.rs crates/egret-catalog-engine/src/lib.rs crates/egret-catalog-engine/tests/iceberg_rest_gate.rs
  git commit -m "feat(gov): equivalence-gated iceberg-rest plan/switch/undo endpoints (SP5c)"
  ```

---

### Task 5: Capability matrix entry + explain

**Files:**
- Modify: `crates/egret-perm-ir/src/matrix.rs` — the standalone `ICEBERG_REST` table + accessors.
- Modify: wherever `embedded_catalog_support`/`embedded_catalog_privilege_gap` are consumed by `explain` (grep `embedded_catalog_support`) — add the `iceberg-rest` case.
- Test: `#[cfg(test)]` in `matrix.rs`.

**Interfaces:**
- Produces (mirror `EMBEDDED_CATALOG_PRIVILEGES`/`embedded_catalog_support`, matrix.rs:713 — standalone, NOT in `EpKind`/`ENTRIES`):
  ```rust
  pub const ICEBERG_REST: &str = "iceberg-rest";
  /// Privilege/capability support for the iceberg-rest EP. Browse/Read/Write
  /// Native; Create/Manage Missing; Mask/RowFilter Lowered→whole-table withhold;
  /// Withhold Native.
  pub fn iceberg_rest_support(cap: /* Privilege or a small Capability enum */) -> Support;
  pub fn iceberg_rest_gap(cap: …) -> Option<String>; // the NotEnforced / lowering reason
  ```

- [ ] **Step 1: Write failing matrix tests** in `matrix.rs`: `iceberg_rest_support(Read) == Native`; `Browse == Native`; `Write == Native`; `Create == Missing(PrivilegeNotEnforceable)`; `Manage == Missing`; a `Mask` capability is `Lowered` with `iceberg_rest_gap(Mask)` naming "storage layer … whole table withheld"; a `RowFilter` is `Lowered`; a `Withhold` is `Native`.

- [ ] **Step 2: Run them to verify they fail.**

- [ ] **Step 3: Implement the standalone table + accessors**, modelled line-for-line on `EMBEDDED_CATALOG_PRIVILEGES`/`embedded_catalog_support`/`embedded_catalog_privilege_gap`, extended to carry the mask/row-filter/withhold capabilities (not only `Privilege`). Do NOT add an `EpKind` variant or an `ENTRIES` row.

- [ ] **Step 4: Run the matrix tests to verify they pass.**

- [ ] **Step 5: Wire into `explain`** so an `iceberg-rest` target reports the privilege support and the mask/row-filter → "table withheld" lowering reason (mirror how `explain` consumes `embedded_catalog_support`). Add/extend a test asserting `explain` surfaces the lowering reason for a masked table on `iceberg-rest`.

- [ ] **Step 6: Commit.**
  ```bash
  git add crates/egret-perm-ir/src/matrix.rs crates/<explain-consumer>
  git commit -m "feat(gov): iceberg-rest capability-matrix entry + explain lowering reasons (SP5c)"
  ```

---

### Task 6: Regen, byte-identical goldens, full gate, live-check recipe

**Files:**
- Possibly: `openapi.json`, `frontend/src/api/generated*` (only if the catalog-engine API surface changed — the new endpoints do change it).
- Create: the live-check recipe section in `.superpowers/sdd/.../task-6-report.md` (documented, run before merge).

- [ ] **Step 1: Regenerate OpenAPI** if the new endpoints changed the surface: `cargo run -p egret-bin -- openapi > openapi.json` then `npm run gen:api`. Commit only if the diff is non-empty.
  ```bash
  git add openapi.json frontend/src/api/generated.digest.json frontend/src/api/generated
  git commit -m "chore(gov): regenerate OpenAPI for iceberg-rest endpoints (SP5c)"
  ```

- [ ] **Step 2: Prove byte-identical goldens.** Ranger: `cargo test -p egret-perm-compile --features harness --test golden_compile`; OPA: same with `EGRET_GOLDEN_MECHANISM=opa`. Both must pass with the tree clean afterwards and NOTHING re-blessed. STOP if any golden diffs — SP5c must not touch compiled output.

- [ ] **Step 3: Full gate** on the touched crates (`egret-perm-eval`, `egret-perm-ir`, `egret-perm-compile`, `egret-catalog-engine`, `egret-bin`): `cargo fmt --check`; `cargo clippy -p <crate> --all-targets -- -D warnings`; `env -u EGRET_ALLOW_DB_SKIP cargo test -p <crate>` (re-run shared-DB flakes serially — `table_hook_db`, `view_hold_db`, `routes` — per SP5b's known contention class).

- [ ] **Step 4: Write the demo-stack live-check recipe** (documented; execute before merge): bring up the demo stack; on a catalog, put a mask on a table and turn on the `iceberg-rest` switch (`POST …/iceberg-rest/{catalog}/plan` then `…/switch`); with an external Iceberg client (PyIceberg) holding a read grant, load the masked table → refused and NO storage credential vended; load a normal table → reads; confirm grants still work and `undo` restores access.

- [ ] **Step 5: Commit** any regen; the goldens and gate produce no new files.

---

## Self-review notes (plan author)

- **Spec coverage:** deny-READ predicate (T1), two hook points (T3), STS falls out of the authorizer `ReadData=false` (T3, no storage change), separate opt-in switch (T2) + plan/switch/undo no-finalize (T4), standalone capability matrix (T5), byte-identical goldens + live-check + KI-315 documented (T6). All spec sections map to a task.
- **Grants unchanged:** the grant snapshot (`build_catalog_snapshot_via`) and `covers` are never modified; the enforcement snapshot is a separate build + cache; the gate's `grants_identical` check is the guard against accidental coupling.
- **Type consistency:** `table_read_restricted` (T1) is the one predicate used by T3 (both hooks) and T4 (gate `new_denies`); `catalog_enforces_rules` + `CatalogEnforcementSnapshotCache` (T2) are used by T3 and T4; `RuleDeny`/`IcebergRestPlan` (T4) reuse `catalog_migrate_gate::Divergence`.
- **Open implementer decisions (flagged in-task):** the exact `load_rule_inputs`/`load_withhold_inputs` scoping for a catalog context (T1 Step 6); whether `table_read_restricted` lives in `values.rs` or `decide.rs` (either, same crate); the small `Capability` enum shape for the matrix (T5) — follow `embedded_catalog` precedent.
