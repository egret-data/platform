# Governance SP3c-2 — Clearance → `withhold` rule in the permission model

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate each `public.classification_policies` row into a `perm.rules` row of a new `withhold` kind whose exemptions are stored as `perm.rule_exemptions`, so clearance enforcement comes from the IR like any other rule — which, as a side effect, fixes the Ranger gap where **groups-only clearance compiled to nothing**.

**Architecture:** Add a `perm.rule_kind='withhold'` value. A withhold rule is a tag-selected `perm.rules` row (no mask/filter payload) plus `perm.rule_exemptions` naming the cleared principals: each `required_roles` entry becomes the SP3c-1 managed group `role:<name> holders`, and each `required_groups` entry becomes that group by name. The withhold lowering reads these migrated rules + their exemptions (expanded through the standard principal directory) instead of reading `classification_policies` directly, so the exemption set resolves uniformly for roles AND groups. A reversible, provenance-backed migration performs the rewrite; its equivalence gate holds the compiled output byte-identical for the clearances that have a defined behavior today (role-based), while groups-only clearance — which compiled to nothing before — is newly enforced (proven by a seeded test, not by the gate).

**Tech Stack:** Rust (sqlx) in `egret-perm-compile` / `egret-perm-ir`; Postgres migrations.

## Global Constraints

- **Depends on SP3c-1** (managed `role:<name> holders` groups, PR #858). Branch off `feat/gov-sp3c`. `role_holder_group::role_holder_group_name(role) -> "role:<name> holders"` and the managed-group machinery already exist and are reused here — a clearance's `required_roles` exemption becomes that group.
- **New `perm.rule_kind='withhold'`** (migration). A withhold rule has `selector_kind='tag'`, a `tag_key`, and NULL `mask_type`/`value_expr`/`filter_expr`. Adjust the `perm.rules` CHECK constraints so a `withhold` row is valid with all mask/filter payload columns NULL (today the table allows NULL payloads, but confirm no CHECK ties `kind` to a payload; add the `withhold`-has-no-payload CHECK).
- **Exemptions are the cleared set.** `perm.rule_exemptions` for a withhold rule name the principals who are exempt (cleared): `required_roles[i]` → `kind='group', ref=role_holder_group_name(role), mode='include'`; `required_groups[i]` (a group UUID) → `kind='group', ref=<groups.name for that id>, mode='include'`. Resolve group UUID → name at migration time.
- **The groups-only gap is fixed, not preserved.** Today `ir_load::load_withhold_inputs` builds `cleared` only from `required_roles` and sets `required_groups_empty`; `lower/withhold.rs::lower_withholds` (groups-only arm ~:485-513) emits nothing when `required_roles` is empty. After this package the exemption set is the expanded union of ALL exemption principals (role-holder groups + required groups), so groups-only clearance resolves to a real cleared set and is enforced on Trino and StarRocks.
- **Equivalence at migration time is conditional:** byte-identical for every clearance whose behavior is defined today (any clearance with at least one `required_roles` entry — the demo stack's one clearance is `required_roles={data_steward}`, `required_groups={}`). A clearance that is **groups-only** (`required_roles` empty, `required_groups` non-empty) legitimately changes from "compiles to nothing" to "enforced"; the migration gate must EXCLUDE those from the byte-identical check and the change is covered by a seeded test instead. Measured: the demo stack has exactly ONE clearance, role-based, so the live migration is fully byte-identical there.
- **Reversible with provenance** (own table `perm.clearance_migration`, not `perm.migrated_from`). Undo restores `classification_policies` and removes the withhold rules + exemptions, hash-guarded (409 on divergence).
- **`classification_policies` leaves the enforcement input.** After migration, `load_withhold_inputs` reads the migrated withhold rules, not `classification_policies`. The table remains as the clearance-authoring store (the Classifications UI still writes it) UNLESS a later step moves authoring too — for SP3c-2, keep `classification_policies` as the authoring source and have the write path ALSO maintain the withhold rule (see Task 6), so enforcement and authoring stay consistent. (Do not delete the table.)
- **`explain` unchanged** for every affected principal on role-based clearance.
- **Reuse** `recompile_role_referencing_profiles` / the chokepoint from SP3c-1: its query arm that reads `classification_policies.required_roles` must now also (or instead) find profiles via the withhold rules' exemptions. Keep clearance recompile-on-holder-change working.
- CI gates: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `committed_openapi_json_is_current`, generated-client digest, DB-gated Rust tests. No golden/fixture is re-blessed for a role-based clearance (byte-identical); a new golden may be added for the groups-only case.
- Migration numbers: current max on this branch is `0221`; new migrations are `0222+`, renumbered above main and open PRs at merge.
- `CARGO_TARGET_DIR=target-sp3c2`. Commits Conventional, co-author `Claude Opus 5.5`. Stage only changed files; do not push. StarRocks DB-gated clearance tests must seed the fork image `ghcr.io/egret-data/egret-starrocks-fe:4.1.7-egret.4` (≥ `STARROCKS_MASK_MIN_EGRET_IMAGE`) or the withhold/mask is image-gated to a deny.

## File Structure

- `migrations/0222_rule_kind_withhold.sql` (new) — add `withhold` to `perm.rule_kind`; relax/add CHECKs so a withhold row carries no mask/filter payload.
- `migrations/0223_clearance_migration.sql` (new) — provenance table `perm.clearance_migration`.
- `crates/egret-perm-ir/src/statement.rs` / `lower/withhold.rs` (modify) — the `Gate` now carries an expanded `cleared` that includes group-resolved members; delete the `required_groups_empty` groups-only skip.
- `crates/egret-perm-compile/src/ir_load.rs` (modify) — `load_withhold_inputs` reads `perm.rules` withhold rules + `rule_exemptions`, expands exemptions via `members`/the principal directory into `cleared` (roles + groups uniformly).
- `crates/egret-perm-compile/src/clearance_migrate.rs` (new) — `migrate_clearances_in_tx` / `undo_clearances_in_tx` + `ClearanceMigration`, equivalence-gated (excluding groups-only), hash-guarded undo.
- `crates/egret-perm-compile/src/routes.rs` (modify) — admin endpoints `POST /perm/migrate/clearances` (+ `/undo`).
- `crates/egret-perm-compile/src/role_membership.rs` (modify) — chokepoint clearance arm reads withhold-rule exemptions.
- Tests: `crates/egret-perm-compile/tests/clearance_migrate.rs` (new, DB-gated).

---

### Task 1: Add the `withhold` rule kind

**Files:** Create `migrations/0222_rule_kind_withhold.sql`; Test `crates/egret-perm-compile/tests/clearance_migrate.rs` (first test).

**Interfaces:**
- Consumes: `perm.rule_kind` ENUM `('mask','row_filter')` (`migrations/0154_perm_model.sql:10`); `perm.rules` CHECKs (`0154:71-...`).
- Produces: `perm.rule_kind` additionally accepts `'withhold'`; a `perm.rules` row with `kind='withhold'`, `selector_kind='tag'`, a `tag_key`, and NULL `mask_type`/`value_expr`/`filter_expr` is valid.

- [ ] **Step 1: Write the migration**

```sql
-- 0222_rule_kind_withhold.sql
-- Clearance becomes a withhold rule: a tag-selected perm.rules row with no
-- mask/filter payload, whose rule_exemptions name the cleared principals.
ALTER TYPE perm.rule_kind ADD VALUE IF NOT EXISTS 'withhold';
-- A withhold rule carries no mask/filter payload. (ALTER TYPE ... ADD VALUE
-- cannot run inside the same tx as its use; this migration only adds the value
-- and the CHECK. Keep them in one migration file; sqlx runs each file in its
-- own tx, and ADD VALUE IF NOT EXISTS is allowed before the ALTER TABLE here
-- because no row uses it yet.)
ALTER TABLE perm.rules
  ADD CONSTRAINT rules_withhold_has_no_payload CHECK (
    kind <> 'withhold'
    OR (mask_type IS NULL AND value_expr IS NULL AND filter_expr IS NULL
        AND selector_kind = 'tag')
  );
```

> **Implementer note:** Postgres forbids using a newly-added enum value in the SAME transaction that added it. `ALTER TYPE ... ADD VALUE` + a CHECK that only *names* the value in a text comparison is fine (the CHECK is not inserting the value). If sqlx wraps the file in one tx and Postgres rejects it, split into `0222_rule_kind_withhold.sql` (ADD VALUE only) and `0223_withhold_check.sql` (the CHECK), and shift the provenance migration to `0224`. Verify which by running the migration.

- [ ] **Step 2: Write the failing test** — insert a `kind='withhold'` tag rule with NULL payload; assert it succeeds; assert a `kind='withhold'` row with a non-null `mask_type` is rejected by the CHECK. (Use the SP3c-1 DB idiom: `let Some(dsn)=egret_test_db::url() else { return };` → `Db::connect(&dsn,5)` → `db.migrate()` → `db.pool`.)

- [ ] **Step 3-4:** Run it (fails before migration, passes after).

- [ ] **Step 5: Commit** `feat(perm): add the withhold rule kind`.

---

### Task 2: Expand clearance exemptions through the principal directory (fix the groups-only gap)

**Files:** Modify `crates/egret-perm-ir/src/lower/withhold.rs` (the `Gate` struct ~:121, the groups-only skip ~:485-513, `everyone_except` ~:350); Modify `crates/egret-perm-compile/src/ir_load.rs` (`load_withhold_inputs` ~:388, `GateRow` ~:354); Test `clearance_migrate.rs`.

**Interfaces:**
- Consumes: `Gate { classification_id, classification_name, required_roles, required_groups_empty, cleared, targets }` (`withhold.rs:121`); `members::role_usernames` / the group member expansion used elsewhere (`members.rs`).
- Produces: `Gate` whose `cleared` is the sorted-deduped union of (a) holders of each `required_roles` role AND (b) members of each `required_groups` group. The `required_groups_empty` field and the groups-only skip arm are removed; a gate with a non-empty exemption set (roles or groups) lowers to a withhold whose audience excepts that set.

- [ ] **Step 1: Write the failing test** — a DB-gated test seeding a GROUPS-ONLY clearance (a classification with `required_groups={G}`, `required_roles={}`, where group G has member `bob`) and compiling the service; assert the compiled withhold now excepts `bob` (i.e. it is enforced, denying everyone except bob) — which fails today because groups-only compiles to nothing. (StarRocks fork image seed required.)

- [ ] **Step 2: Run it** — FAIL (groups-only compiles to nothing today).

- [ ] **Step 3: Resolve required_groups into `cleared`** in `load_withhold_inputs`: after resolving `required_roles` holders, also resolve each `required_groups` UUID to its member usernames (reuse the group-member expansion that `members.rs` uses for group principals) and extend `cleared`. Remove `required_groups_empty` from `Gate` and `GateRow` once nothing reads it. In `withhold.rs::lower_withholds`, delete the groups-only early-skip arm (~:485-513): now every gate with a non-empty `cleared` (or non-empty exemption set) lowers normally via `everyone_except(&cleared)`.

- [ ] **Step 4: Run it** — PASS (groups-only now enforced). Run the whole withhold/clearance test set single-threaded; the existing role-based clearance tests still pass byte-identically.

- [ ] **Step 5: Commit** `fix(perm-ir): groups-only clearance resolves its exemptions instead of compiling to nothing`.

> This task fixes the enforcement gap independent of where the rule is stored. Tasks 3–5 move the STORAGE to perm.rules; this task makes the lowering correct for both roles and groups first (smaller, independently testable).

---

### Task 3: `clearance_migration` provenance table

**Files:** Create `migrations/0223_clearance_migration.sql` (or 0224 per Task 1 note); Test `clearance_migrate.rs`.

**Interfaces:**
- Produces: `perm.clearance_migration (id bigserial PK, tags text[], rules jsonb, rule_ids uuid[], exemption_hashes text[], original jsonb, migrated_by text, migrated_at timestamptz)` recording, per migration run, the classification rows consumed and the withhold rules + exemptions created, enough to undo exactly.

- [ ] **Step 1:** Write the migration (one row per migration run; `original` holds the pre-migration `classification_policies` rows as JSON for restore; `rule_ids` the created withhold rule ids; `exemption_hashes` a canonical hash per created rule for the undo guard).
- [ ] **Step 2-4:** A test inserts and reads back a row.
- [ ] **Step 5: Commit** `feat(perm): clearance migration provenance table`.

---

### Task 4: Reversible clearance → withhold-rule migration (+ equivalence gate)

**Files:** Create `crates/egret-perm-compile/src/clearance_migrate.rs`; Modify `lib.rs`; Test `clearance_migrate.rs`.

**Interfaces:**
- Consumes: `role_holder_group::{role_holder_group_name, sync_role_holder_group_if_present_in_tx}` (SP3c-1); the write seam for rules (`write_rule` / direct inserts); the equivalence helpers (`equivalence::derive_probes`) and hash patterns (`migrate.rs::canonical_json`).
- Produces:
  - `pub async fn migrate_clearances_in_tx(tx, migrated_by: &str) -> Result<ClearanceMigration, PermHttpError>` — for each `classification_policies` row: create a `perm.rules` withhold rule (`kind='withhold'`, `selector_kind='tag'`, `tag_key = tags.key` for the row's `tag_id`, a generated unique `name`); create `perm.rule_exemptions` for each `required_roles` (→ `kind='group'`, `ref=role_holder_group_name(role)`, `mode='include'`; ensure the holder group exists via `sync_role_holder_group_if_present_in_tx` or create it) and each `required_groups` UUID (→ `kind='group'`, `ref=<groups.name>`, `mode='include'`); record provenance; recompile affected; run the equivalence gate EXCLUDING groups-only clearances (role-bearing clearances must stay byte-identical, groups-only legitimately changes); refuse on unexpected divergence; return counts.
  - `pub async fn undo_clearances_in_tx(tx, migration_id: i64) -> Result<(), PermHttpError>` — FOR UPDATE the provenance row; recompute exemption hashes from current rows; 409 on divergence; restore `classification_policies` from `original`; delete the withhold rules + exemptions; recompile; delete the provenance row.
  - `ClearanceMigration { id: i64, tags: Vec<String>, rules_created: usize }`.

- [ ] **Step 1: Write the failing test** — seed the demo-shaped role-based clearance (`required_roles={data_steward}`, holder carol); capture the compiled withhold output BEFORE; run `migrate_clearances_in_tx`; assert (a) a `perm.rules` withhold rule + its `role:data_steward holders` exemption now exist, and (b) the compiled output is byte-identical to before (role-based clearance).
- [ ] **Step 2: Run it** — FAIL (fn not present).
- [ ] **Step 3: Implement** the migration + provenance + gate (exclude groups-only from the byte-identical assertion; for groups-only, assert it is now enforced). `load_withhold_inputs` is switched to read the withhold rules in Task 5; until then, this task can gate by comparing the withhold IR built from `classification_policies` vs from the new rules — OR sequence Task 5 before the gate. (Implementer: if the gate needs the new read path, do Task 5 first and reorder; keep each commit test-green.)
- [ ] **Step 4: Run it** — PASS. Then the undo test: migrate, undo untouched → `classification_policies` restored + withhold rules gone; undo with a tampered exemption → 409.
- [ ] **Step 5: Commit** `feat(perm-compile): reversible clearance -> withhold-rule migration with equivalence gate`.

---

### Task 5: Read clearance from the withhold rules, not `classification_policies`

**Files:** Modify `crates/egret-perm-compile/src/ir_load.rs` (`load_withhold_inputs`); Modify `crates/egret-perm-compile/src/role_membership.rs` (chokepoint clearance arm); Test `clearance_migrate.rs`.

**Interfaces:**
- Consumes: `perm.rules` (`kind='withhold'`) + `perm.rule_exemptions`; the principal directory expansion.
- Produces: `load_withhold_inputs` builds its `Gate`s from the withhold rules (tag_key → classification) and expands their exemptions into `cleared` (roles-as-holder-groups and groups both resolve through the group path). `classification_policies` is no longer read for enforcement. The chokepoint finds clearance-affected profiles via the withhold rules' tag selectors + exemptions.

- [ ] **Step 1: Write the failing test** — after `migrate_clearances_in_tx`, DELETE the `classification_policies` row (simulating that enforcement no longer depends on it) and recompile; assert the clearance is STILL enforced (comes from the withhold rule). Today this fails (enforcement reads the deleted table).
- [ ] **Step 2: Run it** — FAIL.
- [ ] **Step 3: Rewire** `load_withhold_inputs` to SELECT withhold rules + exemptions and expand exemptions into `cleared`. Update the chokepoint's clearance arm (`role_membership.rs`) to match withhold-rule exemptions referencing a role's holder group (so a holder change still recompiles the clearance's profiles). Keep `needs_targets`/tag-assignment logic.
- [ ] **Step 4: Run it** — PASS. Full clearance test set single-threaded green; role-based clearance still byte-identical.
- [ ] **Step 5: Commit** `feat(perm-compile): clearance enforcement reads the withhold rules`.

---

### Task 6: Keep the withhold rule in step with clearance authoring

**Files:** Modify the clearance-authoring write path (the handler behind `POST` set-clearance that writes `classification_policies` — find it via `classification_policies` INSERT/UPDATE sites); Test `clearance_migrate.rs`.

**Interfaces:**
- Consumes: the clearance write path (sets `required_roles`/`required_groups` on `classification_policies`).
- Produces: after a clearance is created/edited/removed through the UI/API, its withhold rule + exemptions are created/updated/removed to match, so enforcement (which now reads the rule) reflects the authored clearance. (This keeps `classification_policies` as the authoring store without re-introducing a second source of truth for enforcement.)

- [ ] **Step 1: Write the failing test** — set a clearance via the write path AFTER migration; assert a matching withhold rule + exemptions appear and the clearance is enforced; edit it (change required_roles); assert the rule's exemptions update; clear it; assert the rule is removed.
- [ ] **Step 2: Run it** — FAIL.
- [ ] **Step 3: Implement** the sync in the clearance write path (reuse the migration's rule/exemption upsert logic; factor a shared `upsert_clearance_withhold_rule_in_tx`).
- [ ] **Step 4: Run it** — PASS.
- [ ] **Step 5: Commit** `feat(perm-compile): clearance authoring maintains its withhold rule`.

---

### Task 7: Admin endpoints + explain-unchanged proof

**Files:** Modify `crates/egret-perm-compile/src/routes.rs` (endpoints `POST /perm/migrate/clearances` + `/undo`, mirroring the role-principal endpoints; regen `openapi.json` + client); Test `clearance_migrate.rs`.

- [ ] **Step 1:** Endpoint smoke test (migrate returns 200 + rule created; undo restores). Explain test: capture explain for a cleared vs non-cleared principal before and after migration on the role-based clearance; assert byte-identical.
- [ ] **Step 2-4:** Implement endpoints + `#[utoipa::path]` + ApiDoc; regen openapi (`cargo run --bin egret -- openapi > openapi.json`) + `npm run gen:api`; confirm digest clean; run the full `clearance_migrate` file + `cargo clippy` + `cargo fmt --check` green.
- [ ] **Step 5: Commit** `feat(perm-compile): clearance migration endpoints + explain-stable test`.

---

## Out of scope (later SP3c packages / follow-ups)

- Implicit workspace grant → explicit `ws:<slug> members` grant (SP3c-3).
- Moving clearance AUTHORING off `classification_policies` entirely (SP3c-2 keeps it as the author store, synced to the rule). A later package may collapse them.
- Per-EP gate + switch demo + live screenshot (SP3d).
- The `Effect::WithholdMask` IR variant stays unused unless a withhold view-guard needs it.

## Self-Review

- **Spec coverage (SP3c-2):** classification_policies → perm.rules withhold (Tasks 1,3,4,5); exemptions = required_roles via holder groups + required_groups (Task 4); groups-only enforced on both engines (Task 2); classification_policies out of the enforcement input (Task 5); explain unchanged for role-based (Task 7); reversible provenance (Tasks 3,4). ✅
- **Decision note:** the equivalence gate is byte-identical ONLY for role-bearing clearances; groups-only is an intentional behavior change (was a bug) proven by a seeded test — stated in Global Constraints and Task 2/4. The demo stack's one clearance is role-based, so the live migration is fully byte-identical.
- **Type consistency:** `migrate_clearances_in_tx` / `undo_clearances_in_tx` / `ClearanceMigration` / `upsert_clearance_withhold_rule_in_tx` named consistently; `role_holder_group_name` reused from SP3c-1.
