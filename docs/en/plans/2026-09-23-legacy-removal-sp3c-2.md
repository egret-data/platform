# SP3c-2 — the legacy removal — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the warehouse-shaped world the catalog registry replaced — the legacy catalog table and its boot backfill, the six legacy engine-catalog endpoints, `engines.warehouse_id`, the `warehouse:*` permission family and the Warehouses UI section — in one PR, with every URL unchanged.

**Architecture:** Deletion, in the order the design's §4 fixes, because each removal has a precondition the one before it satisfies. The backfill and the legacy table leave together (they are each other's only reason to exist). `configError` moves onto the registry list before the route that reports it is deleted. `engines.warehouse_id`'s readers move onto the primary attachment while the column still exists, and the column drops in its own commit with its own live check. The permission re-gate changes `require()` strings and nothing else — no path, no OpenAPI tag, no generated function name.

**Tech Stack:** Rust (axum, sqlx 0.9, utoipa), Postgres migrations, React + TanStack Query, orval-generated client, Vitest, Playwright, kind.

## Global Constraints

- **Migrations are append-only once applied anywhere.** Never edit `0147`–`0150`. This plan adds `0151`, `0152`, `0153` and nothing else. (SP3a edited `0147` after deployment and broke kind with `VersionMismatch(147)`.)
- **sqlx 0.9:** `sqlx::query()` takes `&'static str`. Never build SQL with `format!`. Bind parameters only.
- **Never delete a secret value the cluster or an engine may still hold** (user decision, 2026-09-16). Dropping a table that references secrets does not license deleting the secret rows.
- **Engine delete never deletes Ranger profiles; unbind only.**
- **Redaction is for humans and columns, never for what code branches on.**
- **URLs and OpenAPI tags do not change** (user decision, 2026-09-23). `/control-plane/warehouses/{id}/…` keeps its paths and `tag = "control-plane-catalog"`. Re-gating changes the `require()` string and the 403 description text, nothing else.
- **The primary attachment must name a catalog that has a warehouse** (user decision, 2026-09-23). Letting a non-warehouse catalog lead is explicitly out of scope.
- **SP3a/3b/3c-1 is a required upgrade hop** (user decision, 2026-09-23). `0151` raises rather than dropping data it cannot carry forward.
- **Frontend client:** regenerate with `npm run gen:api` from `frontend/`, never bare `orval` (bare orval strips `encodeURIComponent`). Commit exactly what it produces.
- **`committed_openapi_json_is_current` must pass** after any route or DTO change.
- **Test database:** `egret_test`, never the dev database `egret`. Full env for DB-gated tests:
  ```bash
  export CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret/target
  export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
  export EGRET_TEST_SHARED_STORE_URL=redis://localhost:6399
  ```
  and run with `env -u EGRET_ALLOW_DB_SKIP` — without it, DB-gated tests report `ok` while asserting nothing.
- **Never use `git stash`.** Never override the git author. Never merge without the user's explicit approval.
- **Do not modify the kind `demo-trino` data** (26 endpoints, alerts, datasets).
- **Test hygiene, enforced in review:** no test that cannot fail; no fixture containing the literal string its own assertion searches for; no absence assertion read before the thing could have appeared; no guard whose condition cannot occur. When a test is deleted, say in the commit why nothing else must cover it.
- Commit trailer on every commit:
  ```text
  Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
  ```

---

## File Structure

**Deleted outright:**
- `crates/egret-catalog-registry/src/backfill.rs` (809 lines) — the boot migration from the legacy table.
- `crates/egret-catalog-registry/src/legacy.rs` (664 lines) — `classify` / `LegacyCatalog`, in **Task 4**: its last consumers are `add_catalog` and `update_catalog`, not the backfill.
- `crates/egret-catalog-registry/tests/backfill_*.rs` (9 files).
- `frontend/src/routes/control-plane/Warehouses.tsx` (401 lines) and its two test files.

**Shrunk:**
- `crates/egret-engine-mgmt/src/engines.rs` — six route handlers, their DTOs and helpers, `EngineDto.warehouse_id`, the apply guard, the StarRocks storage derivation's source.
- `crates/egret-engine-mgmt/src/store.rs` — five dead store methods, `Engine.warehouse_id`, four column lists, the `create_engine` parameter.
- `crates/egret-catalog-registry/src/store.rs` — `map_legacy_row_on`, `unmigrated_legacy_catalog_count`.
- `crates/egret-policy/src/store.rs` and `crates/egret-catalog-registry/src/tag_version.rs` — one legacy `UNION` arm each.
- `crates/egret-catalog-engine/src/warehouse_admin/mod.rs` — 27 `require()` strings.
- `crates/egret-core/src/rbac.rs` — four `perm!` entries.
- `frontend/src/App.tsx`, `components/layout/Sidebar.tsx`, `auth/appAccess.ts`.

**Added:**
- `migrations/0151_drop_legacy_engine_catalogs.sql`
- `migrations/0152_drop_engines_warehouse_id.sql`
- `migrations/0153_retire_warehouse_permissions.sql`
- `crates/egret-engine-mgmt/tests/registry_catalog_config_error.rs` — the new home of the unparseable-catalog test.
- Migration tests for `0151` (refusal) and `0153` (grant-before-delete).

---

### Task 1: Retire the boot backfill

The legacy table's other three production readers all exist to serve the backfill or report its failure. They leave first, as Rust-only deletion, so Task 2's migration meets a tree where nothing reads the table but the two legacy `UNION` arms.

**Files:**
- Delete: `crates/egret-catalog-registry/src/backfill.rs`
- **Keep** `crates/egret-catalog-registry/src/legacy.rs` — corrected 2026-09-23. It has a second live consumer the survey missed: `engines.rs:5761` and `:6099` build a `LegacyCatalog` and call `classify` / `classify_generic` to shape untyped catalog request bodies, and `catalog_apply.rs:783` takes one too. All three sit inside `add_catalog` and `update_catalog` — the routes **Task 4** deletes. So `legacy.rs` dies in Task 4, not here.
- Delete: `crates/egret-catalog-registry/tests/backfill_kind_cluster_report.rs`, `backfill_partial_run_and_scan_recovery.rs`, `backfill_primacy_matches_migration_0150.rs`, `backfill_secret_collision_survives_crash.rs`, `backfill_secret_collision.rs`, `backfill_several_primaries.rs`, `backfill_single_primary.rs`, `backfill_step1_fails_step2_recovers_primary_name.rs`, `backfill_warehouse_failure_isolated.rs`
- Modify: `crates/egret-catalog-registry/src/lib.rs:19` (the `pub use backfill::…` re-export, and the `mod backfill;` / `mod legacy;` declarations)
- Modify: `crates/egret-catalog-registry/src/store.rs` — delete `map_legacy_row_on` (`:289-303`) and `unmigrated_legacy_catalog_count` (`:948-962`)
- Modify: `crates/egret-bin/src/main.rs:1263-1274` — the `run_backfill` call and its `match`
- Modify: `crates/egret-engine-mgmt/src/engines.rs:641-653` — the apply guard inside `build_provision_req_with`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `egret_catalog_registry` no longer exports `run_backfill`, `BackfillSummary` or `BACKFILL_LOCK_KEY`. `RegistryStore` no longer has `map_legacy_row_on` or `unmigrated_legacy_catalog_count`. `build_provision_req_with` no longer returns the "boot-time catalog registry migration could not move …" 409.

- [ ] **Step 1: Read the apply guard before deleting it**

Read `crates/egret-engine-mgmt/src/engines.rs:630-660` in full. The guard is an early return inside `build_provision_req_with`; confirm exactly which `?`/`return` it owns and that removing it leaves the surrounding `Result` flow intact. Do not delete by line number without reading.

- [ ] **Step 2: Delete the two modules and their declarations**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
git rm crates/egret-catalog-registry/src/backfill.rs
git rm crates/egret-catalog-registry/tests/backfill_*.rs
```

Then remove `mod backfill;` and the `pub use backfill::{…}` line from `crates/egret-catalog-registry/src/lib.rs`. **Leave `mod legacy;` and its `pub use` alone** — see the Files note above.

- [ ] **Step 3: Delete the three remaining call sites**

- `crates/egret-bin/src/main.rs`: remove the whole `match egret_catalog_registry::run_backfill(…)` block at `:1263`, including any `tracing` lines that only report its summary, and any now-unused import.
- `crates/egret-catalog-registry/src/store.rs`: remove `map_legacy_row_on` and `unmigrated_legacy_catalog_count` and their doc comments.
- `crates/egret-engine-mgmt/src/engines.rs`: remove the apply guard identified in Step 1.

- [ ] **Step 4: Build and let the compiler find the rest**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
CARGO_TARGET_DIR=$PWD/target cargo build --workspace 2>&1 | tail -40
```

Expected: clean, or errors naming further references to the deleted items. Fix each by deletion, never by re-adding a shim. If a test file outside `crates/egret-catalog-registry/tests/` references `run_backfill`, it belongs to this deletion too — name it in the commit message.

- [ ] **Step 5: Confirm nothing but the two legacy arms still reads the table**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
grep -rn "engine_mgmt.engine_catalogs" crates/ | grep -v "/tests/"
```

Expected exactly two SQL sites — `crates/egret-policy/src/store.rs` (inside `tag_assignments_for_service`) and the store methods in `crates/egret-engine-mgmt/src/store.rs` — plus doc comments. If a fourth SQL site appears, stop and report it: the survey missed a reader.

- [ ] **Step 6: Run the affected suites**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
export CARGO_TARGET_DIR=$PWD/target DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test EGRET_TEST_SHARED_STORE_URL=redis://localhost:6399
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-catalog-registry -p egret-policy 2>&1 | tail -30
```

Expected: pass. `egret-policy`'s `download_tags_*` tests still pass here because the legacy arm is still present — they are Task 2's to rewrite.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -F - <<'EOF'
refactor: retire the boot catalog backfill (SP3c-2)

`run_backfill` migrated `engine_mgmt.engine_catalogs` into the registry
at every boot. With SP3a/3b/3c-1 declared a required upgrade hop (design
§4 step 1), it has no remaining job, and it was three of the legacy
table's four production readers: both scan queries and the
`unmigrated_legacy_catalog_count` guard that 409'd every Trino apply.

Deleted with it: nine DB test files that exist to test the migration
itself. `legacy.rs` stays — `add_catalog` and `update_catalog` still
consume `classify`, so it dies in Task 4 with those routes.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

---

### Task 2: Drop `engine_mgmt.engine_catalogs`

**Files:**
- Create: `migrations/0151_drop_legacy_engine_catalogs.sql`
- Create: `crates/egret-catalog-registry/tests/migration_0151_refuses_unmapped_rows.rs`
- Modify: `crates/egret-engine-mgmt/src/store.rs:423-504` — delete `create_engine_catalog`, `list_engine_catalogs`, `get_engine_catalog`, `update_engine_catalog`, `delete_engine_catalog` and the `EngineCatalog` row struct if nothing else uses it
- Modify: `crates/egret-policy/src/store.rs:206-289` — delete the legacy `UNION` arm and rewrite the doc comment
- Modify: `crates/egret-catalog-registry/src/tag_version.rs:26-90` — delete the legacy `UNION` arm and rewrite the doc comment
- Delete: `crates/egret-catalog-registry/src/report.rs` and its `mod` / `pub use` in `lib.rs`
- Modify: `crates/egret-engine-mgmt/src/catalogs.rs` — the two migration-report routes (`:39`, `:41-42`), `get_migration_report` (`:1015-1052`), `acknowledge_migration_report` (`:1054-1075`), `MigrationReportRowDto` (`:554`), `MigrationReportQuery` (`:573`), their utoipa registrations (`:1089-1090`, `:1108`)
- Modify: `crates/egret-catalog-registry/src/store.rs` — `insert_report_row`, `insert_report`, `list_report`, `acknowledge_report`, and `ensure_warehouse_catalog`'s `report_for` parameter
- Modify: the five `ensure_warehouse_catalog` call sites that pass `|_| Vec::new()` — `catalog-engine/src/warehouse_admin/mod.rs:417`, `engine-mgmt/src/engines.rs:3050`, `:3777`, `:5740`, `:9093`
- Modify: `crates/egret-engine-mgmt/tests/store.rs:93-224` (the five method tests), `tests/dependents.rs:235`, `tests/routes.rs:5606`, `tests/registry_schema_constraints.rs:484-510`, `tests/routes.rs:8302-8410`
- Modify: `crates/egret-policy/tests/download_tags_db.rs`, `download_tags_after_backfill.rs`, `download_tags_backfill_atomicity.rs`, `crates/egret-server/tests/tag_bump_reaches_polled_tag_service.rs`

**Interfaces:**
- Consumes: Task 1's tree — no backfill, no apply guard.
- Produces: `tag_assignments_for_service` and `bump_bound_tag_versions` are registry-only. `EngineMgmtStore` has no engine-catalog methods.

- [ ] **Step 1: Write the migration**

`migrations/0151_drop_legacy_engine_catalogs.sql`:

```sql
-- 0151_drop_legacy_engine_catalogs.sql
-- SP3c-2 §4 step 1. The registry (0147) replaced `engine_mgmt.engine_catalogs`,
-- and 0147's boot backfill — deleted in this release — was the only thing that
-- moved rows across. So this migration REFUSES rather than drops data it cannot
-- carry forward: an operator on a pre-SP3a database upgrades through SP3c-1 and
-- boots once, which runs the backfill, and then comes here.
DO $$
DECLARE
    unmapped bigint;
BEGIN
    SELECT count(*) INTO unmapped
      FROM engine_mgmt.engine_catalogs c
      LEFT JOIN registry.legacy_engine_catalogs m ON m.engine_catalog_id = c.id
     WHERE m.engine_catalog_id IS NULL;

    IF unmapped > 0 THEN
        RAISE EXCEPTION
            'engine_mgmt.engine_catalogs still holds % row(s) the catalog registry has not migrated. Upgrade to the SP3c-1 release first and start egret once so the boot backfill runs, then apply this release.',
            unmapped;
    END IF;
END $$;

-- `migration_report` recorded what the backfill did. Nothing writes it now: its
-- other writer, `ensure_warehouse_catalog`, takes a `report_for` closure and
-- every production caller passes one that returns an empty Vec (see step 5b).
DROP TABLE registry.migration_report;
DROP TABLE registry.legacy_engine_catalogs;
DROP TABLE engine_mgmt.engine_catalogs;
```

The map column is `engine_catalog_id` (`migrations/0147_catalog_registry.sql:87-92`) — confirmed, not guessed. Read that block anyway before writing the file; if `registry.migration_report` carries a foreign key into either dropped table, the drop order above must satisfy it.

- [ ] **Step 2: Write the failing migration test**

`crates/egret-catalog-registry/tests/migration_0151_refuses_unmapped_rows.rs`. It must exercise the refusal on a database that still has the table, so it cannot use the normal migrated fixture. Follow the pattern in `crates/egret-catalog-registry/tests/` for acquiring a pool, then:

```rust
// Arrange: a legacy row with no mapping, on a database where 0151 has not run.
// Act: run the 0151 body.
// Assert: it raises, and the message names the SP3c-1 hop.
```

Write it concretely against whatever harness the sibling tests use. The assertion must check the *message*, not merely that an error occurred — a syntax error in the `DO` block also produces an error, and a test that cannot tell them apart passes for the wrong reason. Assert the text contains `SP3c-1`.

- [ ] **Step 3: Run it and watch it fail**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
export CARGO_TARGET_DIR=$PWD/target DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test EGRET_TEST_SHARED_STORE_URL=redis://localhost:6399
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-catalog-registry migration_0151 2>&1 | tail -20
```

Expected: FAIL before the migration file exists or before its message is right.

- [ ] **Step 4: Delete the two legacy `UNION` arms**

In `crates/egret-policy/src/store.rs`, `tag_assignments_for_service`: remove the second `SELECT` of the `UNION` (the `engine_mgmt.engine_catalogs` join at `:266-284`) and the `UNION` keyword, leaving the registry branch as the whole query. Rewrite the doc comment at `:206-232`: it currently explains why the legacy branch stays. Replace that explanation — do not leave it describing code that no longer exists.

The replacement text states the two reasons and why each is gone:

```text
/// One branch only, since SP3c-2: the engine attaches this warehouse's
/// registry catalog through `registry.engine_catalog_attachments`.
///
/// Until SP3c-2 a second branch read `engine_mgmt.engine_catalogs` for two
/// populations the registry could not see: engines whose backfill failed,
/// and a catalog renamed but not yet re-applied. Both are gone — the backfill
/// is deleted, so no engine can be in the first state, and
/// `registry.catalogs.name` is UNIQUE and immutable, so nothing renames a
/// catalog at all.
```

Do the same in `crates/egret-catalog-registry/src/tag_version.rs` for `bump_bound_tag_versions` (the `engines.warehouse_id` arm at `:61-64` and the doc at `:26-39`). Keep the N1 paragraph about bumping the polled tag service — that is still true and load-bearing.

- [ ] **Step 5: Delete the five store methods and their tests**

Remove `create_engine_catalog` / `list_engine_catalogs` / `get_engine_catalog` / `update_engine_catalog` / `delete_engine_catalog` from `crates/egret-engine-mgmt/src/store.rs`, and the tests at `crates/egret-engine-mgmt/tests/store.rs:93-224`. Do **not** touch `crates/egret-query/src/registry/repo.rs:509`'s identically-named `list_engine_catalogs` — it reads `registry.catalogs` and is a name collision.

Remove the `engine_mgmt.engine_catalogs` row from the FK-cascade table list in `crates/egret-engine-mgmt/tests/dependents.rs:235`, and the seed at `tests/routes.rs:5606`.

- [ ] **Step 5b: Delete the migration-report facility**

This entry has been wrong twice; here is the evidence rather than the conclusion.
`registry.migration_report` is written in exactly two places. `RegistryStore::insert_report`
has test-only callers. `insert_report_row` inside `ensure_warehouse_catalog`
(`catalog-registry/src/store.rs:555`) writes whatever the `report_for` closure returns —
and all five production callers pass `|_| Vec::new()`:

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret-sp3c2
grep -n -A8 "ensure_warehouse_catalog(" crates/egret-catalog-engine/src/warehouse_admin/mod.rs \
  crates/egret-engine-mgmt/src/engines.rs | grep -E "\|_\| Vec::new|ensure_warehouse_catalog"
```

Re-run that grep before deleting. If any caller now passes a closure that can return a
non-empty `Vec`, STOP: the facility has a live writer again and none of this applies.

(The mistake to avoid: confirming that `ensure_warehouse_catalog` has five live callers
proves the *function* is live, not that it *writes a report row*. The closure is the writer.
I checked the call sites exist and concluded the table was written — it is not.)

With no writer, delete: `registry.migration_report` (in `0151`), `report.rs`,
`insert_report_row`, `RegistryStore::insert_report` / `list_report` / `acknowledge_report`,
the `report_for` parameter of `ensure_warehouse_catalog` and the `|_| Vec::new()` at all five
call sites, the two routes (`catalogs.rs:39`, `:41-42`), `get_migration_report`,
`acknowledge_migration_report`, `MigrationReportRowDto`, `MigrationReportQuery` and their
utoipa registrations, and the two tests (`registry_schema_constraints.rs:484-510`;
`routes.rs:8302-8410` — keep that test's workspace-assignment half if it is the only coverage
of that behaviour, and drop only the report assertions). `routes.rs:8524`'s comment about
which permission the migration-report GETs require becomes false.

Add `DROP TABLE registry.migration_report;` to `0151`, before the two drops already there.

First verify the frontend has no consumer:

```bash
grep -rn "migrationReport\|migration-report" frontend/src | grep -v "src/api/generated"
```

Expected: no output — SP3c-1 never built the Catalogs-page banner `0147` anticipated. If a
component does consume it, STOP and report.

- [ ] **Step 6: Rewrite the policy tests that pin the legacy branch**

Four tests and one helper in `crates/egret-policy/tests/download_tags_db.rs`:
- `seed_legacy_primary` (`:128-140`) — delete.
- `download_tags_legacy_only_engine_exports_legacy_name` (`:349`) — delete. Its subject (an engine with a legacy row and no attachment) can no longer exist; say so in the commit.
- `download_tags_rename_window_exports_both_names` (`:398`) — delete, and in the same commit note that immutability now makes the window unrepresentable.
- `…neither_attachment_nor_legacy_row_exports_nothing` (`:564`) — **keep**, rewritten to "no attachment exports nothing". This is the fail-closed direction and must not be lost.

`download_tags_after_backfill.rs` and `download_tags_backfill_atomicity.rs` test the deleted backfill: delete both files, and state in the commit that the behaviour they covered (a tag bump inside the attachment's transaction) is now covered by `crates/egret-server/tests/tag_bump_reaches_polled_tag_service.rs`. **Verify that claim by reading that file before writing it** — if it does not cover the bump, keep an adapted test instead.

Remove the `INSERT INTO engine_mgmt.engine_catalogs` seeds from `crates/egret-server/tests/tag_bump_reaches_polled_tag_service.rs:108`.

- [ ] **Step 7: Run the migration and the suites**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
export CARGO_TARGET_DIR=$PWD/target DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test EGRET_TEST_SHARED_STORE_URL=redis://localhost:6399
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-catalog-registry -p egret-policy -p egret-engine-mgmt -p egret-server 2>&1 | tail -40
```

Expected: pass, including `migration_0151_refuses_unmapped_rows`.

- [ ] **Step 8: Prove the tag export still works without the legacy branch**

The tag export failing open is this programme's worst defect (SP3a Task 10). Confirm the surviving registry branch still exports for a normally-attached engine:

```bash
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-policy download_tags 2>&1 | tail -20
```

Expected: the attachment-based tests pass. If every remaining `download_tags` test is an absence assertion, add one that asserts a **present** tag for an attached engine — an export that returns nothing always satisfies "exports nothing".

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -F - <<'EOF'
feat: drop engine_mgmt.engine_catalogs (SP3c-2)

0151 refuses rather than drops: if a legacy row remains that
`registry.legacy_engine_catalogs` never mapped, it raises and names the
SP3c-1 hop, because this release deletes the only code that could have
moved it.

Both legacy UNION arms go with the table. They existed for engines whose
backfill failed and for a rename window; the backfill is gone, so the
first population cannot exist, and `registry.catalogs.name` is UNIQUE and
immutable, so the second never happens.

`download_tags_…_neither_attachment_nor_legacy_row_exports_nothing` is
kept, rewritten — it is the fail-closed direction and this programme has
already shipped one fail-open tag export.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

---

### Task 3: `configError` onto `GET /catalogs`

The precondition for Task 4. `GET /catalogs` today sets `config` from the raw jsonb column and parses nothing; it must learn to report a parse failure **without** being able to fail on one.

**Files:**
- Modify: `crates/egret-engine-mgmt/src/catalogs.rs:107-124` (`RegistryCatalogDto`), `:237-269` (`to_dto`)
- Create: `crates/egret-engine-mgmt/tests/registry_catalog_config_error.rs`
- Modify: `frontend/src/routes/control-plane/EngineDetail.tsx` — `catalogsQ` (`:674-679`), `legacyQ` (`:1349-1353`), the 403 fallback (`:1398-1412`)
- Modify: `frontend/src/routes/control-plane/catalogs/EnginesTab.tsx:352-369` and `catalogs/attachmentUi.tsx:16-29` — the two deliberate `null`s
- Regenerate: `frontend/src/api/generated/**`

**Interfaces:**
- Consumes: Task 2's tree.
- Produces: `RegistryCatalogDto.configError?: string` on `GET /catalogs` and `GET /catalogs/{id}`. `attachmentUi`'s `retryBlockedReason` can be a real string.

- [ ] **Step 1: Write the failing backend test**

`crates/egret-engine-mgmt/tests/registry_catalog_config_error.rs`. Two cases, and the first is the one that matters:

```rust
// A catalog row whose stored config no longer parses must not take the list
// with it: `GET /catalogs` returns 200, lists BOTH catalogs, and carries
// `configError` on the broken one only.
//
// The broken row is written directly with sqlx, bypassing the validator —
// the API cannot create one, which is exactly why this case is reachable
// only from a stored row that predates a tightened validator.
```

Seed two registry catalogs: one valid, one whose `config` column is a JSON object the current `CatalogDef` rejects (e.g. `{"kind":"jdbc"}` with the required connection fields absent — confirm against `crates/egret-catalog-registry/src/config.rs` which shape is actually refused, and assert `row.def()` errs on it in the test's arrange phase so the fixture cannot silently become valid). Then `GET /api/control-plane/catalogs` and assert:
- status 200
- the response array has length 2
- the broken entry has a non-empty `configError`
- the valid entry has no `configError` key at all

- [ ] **Step 2: Run it and watch it fail**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
export CARGO_TARGET_DIR=$PWD/target DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test EGRET_TEST_SHARED_STORE_URL=redis://localhost:6399
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-engine-mgmt registry_catalog_config_error 2>&1 | tail -20
```

Expected: FAIL — no `configError` field exists.

- [ ] **Step 3: Add the field, degrading**

In `crates/egret-engine-mgmt/src/catalogs.rs`, add to `RegistryCatalogDto`:

```rust
    /// Why this row's stored `config` no longer parses, if it does not.
    /// A stored row the tightened validator now refuses stays visible and
    /// repairable instead of vanishing — and, critically, never fails the
    /// whole list (SP3c-1 review N1).
    #[serde(skip_serializing_if = "Option::is_none")]
    config_error: Option<String>,
```

and in `to_dto`:

```rust
    let config_error = row.def().err().map(|e| e.to_string());
```

placed before the struct literal, with `config_error` added to it. `to_dto`'s signature and its `?` on the two registry queries are unchanged — the parse result is *recorded*, never propagated.

- [ ] **Step 4: Run the test and the drift check**

```bash
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-engine-mgmt registry_catalog_config_error 2>&1 | tail -20
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-server committed_openapi_json_is_current 2>&1 | tail -20
```

Expected: the first passes; the second fails until the committed OpenAPI is regenerated. Regenerate it the way the repo already does (the failing test names the command).

- [ ] **Step 5: Regenerate the client**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret/frontend
npm run gen:api
git diff --stat frontend/src/api/generated | tail -5
```

Commit exactly what it produces. Expected: `registryCatalogDto.ts` gains `configError?: string`.

- [ ] **Step 6: Re-point the frontend onto the registry list**

In `frontend/src/routes/control-plane/EngineDetail.tsx`:
- `EngineSummaryStrip`'s `catalogsQ` (`:674-679`) and the detail page's `legacyQ` (`:1349-1353`) both fetch the legacy list. Both now read from the registry list the page already holds (`registryQ`, `:1341-1347`), filtered to this engine's attachments.
- `AttachedRow.configError` (`:1323`) and its consumers (`:1396`, `:1410`, `:1415`, `:1464`, `:1543`, `:1668-1700`) take the field from the registry row instead.
- `configChecked` (`:1391-1397`) exists to distinguish "not checked" from "clean" — with one query it is always checked, so delete it and the "rows the registry did not show" fallback at `:1398-1412` with it. Read that block before deleting: if it carries user-facing copy that is still true, move the copy rather than dropping it.

Read each site before editing. These line numbers are from a survey, not from your working tree.

- [ ] **Step 7: Unblock the two deliberate `null`s**

`frontend/src/routes/control-plane/catalogs/EnginesTab.tsx:352-369` passes `retryBlockedReason={null}` with a comment saying it waits for this move; `catalogs/attachmentUi.tsx:16-29` carries the matching design note. Supply the real reason from `configError` and update both comments so neither still says it is waiting.

- [ ] **Step 8: Vitest**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret/frontend
npx vitest run src/routes/control-plane/__tests__/EngineDetail.test.tsx src/routes/control-plane/catalogs/__tests__ 2>&1 | tail -30
```

Expected: pass. Where a test mocked `listEngineCatalogs`, it now mocks the registry list — a test still passing against a mock of a query the page no longer issues is a test that cannot fail. Check each one you touch actually drives the new path.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -F - <<'EOF'
feat: report configError on the registry catalog list (SP3c-2)

`GET /catalogs` set `config` from the raw column and parsed nothing, so
`GET /engines/{id}/catalogs` was the only route that could tell an
operator why a stored row no longer validates. That route is deleted
next, and `catalog_apply.rs` deliberately routes `ApplyError::Build`
here instead of writing `apply_status = failed`, so the field has to
move before the route goes or a Trino build failure is invisible on both
surfaces.

The parse degrades and never propagates: one refused row must not fail
the whole list, which is the defect SP3c-1's review N1 fixed on the other
route.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

---

### Task 4: Delete the six legacy `/engines/{id}/catalogs*` endpoints

**Files:**
- Modify: `crates/egret-engine-mgmt/src/engines.rs` — the six route registrations at `:102-113` and the handlers `list_catalogs` (`:5640-5672`), `add_catalog` (`:5674-5824`), `get_catalog` (`:5826-5864`), `remove_catalog` (`:6020-6054`), `update_catalog` (`:6056-6176`), `test_catalog` (`:5917-5948`)
- Modify: `crates/egret-engine-mgmt/src/engines.rs` — delete `EngineCatalogDto` (`:331-369`), `legacy_catalog_dto` (`:381-402`), `legacy_catalog_dto_degrading` (`:404-436`), `find_attached` (`:438-446`), `EngineCatalogDetailDto` (`:448-460`), `REDACTED_VALUE` (`:462-465`), `is_sensitive_config_key` (`:467-470`), `redact_catalog_config` (`:472-497`), `restore_redacted_properties` (`:499-528`), `AddCatalogBody` (`:530-540`), `UpdateCatalogBody` (`:542-552`), `CatalogTestDto` (`:5866-5872`)
- Modify: `crates/egret-engine-mgmt/src/catalog_apply.rs` — delete `legacy_config_view` (`:727-780`), and `extract_plaintext_secrets` (`:783+`) **only if** `add_catalog` was its last caller
- Delete: `crates/egret-catalog-registry/src/legacy.rs` (664 lines) and its `mod` / `pub use` in `lib.rs` — `add_catalog` (`engines.rs:5761`) and `update_catalog` (`:6099`) are its last consumers, so it dies with them. Confirm with the compiler; if a consumer outside the six deleted routes appears, keep the module and report it.
- Modify: `crates/egret-engine-mgmt/tests/routes.rs` — `add_catalog_via_api` (`:5415-5433`) and its 13 call sites
- Modify: `frontend/src/api/controlPlane.ts:1022-1047` — `EngineCatalog`, `listEngineCatalogs` and the prose note
- Regenerate: `frontend/src/api/generated/**`

**Interfaces:**
- Consumes: Task 3 — `configError` already lives on the registry list, and no frontend code calls `listEngineCatalogs`.
- Produces: `/api/control-plane/engines/{id}/catalogs` no longer exists. `POST /engines/{id}/catalogs/{cid}` (attach) and `POST /engines/{id}/catalogs/{cid}/apply` (Retry) **remain** — they are registry routes under the same prefix.

- [ ] **Step 1: Confirm the frontend has no caller left**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
grep -rn "listEngineCatalogs\|EngineCatalogDto\|controlPlaneEnginesListCatalogs\|controlPlaneEnginesAddCatalog\|controlPlaneEnginesTestCatalog" frontend/src frontend/tests | grep -v "src/api/generated"
```

Expected after Task 3: only the wrapper definitions in `frontend/src/api/controlPlane.ts`. If a route or component still calls one, Task 3 is incomplete — go back rather than deleting a live caller's endpoint.

- [ ] **Step 2: Keep the two registry routes on the same prefix**

Read `crates/egret-engine-mgmt/src/engines.rs:102-113` and identify, by handler, which entries are `crate::catalogs::attach_catalog` and `crate::catalogs::retry_catalog_apply`. Those two stay. Deleting them breaks the Retry button and the attach picker SP3c-1 shipped.

- [ ] **Step 3: Delete the six handlers, their route entries and their utoipa paths**

Also remove the six `paths(…)` entries from wherever the crate registers them with utoipa.

- [ ] **Step 4: Build, and delete what the compiler reports as unused**

```bash
CARGO_TARGET_DIR=$PWD/target cargo build --workspace 2>&1 | tail -40
CARGO_TARGET_DIR=$PWD/target cargo clippy --workspace --all-targets 2>&1 | grep -E "never used|never read" | head -30
```

Delete each reported item from the Files list above. **Keep** `show_schemas_verdict` (`:5952-6018`) — the registry test route shares it — and `legacy_kind` (`:371-378`), still used by `catalogs.rs:329`.

- [ ] **Step 5: Fix `routes.rs`**

`add_catalog_via_api` (`:5415-5433`) posts to the deleted route. Its 13 call sites are setup, not subject — rewrite the helper to create a registry catalog and attach it (`POST /catalogs` then `POST /engines/{id}/catalogs/{cid}`), so the call sites keep working unchanged. Where a test's *subject* is a deleted route (it asserts on the legacy response shape), delete that test and say so in the commit.

`one_unparseable_catalog_does_not_take_an_engines_whole_list_with_it` (`:10062-10152`) — do not delete. Task 3 gave it a new home in `registry_catalog_config_error.rs`; confirm the new test asserts the same property and then remove the old one, naming the replacement in the commit.

- [ ] **Step 6: Frontend**

Remove `EngineCatalog`, `listEngineCatalogs` and the now-false prose note from `frontend/src/api/controlPlane.ts:1022-1047`, then:

```bash
cd frontend && npm run gen:api && npx tsc --noEmit 2>&1 | tail -20
```

Expected: the eight generated models named in the design (`engineCatalogDto.ts`, `engineCatalogDetailDto.ts`, `engineCatalogDetailDtoConfig.ts`, `addCatalogBody.ts`, `addCatalogBodyConfig.ts`, `updateCatalogBody.ts`, `updateCatalogBodyConfig.ts`, `catalogTestDto.ts`) disappear, and `tsc` is clean.

- [ ] **Step 7: Full backend test sweep for the crate**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-engine-mgmt -p egret-server 2>&1 | tail -40
```

Expected: pass, including `committed_openapi_json_is_current` after regenerating.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -F - <<'EOF'
feat: delete the legacy engine-catalog endpoints (SP3c-2)

Five of the six had no caller anywhere; the sixth was `GET`, whose only
remaining job — reporting `configError` — moved to the registry list in
the previous commit.

`CatalogTestDto`'s undiscriminated `{ ok, message }` dies with
`test_catalog`, which is what makes SP3c's D4 true everywhere rather than
on one route: a green tick that means "the port answers" can no longer be
read as "the credentials work".

`POST /engines/{id}/catalogs/{cid}` and `…/apply` live under the same URL
prefix and STAY — they are the registry's attach and Retry.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

---

### Task 5: Move the `engines.warehouse_id` readers onto the primary attachment

The column still exists at the end of this task. Nothing reads it.

**Files:**
- Modify: `crates/egret-engine-mgmt/src/catalog_apply.rs` or `catalogs.rs` — refuse `is_primary` on a catalog with no warehouse
- Modify: `crates/egret-engine-mgmt/src/engines.rs:626-635` (`build_provision_req_with`, StarRocks arm), `:4977-4983` (`update_starrocks_engine`), `:2899-2905`/`:3073`/`:3800` (`resolve_primary_catalog` consumers)
- Modify: `crates/egret-bin/src/main.rs:286` and `crates/egret-policy/src/admin/mod.rs:372-396` — `EngineBinding.warehouse_id` → `warehouse_ids`
- Create: a backend test that a warehouse-less primary is refused

**Interfaces:**
- Consumes: Task 4's tree.
- Produces: `EngineBinding { warehouse_ids: Vec<Uuid>, … }`. Attaching with `is_primary: true` to a catalog whose `warehouse_id` is `NULL` returns a 4xx naming the reason.

- [ ] **Step 1: Write the failing refusal test**

```rust
// A primary attachment must name a catalog that has a warehouse: StarRocks
// derives its builtin storage volume from it, and `ensure_engine_role` seeds
// the engine's Ranger role from it. A jdbc catalog has neither.
//
// Arrange: an engine, and a jdbc registry catalog (warehouse_id IS NULL).
// Act: attach it with is_primary = true.
// Assert: 4xx, and the body names "warehouse" — an operator must be able to
// tell this refusal from a generic validation failure.
```

Put it where the other attach tests live in `crates/egret-engine-mgmt/tests/`.

- [ ] **Step 2: Run it and watch it fail**

```bash
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-engine-mgmt primary 2>&1 | tail -20
```

Expected: FAIL — the attach currently succeeds.

- [ ] **Step 3: Implement the refusal**

Decide the engine-type question **before** touching the registry, the way SP3a's F-2 fix did: the refusal is evaluated from the catalog row's `warehouse_id` before any row is written, so a rejected attach leaves nothing to roll back.

- [ ] **Step 4: Rewrite the StarRocks storage derivation**

`engines.rs:626-635` currently calls `get_warehouse(engine.warehouse_id)`. Replace with a lookup through the primary attachment. The query pattern:

```sql
SELECT c.warehouse_id
  FROM registry.catalogs c
  JOIN registry.engine_catalog_attachments a ON a.catalog_id = c.id
 WHERE a.engine_id = $1 AND a.is_primary
```

Because of Step 3, `c.warehouse_id` is non-NULL whenever a primary row exists. An engine with **no** primary is the remaining case: `0150`'s partial unique index enforces *at most* one, not exactly one. Decide explicitly what a StarRocks engine with no primary does — refuse provisioning with a clear message is correct; silently defaulting is not. Do the same at `:4977-4983`.

- [ ] **Step 5: Check the Ranger/Client path needs no change**

`resolve_primary_catalog` (`engines.rs:2832-2839`) already returns the primary catalog's `warehouse_id`, and `ensure_engine_role` takes a warehouse id rather than reading the column. Read `:2899-2905`, `:3073` and `:3800` and confirm. If any of the three still reaches `engine.warehouse_id` directly, move it onto `resolve_primary_catalog`. Report what you found — "already correct" is a finding, not a skipped step.

- [ ] **Step 6: `EngineBinding.warehouse_id` → `warehouse_ids`**

`crates/egret-bin/src/main.rs:286` feeds `egret_policy::EngineBinding`, consumed by the Ranger policy-editor autocomplete at `crates/egret-policy/src/admin/mod.rs:372-396`. An engine now has one warehouse per attached egret catalog:

```rust
pub warehouse_ids: Vec<Uuid>,
```

populated from the engine's attachments (every attached catalog with a non-NULL `warehouse_id`, not only the primary). The consumer already dedupes into a `Vec<Uuid>`, so it changes little. **This widens the picker** — an engine attaching three egret catalogs autocompletes all three warehouses' schemas. That is intended (design §4 step 3) and must be stated in the commit message, because a behaviour change nobody wrote down is how this programme has already shipped surprises.

- [ ] **Step 7: Verify nothing production reads the column**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
grep -rn "warehouse_id" crates/ | grep -v "/tests/" | grep -E "engines?\.|Engine\b" | grep -v "registry\."
```

Read every hit. Expected: only `store.rs`'s struct field, its four column lists and the `create_engine` INSERT — which Task 6 removes. Any other hit is a reader Step 4 or 5 missed.

- [ ] **Step 8: Test**

```bash
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-engine-mgmt -p egret-policy -p egret-bin 2>&1 | tail -40
```

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -F - <<'EOF'
refactor: derive warehouse from the primary attachment, not the engine (SP3c-2)

StarRocks' builtin storage volume and the engine's Ranger role both came
from `engines.warehouse_id`. They now come from the primary attachment's
catalog, and a primary attachment is refused on a catalog that has no
warehouse — today's behaviour, made explicit, and the reason SP3c-2 does
not also let a non-warehouse catalog lead (design §4 step 3).

Behaviour change, deliberate: `EngineBinding` carries `warehouse_ids`
rather than one id, so the Ranger policy editor's autocomplete now offers
the schemas of EVERY egret catalog an engine attaches, not only its
primary's. An engine has had several warehouses since SP3a; the picker
was the last place still pretending otherwise.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

---

### Task 6: Drop `engines.warehouse_id`

Its own commit and its own live check, per the design's ordering note.

**Files:**
- Create: `migrations/0152_drop_engines_warehouse_id.sql`
- Modify: `crates/egret-engine-mgmt/src/store.rs:31` (`Engine.warehouse_id`), `:203-227` (the INSERT and `create_engine`'s parameter), and the SELECT column lists at `:231`, `:240`, `:594`
- Modify: `crates/egret-engine-mgmt/src/engines.rs:293`, `:306` — `EngineDto.warehouse_id`
- Modify: `frontend/src/routes/control-plane/useEngineConfigForm.ts:1291`, `useStarRocksConfigForm.ts:506`
- Modify: the 19 test files that `INSERT INTO engine_mgmt.engines (… warehouse_id …)`
- Regenerate: `frontend/src/api/generated/**`

**Interfaces:**
- Consumes: Task 5 — no production reader remains.
- Produces: `create_engine` loses an argument. `EngineDto` no longer carries `warehouseId`.

- [ ] **Step 1: The migration**

```sql
-- 0152_drop_engines_warehouse_id.sql
-- SP3c-2 §4 step 3. Primary is a property of the engine↔catalog relationship
-- (`registry.engine_catalog_attachments.is_primary`, 0150), not of the engine.
-- This column forced one warehouse per engine.
ALTER TABLE engine_mgmt.engines DROP COLUMN warehouse_id;
```

- [ ] **Step 2: Drop the field and the INSERT parameter**

Remove `warehouse_id` from `Engine`, from the three SELECT column lists and from `create_engine`'s INSERT and signature. Let the compiler find every caller of `create_engine` and update each.

- [ ] **Step 3: Drop it from the wire**

Remove `warehouse_id` from `EngineDto` (`engines.rs:293`, `:306`). Then delete the two frontend lines that echo `engine.warehouseId` into `UpdateEngineInput` — the server already ignores the field on `PUT`, so they are pure deletion, not a behaviour change.

- [ ] **Step 4: Fix the test seeds**

Nineteen test files insert the column. Remove it from each INSERT:

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
grep -rln "INSERT INTO engine_mgmt.engines" crates/
```

Work through the list the grep prints rather than a written list — nine of the nineteen were deleted in Task 1 and the count has moved.

- [ ] **Step 5: Regenerate and typecheck**

```bash
cd frontend && npm run gen:api && npx tsc --noEmit 2>&1 | tail -20
```

Expected: `model/engineDto.ts` loses `warehouseId`; `tsc` clean.

- [ ] **Step 6: Full backend sweep**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
env -u EGRET_ALLOW_DB_SKIP cargo test --workspace 2>&1 | tail -40
```

Expected: pass. Run one suite at a time if the workspace run is unstable — never two workspace suites concurrently.

- [ ] **Step 7: Live check on kind — both engine types**

This is the step the design singles out. Deploy this branch to kind, preserving the five live values:

```bash
command kubectl --context kind-egret get pods -n egret-system
```

Then, through the UI:
1. Provision a **Trino** engine with an egret-warehouse catalog as its primary. Wait for `HEALTHY`.
2. Provision a **StarRocks** engine the same way. Wait for `HEALTHY`.
3. For the StarRocks engine, confirm the builtin storage volume points at the primary attachment's warehouse — check the FE's `SHOW STORAGE VOLUMES` or the rendered spec, not the egret UI's own echo of what it intended.
4. Confirm the engine's Ranger role was seeded: the role exists and names the primary catalog's warehouse.

Report what you observed, verbatim, including anything that did not work. A measurement from egret's own UI of a value egret just sent is not a measurement.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -F - <<'EOF'
feat: drop engines.warehouse_id (SP3c-2)

The last NOT NULL anchor for "an engine has a primary" goes with it;
0150's partial unique index says at most one, so `resolve_primary_catalog`
is now the sole enforcement point.

`EngineDto.warehouseId` leaves the wire, which deletes two frontend lines
that echoed it back into `UpdateEngineInput` — the server has ignored the
field on PUT since SP3c-1.

Verified live on kind: a Trino engine and a StarRocks engine each
provisioned after the drop, each HEALTHY, with the StarRocks storage
volume and the engine's Ranger role both derived from the primary
attachment's catalog.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

---

### Task 7: Retire `warehouse:*`

**Files:**
- Create: `migrations/0153_retire_warehouse_permissions.sql`
- Create: a migration test for the grant-then-delete
- Modify: `crates/egret-catalog-engine/src/warehouse_admin/mod.rs` — 27 `require()` strings and the module doc at `:1-19`
- Modify: `crates/egret-bi/src/catalog_tags.rs:28,30` — the two constants
- Modify: `crates/egret-core/src/rbac.rs:82-104` — delete four `perm!` entries
- Modify: `frontend/src/auth/appAccess.ts:16-21`, `App.tsx:371`, `:431`, `:432`, `components/layout/Sidebar.tsx:112`, `catalogs/Catalogs.tsx:266`, `catalogs/CatalogDetail.tsx:138-141`, `:200-212`
- Modify: `frontend/src/routes/control-plane/warehouse-tabs/RolesTab.tsx:126`, `ClientsTab.tsx:118`, `BrowseTab.tsx:167`, `SettingsTab.tsx:312`

**Interfaces:**
- Consumes: Task 6's tree.
- Produces: `warehouse:*` is not a known permission. Every `warehouse_admin` and resource-tag route requires `catalog_registry:*`, at the same URL and under the same OpenAPI tag.

- [ ] **Step 1: The migration**

```sql
-- 0153_retire_warehouse_permissions.sql
-- SP3c-2 §4 step 4. 0148 granted catalog_registry:* to every platform role
-- holding warehouse:* — once, in the past. Every role created since holds
-- warehouse:* alone, so the grant is RE-RUN here, in the same transaction as
-- the delete and before it. Nobody gains access, nobody loses it.
INSERT INTO role_permissions (role_id, permission)
SELECT rp.role_id, 'catalog_registry:' || split_part(rp.permission, ':', 2)
  FROM role_permissions rp
  JOIN roles r ON r.id = rp.role_id
 WHERE r.scope = 'platform'
   AND rp.permission IN ('warehouse:read', 'warehouse:create', 'warehouse:update', 'warehouse:delete')
ON CONFLICT (role_id, permission) DO NOTHING;

DELETE FROM role_permissions
 WHERE permission IN ('warehouse:read', 'warehouse:create', 'warehouse:update', 'warehouse:delete');
```

Note the grant's `r.scope = 'platform'` filter, copied from 0148: a workspace-scope role holding the string would gain nothing usable from a platform key, and 0148's comment explains why the scope-blind string still matters.

- [ ] **Step 2: Write the failing migration test**

The population that matters is a role created **after** 0148 — seed exactly that:

```rust
// Arrange: a platform role holding only `warehouse:read`, created after 0148
// (i.e. inserted by the test, not by a fixture that predates it).
// Act: run 0153.
// Assert: the role holds `catalog_registry:read`, and holds no permission
// beginning `warehouse:`.
```

Assert **both halves**. A test that only checks the new key passes even if the delete never ran.

- [ ] **Step 3: Run it, watch it fail, then implement**

```bash
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-core migration_0153 2>&1 | tail -20
```

Place the test in whichever crate already hosts migration tests; `egret-core` is a guess — put it where `rbac` and role fixtures already live.

- [ ] **Step 4: Re-gate the 30 `require()` sites**

`catalog-engine/src/warehouse_admin/mod.rs`: `warehouse:read` → `catalog_registry:read` (13 sites), `create` (4), `update` (6), `delete` (4). `egret-bi/src/catalog_tags.rs`: the two constants at `:28,30`.

Update every OpenAPI `403` description that names `warehouse:*`, and the module doc at `mod.rs:1-19`, which states the router is `warehouse:*`-gated.

**Do not** change any path, any `tag = …`, or any handler name. A renamed generated function is ~40 frontend call sites and three e2e specs for nothing a user sees.

- [ ] **Step 5: Delete the permission catalog entries**

Remove the four `perm!` entries from `crates/egret-core/src/rbac.rs:82-104`. Check the "Warehouses" group label disappears with them rather than being left empty.

- [ ] **Step 6: Frontend permission strings**

- `auth/appAccess.ts:16` — remove `"warehouse:read"` from `CONTROL_PLANE_PERMS` and update the comment at `:17-21`, which says it leaves in SP3c-2.
- `App.tsx:371` (landing preference), `:431` and `:432` (route guards) — `catalog_registry:read`.
- `Sidebar.tsx:112` — `catalog_registry:read`.
- `Catalogs.tsx:266` — `canDeleteWarehouse` → `catalog_registry:delete`.
- `CatalogDetail.tsx:138-141` — `canReadWarehouse` is now redundant with the route guard; `warehouseTabs` reduces to `wh !== null`. Read `:200-212`'s 403 copy before deleting anything there.

- [ ] **Step 7: The four bare "forbidden" panes**

`Error::Forbidden`'s `Display` is the literal word `forbidden` (`egret-core/src/error.rs:49`). Four sites render it raw on a 403, and **all four are the tabs this task re-gates** — the persona who newly hits them is created by this very commit:

- `warehouse-tabs/RolesTab.tsx:126`
- `warehouse-tabs/ClientsTab.tsx:118`
- `warehouse-tabs/BrowseTab.tsx:167`
- `warehouse-tabs/SettingsTab.tsx:312` (`:133` is already handled — copy its treatment)

The pattern is `EnginesTab.tsx:461-465` and `CatalogDetail.tsx:205-212`: name the missing permission key and say what to ask for. Each pane must name `catalog_registry:read` (or the verb that route needs), not "forbidden".

- [ ] **Step 8: Write the permission test that matters**

A user holding only `catalog_registry:read` enters Control Plane, sees Catalogs, and can open a catalog's warehouse tabs. This is the lock-out D3 exists to prevent, and after this task `warehouse:read` no longer admits anyone.

Beware the SP3c-1 Task 11 defect: a 403 branch is unreachable when the query is gated by the same permission (`enabled: canRead` means `isError` is never true). If a test asserts a 403 pane, make sure the persona can actually reach the query.

- [ ] **Step 9: Test and commit**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
env -u EGRET_ALLOW_DB_SKIP cargo test --workspace 2>&1 | tail -40
cd frontend && npm run gen:api && npx vitest run 2>&1 | tail -30
```

```bash
git add -A
git commit -F - <<'EOF'
feat: retire warehouse:* in favour of catalog_registry:* (SP3c-2)

0153 re-runs 0148's grant before deleting the old keys, in one
transaction. 0148 ran once, in the past; every role created since held
`warehouse:*` with no counterpart, and that population is exactly who D3
exists to protect.

Re-gate without re-tagging: every path and every OpenAPI tag is
unchanged, so no generated function is renamed and the three e2e specs
that POST to `/api/control-plane/warehouses` keep working. Only the
`require()` string and the 403 copy move.

The four warehouse tabs that rendered `Error::Forbidden`'s Display — the
bare word "forbidden" — now name the permission and the remedy. This
commit is what creates the persona that sees them.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

---

### Task 8: Delete the Warehouses UI section

**Files:**
- Delete: `frontend/src/routes/control-plane/Warehouses.tsx`, `routes/control-plane/__tests__/Warehouses.test.tsx`
- Modify: `frontend/src/App.tsx` — the import (`:90`), the list route (`:431`), the landing preference (`:371`); **keep** `WarehouseRedirect` (`:282-358`) and its route (`:432`)
- Modify: `frontend/src/components/layout/Sidebar.tsx:104-113`
- Modify: `frontend/src/routes/control-plane/catalogs/CatalogCreate.tsx:28` — the import that breaks the build
- Modify: `frontend/src/routes/control-plane/WorkspaceDetail.tsx:377`, `StorageDetail.tsx:546`
- Modify: `frontend/src/__tests__/warehouseRedirect.test.tsx`, `components/layout/__tests__/Sidebar.test.tsx`, `routes/control-plane/__tests__/WorkspaceDetail.test.tsx`

**Interfaces:**
- Consumes: Task 7 — the section's permission no longer exists.
- Produces: `/control-plane/warehouses` (list) is gone; `/control-plane/warehouses/:id` still redirects.

- [ ] **Step 1: Verify a warehouse can still be created — in a browser**

`AddWarehouseDialog` (`Warehouses.tsx:258-401`) is the **only** create-warehouse UI in the tree. Before deleting it, drive `CatalogCreate` in a browser on kind and create an `iceberg` egret-warehouse catalog end to end, then confirm the warehouse exists (it appears on the catalog's Configuration tab and in `GET /control-plane/warehouses`).

If `CatalogCreate` cannot do this, **stop and report** — the section stays until it can, and that is a separate task, not an improvisation inside this one.

- [ ] **Step 2: Move the import that a URL grep cannot find**

`frontend/src/routes/control-plane/catalogs/CatalogCreate.tsx:28` imports `warehousesQueryKey` from `"../Warehouses"`, which merely re-exports it (`Warehouses.tsx:49`) from `"../queryKeys"`:

```diff
-import { warehousesQueryKey } from "../Warehouses";
+import { warehousesQueryKey } from "../queryKeys";
```

`warehousesQueryKey` itself **stays** in `queryKeys.ts` — `CatalogDetail.tsx:182` keys its single-warehouse fetch under it.

- [ ] **Step 3: Delete the page, the list route and the sidebar entry**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
git rm frontend/src/routes/control-plane/Warehouses.tsx frontend/src/routes/control-plane/__tests__/Warehouses.test.tsx
```

Then remove the import at `App.tsx:90`, the `<Route path="warehouses">` at `:431`, the `FIRST_SECTION_BY_PERM` entry at `:371`, and the sidebar item at `Sidebar.tsx:104-113` with its four comment lines.

`WarehouseRedirect` and its `warehouses/:id` route stay, re-gated in Task 7. Update the "Back to Warehouses" link inside its blocked pane (`App.tsx:339`) — it now points at a route that does not exist. Send it to Catalogs.

- [ ] **Step 4: Re-point the three inbound links**

- `WorkspaceDetail.tsx:377` — `/control-plane/warehouses?workspace=${workspaceId}` → `/control-plane/catalogs?workspace=${workspaceId}`. `Catalogs.tsx:270-279` already supports that filter server-side; confirm the parameter name matches.
- `WorkspaceDetail.tsx:409` and `StorageDetail.tsx:546` — `/control-plane/warehouses/${w.id}` resolve through the surviving redirect. Leave them, and add a one-line comment at each saying so, or they read like dangling links to the next reader.

- [ ] **Step 5: Collapse the dual-key invalidation**

`Warehouses.tsx:146-158` and `:277-287` both said the PR that retires the section owns this. With the page gone there is one writer of `listWarehouses` left (`EngineCreate.tsx:94`). Read `queryKeys.ts:1-26`, which explains the pair, and remove whichever half is now unreachable — then update that comment so it no longer names SP3c-2 as a future owner.

- [ ] **Step 6: Fix the tests**

`Sidebar.test.tsx:231-233`, `:249-253`, `:467`; `WorkspaceDetail.test.tsx:212`, `:218`; `warehouseRedirect.test.tsx` (keep — the redirect survives, but its describe block at `:140-173` about a post-0148 `warehouse:read`-only role must be rewritten for `catalog_registry:read`); `catalogsRouting.test.tsx:73-78`.

- [ ] **Step 7: Typecheck and Vitest**

```bash
cd frontend && npx tsc --noEmit && npx vitest run 2>&1 | tail -30
```

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -F - <<'EOF'
feat: delete the Warehouses UI section (SP3c-2)

Its five tabs moved to the catalog detail page in SP3c-1 and its create
dialog is superseded by CatalogCreate — verified in a browser before
deleting, because AddWarehouseDialog was the only create-warehouse UI in
the tree.

`WarehouseRedirect` stays, unlisted: it already answers the warehouse-id
→ catalog-id lookup that WorkspaceDetail and StorageDetail would each
otherwise have to grow, and it keeps a bookmarked URL working.

CatalogCreate imported `warehousesQueryKey` from `../Warehouses` rather
than from `../queryKeys` where it is defined — a compile break no grep
for the URL would have found.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

---

### Task 9: Comment sweep and the branch gate

**Files:**
- Modify: the ~20 non-doc comments naming SP3c-2, **scoped to files this PR touched**
- Modify: `docs/en/plans/2026-09-23-legacy-removal-sp3c-2.md` and the SP3c design, if implementation contradicted either

- [ ] **Step 1: Sweep the comments that are now false**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
grep -rn "SP3c-2\|SP3c removes\|until SP3c" crates/ frontend/src migrations/ | grep -v "^docs/"
```

Every hit in a file this PR touched must be read and either updated or deleted. A comment that says "this leaves in SP3c-2" inside code that just left in SP3c-2 is a comment made false by its own commit — this programme's most frequent review finding.

Do **not** sweep the 1662 "Task N" references (SP3c-1 task numbers) tree-wide; scope to files this PR touched.

- [ ] **Step 2: Full local CI**

GitHub CI is out of quota; everything runs locally.

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
export CARGO_TARGET_DIR=$PWD/target DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test EGRET_TEST_SHARED_STORE_URL=redis://localhost:6399
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -30
env -u EGRET_ALLOW_DB_SKIP cargo test --workspace 2>&1 | tail -40
cd frontend && npx tsc --noEmit && npx vitest run 2>&1 | tail -20
```

Two pre-existing `egret-workflow` clippy findings are known and not this PR's to fix; if they appear, say so rather than silencing them.

Then the e2e specs, against the deployed kind stack. Three touch `/control-plane/warehouses`:
`frontend/tests/e2e/helpers.ts:109`, `embedded-catalog.spec.ts` and `provisioning-ui-kind.spec.ts`. The
first two use **API** paths, which this PR does not move, so they should pass untouched — if either
fails, the re-gate changed something it was not supposed to. The third drives the provisioning UI and
is the one Task 8 can legitimately break.

```bash
cd frontend && npx playwright test tests/e2e/embedded-catalog.spec.ts tests/e2e/provisioning-ui-kind.spec.ts 2>&1 | tail -30
```

Log in once per spec — the login endpoint rate-limits at 5 attempts per 5 minutes.

- [ ] **Step 3: The live gate — both engine types, on the branch as merged**

Deploy the finished branch to kind, preserving the five live values (`oidc.enabled=true`, `ingress.enabled=true`, `kernel.launcher=kubernetes`, `backend.replicas=2`, `sharedStore.enabled=true`). Never the bare `just deploy-kind-oidc` recipe.

Then, in a browser:
1. Create an egret-warehouse catalog through `CatalogCreate`.
2. Provision a **Trino** engine with it as primary → `HEALTHY`.
3. Provision a **StarRocks** engine with it as primary → `HEALTHY`.
4. Open the catalog's five warehouse tabs (Roles, Clients, Browse, Connect, Settings) as an operator holding `catalog_registry:*` and confirm each loads.
5. Open one of them as a user holding **no** catalog permission and confirm the 403 pane names the permission rather than saying "forbidden".
6. Confirm a tag change still reaches the Trino Ranger plugin: change a resource tag and watch `download_tags` stop returning 304.

Report each step's observed result. Anything that failed is reported as failed, with the output.

- [ ] **Step 4: Open the PR**

One PR, per the user's decision. Body summarises the five removals, names the three migrations, states the required-upgrade-hop consequence prominently, and names the two deliberate behaviour changes (the widened Ranger autocomplete picker; a StarRocks engine with no primary now refuses to provision).

Then **stop**. Do not merge without the user's explicit approval.

---

## Deliberately not in this PR

- **`applyStatus` as a typed enum.** The design's carried list includes it. It is left out: the frontend's unknown-value fallback (`attachmentUi.tsx:83-90`, pinned by `EnginesTab.test.tsx:285-294`) is deliberate defence against a DB value the UI predates, and a generated TS union would delete it in a PR that already removes some 2 500 lines. Worth doing; worth doing separately.
- **Letting a non-warehouse catalog lead** (D2, withdrawn 2026-09-23). A feature, scheduled after SP4.
- The two pre-existing `egret-workflow` clippy findings.
- The tree-wide "Task N" comment sweep.
