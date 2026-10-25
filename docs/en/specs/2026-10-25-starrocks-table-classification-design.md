# StarRocks table-level classification compile (Classification UX, Package 2, D2): design

Status: design, 2026-10-25. Not implemented. Base: `origin/main` `596d8eea` (#744). Branch
`feat/starrocks-table-classification`.
Plan: [`../plans/2026-10-25-starrocks-table-classification.md`](../plans/2026-10-25-starrocks-table-classification.md).
Programme: Control Plane & DG, Classification UX. Parent: [`2026-10-24-verify-mask-per-user-design.md`](2026-10-24-verify-mask-per-user-design.md)
§3 D2 and §12 (this is the package that spec deferred).

The user settled four questions on 2026-10-25 (§3, U1–U4). The controller set the other defaults (§4, C1–C17); the
user may revisit those. Every UI string uses the parent glossary
(`2026-10-21-classification-ux-design.md` §4).

## 1. Problem

On StarRocks a classification attached to a **whole table** compiles nothing:

- a clearance on it compiles no deny and no NULL mask, so an uncleared reader reads every value, directly and
  through a view (E1, E2);
- a classification row filter on it filters no row (E5);
- the materialized-view deny ignores it (E3).

Trino enforces both, because its tag enricher applies a table-level tag to the table and its columns at query time.
Package 2 stated the gap in Explain, the rule dialog and three UI hints instead of fixing it (E17). This package
fixes the compiler and removes those statements.

Two facts make the fix more than dropping a `WHERE` clause:

- egret keeps no column list for tables (E10), and the NULL mask that protects a column through a view is per column
  (E4, E20);
- a table's columns change after the compile, and no egret code runs when they do (E11).

## 2. Evidence (verified at `596d8eea`)

Paths are relative to the repo root. `ST` = `crates/egret-policy/src/store.rs`, `CL` =
`crates/egret-perm-compile/src/clearance.rs`, `CM` = `.../clearance_mask.rs`, `TR` = `.../trigger.rs`, `RU` =
`.../rules.rs`, `RT` = `.../routes.rs`, `EF` = `.../effect.rs`, `EE` = `.../explain_effect.rs`, `VH` =
`crates/egret-catalog-engine/src/view_hold.rs`, `SRDEF` = `crates/egret-policy/src/service-defs/starrocks.json`.

| id | fact | where |
|---|---|---|
| E1 | The only StarRocks expansion, `tagged_columns_for_service_via`, selects `a.column_name <> ''`, so table-level rows (`''`, migration 0081) are dropped. Its doc says it "masks a COLUMN, never a whole table". Names are returned verbatim; assignments are stored verbatim | `ST:1932-1951`, filter `ST:1944`, doc `ST:1228-1232`; `crates/egret-bi/src/catalog_tags.rs:111-116` |
| E2 | StarRocks clearance: per tagged column one `select` deny `perm-clearance-{tag}-{c}.{d}.{t}.{col}` (`{public}` denied, cleared users in `denyExceptions`, priority 0, no override) and one `PendingMask`; the per-tag ceiling counts tagged columns. No dedup ("collapsing them would un-deny one") | `CL:349-405` (ceiling `:368-374`, name `:376-379`, mask `:398-402`); `CL:104-108`; shape `CL:493-545` |
| E3 | The engine-wide materialized-view deny is built only from the pending masks, so a table-only classification neither creates nor narrows it | `CL:408-415`, `CL:457-480` |
| E4 | The masks are merged per column (cleared = intersection, named after the lowest tag id) and folded through the evaluator at priority 1, before the overlap refusal. The merge key is the exact-case `(catalog, schema, table, column)` | `TR:602-614`, `TR:795-875`, merge `TR:828`; `CM:111-128`; name `CM:85-93` |
| E5 | A tag rule on StarRocks takes `tagged` from the same expansion; a row filter is deduped per table on the resource values and named `perm-rule-{id}-{c}.{d}.{t}`; row filters stop at 3 levels; the rule ceiling counts `tagged` | `TR:917-929`; `RU:372-437` (ceiling `:387-393`, dedup `:411-423`); `crates/egret-perm-compile/src/grants.rs:403`; `RU:471-487` |
| E6 | Ranger matches every StarRocks level case-insensitively: the access resources set `ignoreCase: true`; the data-mask and row-filter resources omit it, and Ranger's default is true | `SRDEF` resources; `crates/egret-ranger-eval/src/servicedef.rs:102`, `:119` |
| E7 | The access `column` resource accepts `*` (`wildCard`, `accessTypeRestrictions ["select"]`). egret already emits full-depth `*` for grants. It emits no wildcard **mask** by design | `SRDEF:100-122`; `crates/egret-perm-compile/src/grants.rs:260-298`, `:300-316` |
| E8 | Trap: a column value `""` survives `resources_object` (only an empty `values` list is dropped); Ranger drops the empty value and the level becomes match-any | `crates/egret-policy/src/admin/ranger_json.rs:83-91`; `crates/egret-policy/src/ambiguous_arbitration.rs:96-106`; `crates/egret-perm/src/actions.rs:390` |
| E9 | StarRocks consults no access policy on a column whose deciding mask is `MASK_NULL`; a query reading only such columns is checked on another column, on `___count___` for `count(*)`, and on the hidden `_file`/`_pos` when every column is masked (measured). A hand-written `column: *` deny does not reach the masked column itself | `RT:796-818`; test `crates/egret-perm-compile/tests/routes.rs:4374` |
| E10 | egret stores no table columns (legacy tables dropped). Table schemas live in embedded Lakekeeper's `lakekeeper` schema in the same database. `egret-catalog-engine` depends on `egret-perm-compile`, so the compiler cannot call `load_table_detail`. Lakekeeper's own "active" predicate is `deleted_at IS NULL AND metadata_location IS NOT NULL`, with `typ` `'table'`/`'view'` | `migrations/0026_drop_legacy_catalog.sql`; `crates/egret-catalog-engine/src/pool.rs:1-26`; `crates/egret-catalog-engine/src/browse.rs:271-317`; `crates/egret-catalog-engine/Cargo.toml:123`; `crates/egret-catalog-engine/.sqlx-lakekeeper/query-fb866c48…json`, `query-32335b00…json` (`table_current_schema`, `table_schema`) |
| E11 | The Iceberg guard's hold sees view requests only: `classify` returns `None` for every table path. It is an axum layer inside the guard, before Lakekeeper. Its after-commit steps (drop, rename source) log a failure and pass the response | `VH:335-371`, `VH:2118-2180`; `crates/egret-catalog-engine/src/mount.rs:278-300`; `VH:1913-1941` |
| E12 | No code moves or deletes table-level assignments on a table rename or drop; the only deletes are the view hold's, `catalog_tags.rs:221` and `view_tags.rs:322`, and there is no `UPDATE catalog_tag_assignments` | `/usr/bin/grep` over `crates/**/src` |
| E13 | Boot passes: `recompile_role_referencing_profiles` (one transaction; every compiler-targeted profile when any gate names a role, so one refusal rolls the whole pass back), `sync_admin_policies` (one transaction per service, failures logged), `recompile_queued_renames` (a queue table from a migration) | `crates/egret-bin/src/main.rs:2370-2420`; `crates/egret-perm-compile/src/role_membership.rs:251-290`, `:357-385`; `TR:1780-1800`, `TR:2231-2260`; `crates/egret-bi/src/tags.rs:467` |
| E14 | Ceilings: `DEFAULT_EXPANSION_CEILING = 500` per rule or clearance, always refused above; `PROFILE_EXPANSION_CEILING = 1000`, refused only when the total grows above it | `TR:148`, `TR:195`, `TR:1166-1177`; `crates/egret-perm/src/actions.rs:136-194` |
| E15 | The overlap refusal rejects only pairs this compile creates; `GET /perm/overlaps` lists pairs already stored. A refused pair is never stored, so every later compile of that profile creates it again | `TR:616-670`; `RT:1939-1952` |
| E16 | A recompile's skips reach the caller as `CompileResponse.skipped` (`SkipReport{kind: "clearance", id: tag_id, reason}`) and the log; a failed self-contained recompile is recorded in `perm.compile_runs` | `RT:1187-1216`; `TR:1533-1570`; `crates/egret-perm-compile/src/compile_runs.rs:71` |
| E17 | Package 2 states the gap in: `RuleEffect.starrocks_table_only`/`StarRocksTableOnly` and `TABLE_ONLY_TABLES`; `ExplainProfile.clearance_not_enforced`; the `gated_clearances` split; the StarRocks arm of `row_filter_not_applied` and `ROW_FILTER_GAPS_TABLE_ONLY`; two explain sentences; `StarRocksTableOnlyNotice`; half of `TRINO_ROW_FILTER_HINT`; `TABLE_LEVEL_NOTE`; `COMPILES_TO`; two backend tests | `EF:104-122`, `EF:312-322`, `EF:361-371`; `RT:779-790`, `RT:898-915`, `RT:2580-2594`, `RT:2884-2896`, `RT:3682-3724`, `RT:3812-3828`; `EE:809-830`, `EE:934-941`; `frontend/src/routes/governance/RulePreview.tsx:170`, `:232`; `RuleDialog.tsx:56-58`, `:83-84`, `:604-631`, `:893-900`; `data/AddTagDialog.tsx:42-43`; `classifications/tabs/ClearanceTab.tsx:28-29`; `crates/egret-perm-compile/tests/routes.rs:4812`, `:9831` |
| E18 | Explain: `cleared_for` holds only column-attached gated tags (`gated_clearances` partitions on `attached_at_column`); under `masked_read_skips_access` the table `select` verdict is `other_columns_access`, and a denial reads "refused … StarRocks masks it to NULL, and the query still needs SELECT on the other columns it reads" | `RT:3682-3724`, `RT:2582-2587`, `RT:2910`; `RT:2804`; `EE:590-609` |
| E19 | Attribution takes any locator after `perm-clearance-{uuid}-`, so a `…{c}.{d}.{t}.*` name attributes to the clearance | `crates/egret-perm-compile/src/attribution.rs:100-109` |
| E20 | A view over a table-level-classified base already gets per-column derived tags. The base NULL mask is what holds through a StarRocks view (A2); the base deny does not. The StarRocks `view` resource carries no `select` | `crates/egret-perm-compile/src/lineage.rs:222-259`; `CM:1-13`; `CL:130-142` |
| E21 | No release tag exists (`git tag` is empty). The gap wire fields shipped in #744 (merged at `596d8eea`). Their only consumers are the generated client, `RulePreview`/`RuleDialog` and tests | `git tag`; `/usr/bin/grep` over `frontend/src`, `crates` |
| E22 | Highest migration on main: `0209`. Open PR #742 adds none (2026-10-25) | `migrations/`; `gh pr view 742 --json files` |
| E23 | A tag MASK rule on a table-level classification is refused on every write path, engine-agnostic | `crates/egret-policy/src/unreadable_table_mask.rs:1-37`, `:443-472` |

## 3. User decisions (2026-10-25, not reopened)

- **U1. Deny shape.** One access deny with `column: *` per (classification, table) for a classification attached at
  table level on StarRocks: deny `{public}`, `denyExceptions` = cleared users, `select`. No column list. It covers
  columns added later, hidden columns and `count(*)`, as Trino does. An uncleared reader who holds a grant is refused
  the whole table.
- **U2. Schema changes.** A table-commit hook (a layer like the view hold) recompiles the affected StarRocks profiles
  at once when a table carrying a table-level gated classification changes schema (add, drop or rename a column;
  create, rename or drop the table).
- **U3. Ceilings.** Keep `PROFILE_EXPANSION_CEILING` 1000 and `DEFAULT_EXPANSION_CEILING` 500. Over the ceiling the tag
  write is refused with a message naming the table(s) and column counts. The `*` deny counts 1 per table.
- **U4. Unknown table or no schema.** The `*` deny still applies; only the per-column masks are skipped, and egret says
  so in the compile result and in Explain. The hook adds the masks when the table appears.
- **U5. A schema change whose recompile would be refused is refused (confirms C11).** The hook dry-runs the
  recompile before the commit; if it is refused (ceiling, undecidable fold, new tie) the Iceberg request fails with a
  409 that names the reason. A profile is never wedged by a schema change.
- **U6. Rollout refusals show in Governance.** A refusal on the first recompile after upgrade (C14) keeps the
  profile's previous compile and must surface as the existing Governance Overview `compile_failed` finding
  (`crates/egret-controlplane/src/governance_overview.rs:256`, fed by `compile_runs::failed_compile_runs_via`), whose
  sentence names the two rules in conflict and links them. The boot pass records each refused profile as a failed
  compile run. Release note as in C14.

## 4. Controller decisions (defaults; the user may revisit)

| # | Decision | Why |
|---|---|---|
| C1 | **Per-column NULL masks for table-level attachments too**, one per column of the table's current schema, through the existing `merge_by_column` and fold at priority 1. No wildcard mask: `grants.rs` keeps its refusal | The deny withholds only the direct read; the base mask is what holds through a view (E20). A wildcard clearance mask cannot carry per-column fold results and would tie with per-column masks in the same band (findings §2) |
| C2 | **Column source: SQL on `lakekeeper.*` over the compile's own connection**, in one module `crates/egret-perm-compile/src/table_columns.rs`. Rejected: a port trait implemented by `egret-catalog-engine` | Same snapshot and transaction as the rest of the compile; works from every recompile call site (egret-bi, catalog-engine, controlplane, perm routes) with no wiring and no global registration (no precedent in the crate); no second pool or `ApiContext`. The cost, coupling to Lakekeeper's tables, is confined to one query and guarded by C3 |
| C3 | **Drift guard:** a DB-gated test in `egret-catalog-engine` creates a table through Lakekeeper's real REST router, applies `add-schema`/`set-current-schema` commits (add, rename, drop a column), renames and drops the table, and after each step asserts that `table_columns` returns exactly the top-level names `browse::load_table_detail` returns (and `None` after the drop). perm-compile's own tests use a minimal stub `lakekeeper` schema; the guard is what keeps the stub honest | Lakekeeper is git-pinned (`Cargo.toml:32-35`); its schema changes only on a rev bump, and this test fails on that bump. A runtime query error fails the compile (refuse, never guess) rather than skipping masks |
| C4 | **"Current schema"** = `table_current_schema.schema_id` → `table_schema.schema` (Iceberg schema JSON) → the `name` of each top-level `fields[]` entry, in field-id order. Nested struct/list/map sub-fields are ignored (the mask covers the whole top-level column). Only an active table counts (`typ = 'table'`, `deleted_at IS NULL`, `metadata_location IS NOT NULL`); a view or a staged create is "no schema" (U4). The namespace must be single-level (the Ranger resource has one schema level, as the view hold rules) | StarRocks masks and column privileges address top-level columns; Lakekeeper's own active predicate (E10) |
| C5 | **Case folding.** One fold, `to_lowercase()` (the evaluator's own `fold`, exported as `egret_ranger_eval::resource::fold_identifier`), on schema, table and column, in the compile expansion and in `merge_by_column`. Emitted spelling: the catalog's actual name for every table the column source was asked about (tables reached by a table-level gated attachment); elsewhere the first spelling in the expansion's order (today's spelling for a table with one spelling) | Ranger matches case-insensitively (E6), so two spellings are one resource: two priority-1 masks would be a refused differing tie. The rule leaves today's bodies for column-only tables byte-identical (no churn on upgrade) |
| C6 | **Denies.** A table-level gated attachment emits one `*` deny `perm-clearance-{tag}-{c}.{d}.{t}.*` (U1). A column-level deny of the same tag on the same (folded) table is dropped: the `*` deny covers it with the same exceptions | Fewer policies; the name is a function of (tag, resource) and the body of (tag, resource, cleared), so equal names imply equal bodies |
| C7 | **Materialized-view deny** contributors are every gated (tag, cleared) pair that reaches the profile at either level, including an unknown table | A table-only classification must narrow the engine-wide deny like a column one (E3) |
| C8 | **Row filter.** A row-filter-only expansion returns table coordinates for both levels, deduped on the folded `(catalog, schema, table)`. Same name and shape as today. The rule ceiling counts tables for a row filter. No column list, so no U4 case | E5: the per-table shape was always intended; only the shared column query dropped table-level rows |
| C9 | **Ceiling arithmetic (U3).** Per clearance: (column-level denies not covered by a `*`) + Σ over table-level tables of (1 + its column count). Column-level masks stay uncounted per tag, as today. The profile total counts emitted policies, as today. Both refusals name up to five largest table-level tables with their column counts | Counting column-level masks now would newly refuse stored tags with 251–500 tagged columns |
| C10 | **Hook: after-commit recompile, synchronous, never undoing the commit.** After Lakekeeper answers 2xx, the hook recompiles the affected StarRocks profiles in one transaction per profile before returning the response, in a detached task bounded by `lock_timeout`. A failure is logged, recorded in `perm.compile_runs` and retried by the boot pass (C13); the 2xx passes | The Iceberg commit has happened and cannot be refused afterwards (the view hold's after-commit steps make the same choice, E11). Until the recompile lands, the `*` deny already refuses uncleared direct reads of a new column, so a failure costs view protection of the new column only |
| C11 | **Hook: pre-commit dry run for a schema-growing request.** For a create, register, rename, or update/transaction carrying `add-schema` or `set-current-schema` on a table with a table-level gated attachment, the hook first runs the real recompile of the affected profiles in a rolled-back transaction with the request's new schema overlaid (`SET LOCAL egret.pending_table_columns`), and refuses the request with a 409 Iceberg error naming the compile's refusal when it fails | A per-tag ceiling refusal is absolute (E14): a column add that crosses it would otherwise wedge every later compile of the profile. The programme refuses at the write that causes a refusal; the view hold refuses before commit for the same reason |
| C12 | **Affected profiles** = StarRocks profiles bound to the warehouse (`services_bound_to_warehouse`) when the table (old or new name, folded) carries a table-level attachment of a tag with a role-bearing gate. Row filters need no hook: they do not depend on columns. Data commits (no schema update) are not looked up at all | One indexed query per schema-changing commit; zero for appends |
| C13 | **Boot pass** `recompile_table_classified_profiles`: every boot, one transaction per StarRocks profile that reaches a table-level attachment of a gated tag or of a tag a row-filter rule selects. Failures are logged and recorded; idempotent (an unchanged body writes nothing). No queue table | The first recompile after upgrade must not depend on an unrelated write; per-profile transactions keep one refusal from blocking the rest (unlike the role pass, E13); it also repairs any hook failure |
| C14 | **Rollout ties.** A newly reached table can create a differing tie between two classification row filters (or with a hand-written priority-1 mask). Only pairs this compile creates are refused (E15), so that profile keeps its previous compile, the refusal is recorded and logged, and every later write that recompiles the profile is refused until a steward edits or deletes one of the two rules (that edit's own compile removes the tie and is accepted). Release note: check each StarRocks profile's last compile after upgrade; `GET /perm/overlaps` lists only stored pairs | Refusing silently-created ties is the programme's rule; the note says where the new ones show |
| C15 | **Remove** the Package 2 gap statements for StarRocks table-level (E17); keep Trino's D3 gap. Wire fields are **removed**, not deprecated | No release tag exists and the only consumers are in this repo (E21) |
| C16 | **No migration.** The hook keeps no queue (C10, C13) | Migration numbers are shared; none is reserved |
| C17 | Platform docs (`egret-data/docs`) in a separate docs PR after merge, as Package 2's C13 | Same flow the user chose on 2026-10-23 |

## 5. Design

### 5.1 The expansion

Replace the single column query with two reads in `egret-policy` (both keep `registry.catalogs` ⋈
`engine_catalog_attachments` ⋈ `engines` on `authz_service_id`):

- `tag_assignments_for_service_via(conn, service_id, tag_key) -> Vec<TagAssignmentRow {catalog, warehouse_id,
  schema_name, table_name, column_name}>`: both levels, verbatim, ordered. `column_name = ''` means table level.
  `tagged_columns_for_service_via` is deleted (its two callers move).
- The compiler groups the rows in Rust under the folded key (C5) into `TableTarget { catalog, warehouse_id, schema,
  table, table_level: bool, columns: Vec<String> }`.

An empty column name is **never** turned into a `TaggedColumn` (E8). `TaggedColumn` gains a constructor that refuses
an empty level, and a compile-time test walks every policy in the desired set of a table-level fixture and asserts no
`resources.*.values` contains `""`.

### 5.2 Column source (C2–C4)

`table_columns::current_columns_via(conn, warehouse_id: Uuid, schema: &str, table: &str) ->
Result<Option<TableColumns { schema: String, table: String, columns: Vec<String> }>, CompileError>`:

```sql
SELECT ti.tabular_namespace_name, ti.name, ts.schema
  FROM catalog.warehouses w
  JOIN lakekeeper.tabular ti ON ti.warehouse_id = w.lakekeeper_warehouse_id::uuid
  JOIN lakekeeper.table_current_schema tcs
    ON tcs.warehouse_id = ti.warehouse_id AND tcs.table_id = ti.tabular_id
  JOIN lakekeeper.table_schema ts
    ON ts.warehouse_id = tcs.warehouse_id AND ts.table_id = tcs.table_id AND ts.schema_id = tcs.schema_id
 WHERE w.id = $1 AND ti.typ = 'table' AND ti.deleted_at IS NULL AND ti.metadata_location IS NOT NULL
   AND cardinality(ti.tabular_namespace_name) = 1
   AND lower(ti.tabular_namespace_name[1]) = lower($2) AND lower(ti.name) = lower($3)
```

The exact column names and casts above are a sketch read from Lakekeeper's query cache (E10). The plan's first column
source task pins them with the C3 drift test before anything uses them.

- The returned `schema`/`table`/`columns` are the catalog's spelling (C5).
- `columns` = top-level `fields[].name`, field-id order. Two fields that fold equal (Iceberg allows `Email` and
  `email`) collapse to one entry, the lower field id: Ranger cannot tell them apart (E6).
- One batched variant takes every table of a compile at once (`unnest`), so a profile costs one query.
- **Overlay** (C11): when the transaction-local setting `egret.pending_table_columns` is present
  (`current_setting(…, true)`), its JSON (`[{warehouse_id, schema, table, columns | null}]`) replaces the SQL answer
  for those tables. Only the hook's dry run sets it, with `SET LOCAL`, so it dies with that transaction.

### 5.3 Clearance on StarRocks (U1, C1, C6, C7, C9)

For each gate with required roles (unchanged skips: informational, groups-only), per `TableTarget`:

1. **Table level:** one `*` deny named `perm-clearance-{tag}-{c}.{d}.{t}.*`, resources `{catalog, database, table,
   column: ["*"]}` (spelling per C5), the same `clearance_input` shape (E2), `select` only. Then the column source:
   - `Some(cols)`: one `PendingMask` per column (catalog spelling);
   - `None` (U4): no mask; a `SkippedItem { source_kind: Clearance, id: tag_id, reason }` per (tag, table), reason
     "`c.s.t` is not a table in the catalog (unknown, a view, or not yet created): its whole-table select deny is
     compiled, its NULL masks are not, so a view over it is not protected until the table exists".
2. **Column level** on a table without a table-level attachment of the same tag: today's per-column deny and mask.
   With one: the masks still go to the merge; the per-column denies are dropped (C6).
3. **Ceiling** (C9) before any policy is pushed; the refusal is `CompileError::ClearanceExpansionCeiling { tag, count,
   ceiling, tables: Vec<TableCount { table, columns }> }`, message e.g. "compiling the clearance on tag PII would expand
   to 734 policies on StarRocks, over the ceiling 500: whole-table on wh.sales.events (1 deny + 512 column masks),
   wh.sales.orders (1 + 180), and 40 per-column denies. Attach PII to fewer tables, or to columns".
4. **MV deny** (C7): built from `Vec<(tag_id, cleared)>` contributors, not from the pending masks.

`merge_by_column` keys on the folded tuple; when two spellings meet, the catalog spelling wins (C5), then the first
in order. Everything after it (fold at priority 1, name after the lowest tag id, overlap refusal) is unchanged.

**What the engine does** (to be confirmed live, plan Task 16 step 2):

- uncleared, granted reader: every known column decides `MASK_NULL`, so StarRocks checks no known column (E9) and
  falls through to `_file`/`_pos` (or `___count___`), which the `*` deny matches: **refused**. A column the compile
  has not seen yet (added since) has no mask, is access-checked, and the `*` deny refuses it;
- cleared reader: excepted from the deny; each column shows the folded rule outcome;
- through a view: the base masks hold, so an uncleared reader reads NULL in every column of the classified base.

### 5.4 Row filter (C8)

`compile_one_rule` (StarRocks, tag selector) asks for `TableTarget`s and, for a row filter, hands `compile_rule` one
`TaggedColumn`-free table list: `compile_rule` gets a `RowFilterTables(&[TableRef])` input for that branch instead of
`tagged`. The name stays `perm-rule-{id}-{c}.{d}.{t}`, the body stays byte-identical, the dedup key is the folded
table. A mask rule keeps the column list (column-level only; table-level mask rules stay refused at write, E23).

### 5.5 Table-commit hook (U2, C10–C12)

A new middleware `crates/egret-catalog-engine/src/table_hook.rs`, installed by `front_catalog_router` beside the view
hold (inside the guard, before Lakekeeper). It handles:

| request | path | counts when |
|---|---|---|
| create | `POST /{prefix}/namespaces/{ns}/tables` | always (a staged create is judged at its committing update) |
| register | `POST /{prefix}/namespaces/{ns}/register` | always |
| update | `POST /{prefix}/namespaces/{ns}/tables/{table}` | `updates` contain `add-schema` or `set-current-schema` |
| multi-table commit | `POST /{prefix}/transactions/commit` | any table change as for update |
| rename | `POST /{prefix}/tables/rename` | always |
| drop | `DELETE /{prefix}/namespaces/{ns}/tables/{table}` | always |
| drop namespace | `DELETE /{prefix}/namespaces/{ns}` | always (every table under it) |

Order per request:

1. Classify the path; any other request passes untouched. Read the body (bounded like the view hold) only for the
   POST kinds. An update without a schema change passes with no DB query (C12).
2. Resolve the prefix to the egret warehouse (`VH:690-701`'s query, shared). Multi-level namespaces pass.
3. Look up affected profiles (C12). None: pass.
4. Schema-growing kinds (C11): the new column list comes from the request (the create/register schema, the
   `add-schema` schema with `set-current-schema -1`, or a stored schema id read through the column source; rename:
   the source's columns under the destination name). Dry-run recompile in a rolled-back transaction with the overlay;
   `lock_timeout` as the view hold. Compile refusal → 409 `"egret refused the table change: <compile message>"`;
   lock timeout → 409 busy, retry.
5. Forward to Lakekeeper. Non-2xx: return it.
6. 2xx: recompile the affected profiles, one transaction each (C10), in a detached task the handler awaits up to the
   bound; failure → `warn!`, `record_compile_failure`, response unchanged.

The hook does **not** move table-level assignments on a rename or drop (E12): that is a defect of its own on both
engines (§10). After a rename the recompile reports the old name as unknown (U4) and the new name carries no
classification.

### 5.6 Boot pass (C13)

`egret_perm_compile::recompile_table_classified_profiles(pool) -> Result<usize, CompileError>`, spawned in
`egret-bin` beside the other boot passes, best-effort. Profiles: StarRocks, bound, reaching a table-level assignment
whose tag has a role-bearing gate or is selected by a row-filter rule. One transaction per profile with
`lock_services_in_tx`; a failure is logged and recorded in `perm.compile_runs`.

### 5.7 Explain

- `gated_clearances` returns every gated resolved tag (no partition); `cleared_for` therefore names table-level
  classifications too ("cleared for *PII* and *Finance*").
- Removed: `ExplainProfile.clearance_not_enforced`, the D2 sentence and its call sites, the StarRocks arm of
  `row_filter_not_applied`, `ROW_FILTER_GAPS_TABLE_ONLY`, the StarRocks tuple of `append_row_filter_gaps`. The
  `ExplainRowFilterGap` doc keeps only Trino. `ExplainClearanceGap` leaves the wire; the builder's `cleared_for`
  takes an internal `ClearanceRef { tag_id, tag_name }`.
- New wording, uncleared reader, StarRocks, table-level clearance: when `other_columns_access` (or the direct access
  verdict) is denied by a clearance policy whose locator ends in `.*`, the line reads "`u` is **refused** `email`
  (clearance *PII*, on the whole table)". The "query still needs SELECT on the other columns" tail is kept only for
  other deniers.
- New `ExplainProfile.clearance_masks_missing: Vec<ExplainClearanceMaskGap { tag_id, tag_name, reason }>` (StarRocks,
  column read, skipped when empty): a gated tag that resolves on the target (either level) while the served artifact
  has no clearance mask matching the column. `reason`: `table_not_in_catalog` when the column source has no table,
  else `not_compiled` (content-derived, not from timestamps). Sentence appended: ". No NULL mask for clearance *PII*
  is served here: egret found no table `sales.orders` in the catalog, so a view over it shows the values" /
  "…: the profile has not been recompiled since this column appeared".
- The differential pins that a table request (no column) and a column request both match a `column: *` deny on the
  2.8.0 and 2.9.0 jars (plan Task 14). Explain's `other_columns_access` relies on the table request.

### 5.8 Rule effect and UI copy

- `RuleEffect.starrocks_table_only`, `StarRocksTableOnly`, `TABLE_ONLY_TABLES`: deleted. `trinoColumnOnly` stays.
- `RulePreview`: `StarRocksTableOnlyNotice`, `starrocksTickReason` deleted. `RuleDialog`: their wiring deleted; the
  tick depends on `secondMask || trinoColumnOnly`.
- `TRINO_ROW_FILTER_HINT` → "On Trino, a row filter by classification applies only where the classification is on the
  whole table."
- `TABLE_LEVEL_NOTE` → "On a whole table, this classification counts for clearance on every column. A rule that masks
  by classification needs it on columns, not on the table."
- `COMPILES_TO` → "Compiles to one deny tag policy in each Trino profile's tag service. On StarRocks it compiles to a
  select deny and a NULL mask per tagged column, or, on a whole table, one select deny on the table and a NULL mask per
  column. StarRocks does not access-check a masked column, so an uncleared reader reads NULL in a classified column
  rather than being refused, unless the classification is on the whole table."

## 6. Principles

- A whole-table classification means the same on both engines: the uncleared are refused, a view hides the values.
- The deny never depends on knowing the columns; only the view protection does, and its absence is said.
- Refuse at the write that causes a refusal (tag write, rule write, schema-growing table change); never undo an
  Iceberg commit after the fact.
- One fold, one spelling rule, one expansion for both levels.

## 7. Backend changes (enumerated)

1. `egret-policy/src/store.rs`: `tag_assignments_for_service_via` (both levels); delete
   `tagged_columns_for_service_via` and its `PolicyStore` wrapper and doc; a `TagAssignmentRow` type.
2. `egret-ranger-eval/src/resource.rs`: export `fold_identifier`.
3. `egret-perm-compile`:
   - `table_columns.rs` (new): column source, batched variant, overlay;
   - `rules.rs`: `TableTarget`, `TaggedColumn::new` refusing empty levels, row-filter tables input;
   - `clearance.rs`: table-level `*` deny, masks from the column source, U4 skip, C6 dedup, C9 ceiling, C7 MV
     contributors;
   - `clearance_mask.rs`: folded merge key and spelling rule;
   - `trigger.rs`: callers of the expansion; `recompile_table_classified_profiles`; affected-profile query for the hook
     (`affected_starrocks_profiles_for_table`);
   - `routes.rs`, `explain_effect.rs`, `effect.rs`: §5.7, §5.8;
   - `harness_support.rs`: the table clearance deny for the differential.
4. `egret-perm/src/actions.rs`: `ClearanceExpansionCeiling.tables`, `ProfileExpansionCeiling.largest`.
5. `egret-catalog-engine`: `table_hook.rs` (new), `mount.rs` wiring; the prefix resolution shared with `view_hold.rs`.
6. `egret-bin/src/main.rs`: the boot pass; the hook's construction beside the view hold.
7. `openapi.json`, `frontend/src/api/generated/**` regenerated (`npm run gen:api`).

## 8. Wire shapes

Removed (C15): `RuleEffect.starrocksTableOnly`, schema `StarRocksTableOnly`; `ExplainProfile.clearance_not_enforced`,
schema `ExplainClearanceGap`.

Added:

```json
{
  "service_id": 7, "service_name": "sr-main", "engine": "starrocks",
  "clearance_masks_missing": [
    {"tag_id": "…", "tag_name": "PII", "reason": "table_not_in_catalog"}
  ],
  "effect_lines": [
    {"about": "column", "outcome": "refused",
     "text": "nobody-u is refused email (clearance PII, on the whole table). No NULL mask for clearance PII is served here: egret found no table sales.orders in the catalog, so a view over it shows the values",
     "parts": ["…"]}
  ]
}
```

`clearance_masks_missing` uses `skip_serializing_if = "Vec::is_empty"`. Compile results carry the U4 skip as an
existing `SkipReport` (`kind: "clearance"`, `id`: the tag id). Ceiling refusals keep their HTTP status; only the
message grows.

## 9. Migration and compatibility

- No database migration (C16).
- Compiled output changes on upgrade, at the first recompile of each StarRocks profile (boot pass, C13): new `*`
  denies, masks and row filters for table-level attachments; dropped per-column denies covered by a `*`; possibly
  respelled names on tables reached by a table-level attachment (C5). Plugins re-download once.
- Wire: two fields and two schemas removed (C15); one field and one schema added. Older frontends in the field: none
  (E21).
- Release note (C14): table-level classifications now enforce on StarRocks; check each StarRocks profile's last
  compile after upgrade (a new row-filter tie is refused and recorded there); `GET /perm/overlaps` lists the pairs
  already stored; a schema change that would push a clearance over its ceiling is now refused at the Iceberg commit.

## 10. Risks

- **Engine behaviour is inferred, not yet measured for this shape**: the refusal of an uncleared reader rests on E9's
  hidden-column check plus the `*` deny. The live check (plan Task 16) measures it; if StarRocks returns NULLs instead
  of refusing, U1's "refused the whole table" does not hold on StarRocks and the user decides.
- **`MASK_NULL` on complex types** (struct, array, map) is unmeasured; the per-column mask now reaches every column of
  a table. A column-level attachment on such a column already has this exposure. Measured in the live check.
- **Lakekeeper coupling** (C2): a Lakekeeper rev bump that renames these tables fails every StarRocks compile that
  reaches a table-level attachment, loudly; C3 catches it before merge.
- **Hook latency**: a schema-changing commit on a table-level-classified table costs a dry run and a recompile of each
  bound StarRocks profile (two compiles). Commits on other tables cost nothing or one indexed query.
- **Plugin lag**: "at once" is the compile; the engine applies it on its next policy poll. The `*` deny covers new
  columns for the uncleared meanwhile; a cleared reader may see a new column unmasked by a rule mask for one poll.
- **Rollout wedge** (C14) on a profile where a table-level row filter meets another classification row filter.
- **Hook bypass**: a schema change that does not pass through egret's Iceberg mount (none known: in-process code only
  lists and loads tables) is caught by the next recompile or boot pass, while the `*` deny holds.

## 11. Related defects (found, not fixed here)

1. A table rename or drop leaves its table- and column-level classifications on the old name, on both engines (E12):
   after a rename the table is unclassified everywhere. Needs a view-hold-like move of the assignments.
2. `recompile_role_referencing_profiles` recompiles every profile in one transaction (E13): one refusing profile rolls
   the whole boot pass back. This package's boot pass is per profile and does not depend on it.
3. Package 2 live finding 4: a refused preview links the unsaved rule's id. Unchanged.
4. Masks alone expose `_file`/`_pos` (the object-store path) on `SELECT *` (Step 0 live spike, §14). Today's
   column-level clearance on a table where EVERY column is classified probably has the same exposure: pre-existing,
   unmeasured.
5. A table-commit recompile that fails AFTER the commit (a lock wait past `DEFAULT_LOCK_WAIT`, 10 s, while other
   permission writes hold the profile; or a register whose compensating drop fails) is logged and recorded in
   `perm.compile_runs` (the Overview's `compile_failed` shows it) and retried only by the next recompile of that
   profile (any grant, rule, classification or table write bound to it) or the next boot. Until then a column added by
   that commit has no NULL mask: the `*` deny still refuses its direct read, a view over it reads the real value
   (review finding 5; not mitigated).
6. A classification or rule write that commits between a table change's dry run and its commit can still make the
   after-commit recompile refuse (the per-profile lock serializes table changes only, not every permission write):
   the window is the dry run's end to the lakekeeper commit (milliseconds); the result is item 5's failed-recompile
   state, and the profile stays refused until a steward narrows the classification.

## 12. Non-goals

- Mask RULES on a table-level classification: stay refused at write (E23).
- Wildcard masks of any kind (C1).
- Trino's D3 gap (row filter on a column-level classification): a later package.
- Moving assignments on a table rename or drop (§11.1).
- Platform docs in this PR (C17).

## 13. Open questions

None blocking. Worth the user's eye:

1. C11 refuses a schema-growing Iceberg commit when the recompile it would cause is refused (ceiling, undecidable
   fold, new tie). The alternative is to let it commit and record the failed compile (the profile is then wedged on a
   per-tag ceiling until a steward acts).
2. If the live check shows StarRocks reading NULLs rather than refusing (§10, first risk), keep U1's shape or add a
   table-level access deny? (StarRocks' table resource has no `select`, so there may be no such deny.)

## 14. Implementation notes (append-only)

Deviations the code forces are recorded here, per task, as they happen.

### Step 0 (2026-10-25)

- `origin/main` had moved to `ff03d0f0` (#745, alerts frontend and docs only); the branch was rebased onto it. Every
  Step 0 row still holds; no line drift that matters (E1's filter is at `store.rs:1944`).

### Task 1

- The new row type is `ServiceTagAssignmentRow`, not `TagAssignmentRow`: `egret_policy` already exports a
  `TagAssignmentRow` (the tag download's row). It carries `warehouse_id` beside the spec's fields.
- `fold_identifier` is the evaluator's `fold` renamed and made public (no alias kept); behaviour is unchanged.
- `merge_by_column` folds all four levels (catalog too: Ranger matches it case-insensitively like the others).
  `PendingMask` gained `catalog_spelled: bool`, which is how "the catalog spelling wins" reaches the merge; every
  column-level mask sets it `false` until Task 4.
- `group_by_table` keeps the first spelling in the expansion's `ORDER BY`, which is the database collation's order
  (under `en_US.utf8`, `orders` sorts before `Orders`). A table with one spelling is unaffected.
- The rule mask expansion goes through the same grouping, so one tag on two spellings of one column now compiles one
  rule mask (it compiled two agreeing ones before).

### Task 2

- `mv_deny_inputs` takes one cleared set per gated tag that reaches the profile (either level), not the pending
  masks; the harness wrapper `starrocks_mv_denies` keeps its signature and maps to it.
- The per-tag ceiling already counts a table-level table as 1 here; Task 6 adds its column masks to the count.
- `starrocks_deny` (one body per column or `*`) is factored out of the loop for Task 14's harness.

### Step 0 live spike (2026-10-25)

Measured on StarRocks 4.1.4 by a separate spike (compose project `egret-srspike`); it confirms U1's premise.

- `*` deny + per-column `MASK_NULL` (the compiled shape): an uncleared granted reader gets ERROR 5204 on every direct
  read. StarRocks checks a column it adds to the scan itself: the first column for named masked columns, `_file` for
  `SELECT *`, `___count___` for `count(*)`. Through a plain view (native and Iceberg) the reader gets NULL in every
  column. A cleared user (`denyExceptions` + a `MASK_NONE` item) reads real values directly and through views. A
  column added after the compile, with no mask yet, is refused for the reader.
- `*` deny alone (U4's unknown-table state once the table exists without masks): real values LEAK through a view. So
  the hook filling the masks (U2) is load-bearing, and the compile report and Explain for U4 must say the view path is
  unprotected until the masks exist.
- Caveats: a query whose masked predicate folds to false is not refused (0 rows, nothing leaks), so "refused the
  whole table" holds for reads that scan. `DESC` and `SHOW CREATE TABLE` stay visible. Masks alone expose
  `_file`/`_pos` on `SELECT *` (§11 item 4).

### Task 3

- The §5.2 SQL held against real Lakekeeper as sketched, with one change: the warehouse join compares
  `ti.warehouse_id::text = w.lakekeeper_warehouse_id` (the egret column is `text`) rather than casting the egret
  column to `uuid`, so a malformed value elsewhere cannot fail a compile.
- The batched variant is `current_columns_batch_via(conn, &[TableKey])` (`unnest … WITH ORDINALITY`), answering in
  input order; `current_columns_via` is a one-key call of it. The overlay waits for Task 8.
- `table_columns::LAKEKEEPER_SCHEMA` is the literal the SQL spells; the drift test asserts it equals
  `egret_catalog_engine::LAKEKEEPER_SCHEMA`.
- The drift test switches the warehouse to a soft delete profile before the drop: an in-memory warehouse drops hard
  (the row goes), which would leave the `deleted_at IS NULL` predicate untested (measured by mutation).
- The stub (`tests/support/lakekeeper_stub.rs`) types `typ` as `text` (Lakekeeper's is an enum); the query compares
  it with a literal, which works on both.

### Task 4

- The StarRocks half of `compile_clearance` now runs after every gate is read (`compile_starrocks_gates`), so the
  column source is asked about every whole-table table of the profile in one query.
- C5's spelling rule is applied up front, not only in the merge: on a table the column source knows, every policy
  (the `*` deny, the per-column denies of other tags, every mask) takes the catalog's schema and table spelling, and a
  column-level column the catalog knows takes its column spelling (`catalog_spelled = true`). A column-level
  assignment naming a column the catalog does not have keeps its typed column name and compiles as before.
- A table the column source does not know compiles its `*` deny and no mask; Task 6 adds the skip report.

### Task 5

- `compile_rule`'s StarRocks input is `tagged: &[TableTarget]` (one parameter for both rule kinds) rather than a
  separate `RowFilterTables(&[TableRef])` input: a row filter emits one policy per target table, a mask one per
  column-level column. Every other caller passes `&[]`; `compile_rules.rs` groups its column lists with a local
  `targets` helper.
- The mask branch lost its per-resource `seen` dedup: targets are already one per folded table and columns one per
  folded name, so it could no longer fire.
- The route test `a_starrocks_row_filter_on_a_table_classification_filters_nothing` failed once the row filter
  compiled, so it is renamed `…_filters_rows` here and asserts the filtered decision and the "sees rows where" line.
  Its preview `starrocksTableOnly` assertion stays, and `row_filter_not_applied` (which still, wrongly, lists the
  StarRocks rule) is no longer asserted: both go in Task 11, which completes the inversion.

### Task 6

- `ClearanceExpansionCeiling` gained `tables: Vec<TableCount>` (the five largest whole-table tables) and
  `per_column_denies`; with no whole-table table its message is unchanged. `ProfileExpansionCeiling` gained
  `largest: Vec<TableCount>` (the profile's five largest whole-table tables, from the clearance compile), appended to
  its message. `TableCount { table, columns }` lives in `egret_perm`.
- The U4 skip reason names `catalog.schema.table` and says a view over it is not protected until the table exists
  (the live spike measured that the deny alone lets a view read real values).
- The wire test drives `POST /perm/compile/{id}` (same `CompileResponse.skipped`) rather than a rule write:
  `routes.rs::compile_reports_a_whole_table_clearance_on_an_unknown_table_on_the_wire`. Written after the fix; its
  assertion was mutation-checked instead (dropping the skip fails it).
- A compile that reaches a whole-table attachment queries `lakekeeper.*`; a database without that schema (some test
  databases) fails such a compile. Production always has it (embedded catalog). Tests that need it install the stub.

### Task 7

- The hook is installed by `mount_catalog_engine` (new `table_hook` parameter) through `table_hook::layer`, around
  lakekeeper's router and inside the view hold and the guard; `front_catalog_router`'s signature is unchanged, so tests
  compose `front_catalog_router(table_hook::layer(inner, hook), …)`.
- The affected-profile query is `affected_starrocks_profiles_for_tables(conn, warehouse_id, &[(schema, Option<table>)])`
  (plural: a rename names two tables; `None` is every table of a dropped namespace).
- The prefix resolution, segment decoding, multi-level check and rename body parsing are the view hold's, made
  `pub(crate)` and reused.
- The after-commit recompile runs in a spawned task the handler awaits (as the view hold's after-commit steps), one
  transaction per profile with `lock_timeout` = `DEFAULT_LOCK_WAIT` (10 s). A failure is logged and recorded with
  `record_compile_failure`.
- A failure to look up the affected profiles BEFORE forwarding refuses the request with a 409 ("retry") rather than
  passing it unrecompiled.
- The red evidence for the DB tests is the same tests run with the layer removed (3 of 5 fail; the "runs no compile"
  test was mutation-checked by widening the affected-profile filter).

### Task 8

- The overlay setting is `table_columns::PENDING_COLUMNS_SETTING` (`egret.pending_table_columns`), a JSON array of
  `PendingColumns { warehouse_id, schema, table, columns | null }`; an empty string (what a custom setting reads after
  its transaction ended in the same session) is no overlay.
- What is dry-run: a create (not a staged create, which is judged at its committing update), an update or a
  multi-table commit whose `set-current-schema` picks a new schema (`-1` = the schema added last; an explicit id = an
  added schema of that id, else the STORED schema read by the new `stored_schema_columns_via`), and a rename (the
  source's current columns under the destination, `null` under the source). A **register is not dry-run**: its schema
  is in the metadata file the request points to, which the hook does not read; its after-commit recompile still runs.
  An `add-schema` without a new current schema changes no columns and is not dry-run.
- All affected profiles are dry-run in ONE rolled-back transaction (`lock_timeout` = the hook's lock wait). A refusal
  is a 409 `"egret refused the table change: <compile error>"`; a lock timeout is a 409 asking to retry.
- The red evidence for the ceiling test is the Task 7 hook (HEAD) run against the new test (the commit went through);
  the "dry run leaves nothing" assertion was mutation-checked by committing the dry run.

### Task 9

- `recompile_table_classified_profiles(pool)` lives in `trigger.rs`; profiles = StarRocks engines' profiles bound to
  a catalog whose warehouse holds a table-level assignment of a tag with a role-bearing gate or selected by a
  row-filter rule. One transaction per profile; a refusal is logged and recorded with `record_failure_logged`. It is
  spawned in `egret-bin` beside `sync_admin_policies`, logging the count.
- Its tests are in a binary of their own, `tests/table_boot_pass.rs` (not `clearance_triggers.rs`, whose fixture is a
  Trino profile and whose tests would be recompiled by an installation-wide pass); the two run serially.
- U6: the Overview's `compile_failed` source links every `perm.rules` row whose name appears in quotes in the recorded
  error (a refusal labels rules by their unique name; the order-ambiguous refusal labels both rules of the tie), as a
  sample (`ref: rule`) right after its profile's sample, and its sentence appends "The refusal names rules A and B."
  No migration: the link is derived from the recorded error and the current rules. Tested through the real handler
  with a real boot-pass refusal (`governance_attention.rs::a_boot_pass_refusal_names_and_links_the_two_rules`).

### Task 10

- The clearance half of the Package 2 gap goes here, not in Task 11, because the new tests assert its absence:
  `ExplainProfile.clearance_not_enforced`, the D2 sentence (`append_clearance_gaps`) and the `gated_clearances`
  partition are removed; `ExplainClearanceGap` became the internal `explain_effect::ClearanceRef`. Task 11 keeps the
  row-filter half and the rule effect.
- Wording follows the live spike. A clearance deny whose locator ends in `.*` reads "(clearance *X*, on the whole
  table)" wherever a denial is attributed (column line, table line). Under `masked_read_skips_access` with the table
  `select` denied by such a deny, the line is "u is **refused** email (clearance *X*, on the whole table); through a
  view it reads NULL"; any other denier keeps the "query still needs SELECT on the other columns" tail.
- `clearance_masks_missing` is set when the profile is StarRocks, the column has a gated classification at either
  level, and the served artifact holds NO clearance mask matching the column (any clearance mask counts: one merged
  mask per column, named after the lowest tag id). The reason asks the column source once per request. The sentence
  ends "…, so a view over it shows the values" for both reasons (the spike measured the deny-only leak).
- A StarRocks explain of such a column now queries `lakekeeper.*`; `tests/routes.rs`'s `setup` installs the stub.
- The API shape changed (one field and two schemas out, one field and two schemas in): `openapi.json` and the client
  are regenerated in Task 12, so the egret-server drift test fails between Tasks 10 and 12.

### Task 11

- Removed: `RuleEffect.starrocks_table_only`, `StarRocksTableOnly` (and its OpenAPI registration), `TABLE_ONLY_TABLES`,
  `ROW_FILTER_GAPS_TABLE_ONLY`, the StarRocks arm of `row_filter_not_applied` and the StarRocks tuple of
  `append_row_filter_gaps`. `row_filter_gap_tables` and `row_filters_not_applied` lost their query parameter (only the
  Trino query is left). The clearance half went in Task 10; the store doc "masks a COLUMN, never a whole table" went
  with `tagged_columns_for_service` in Task 1.
- `effect::tag_reach` already counts a table-level table for a row filter (`count(DISTINCT (catalog, schema,
  table))` over both levels); it counts exact-case, so `Orders` and `orders` count twice there (preview count only;
  not changed here).
- `a_starrocks_row_filter_on_a_table_classification_filters_rows` now also asserts no `starrocksTableOnly` in the
  preview and no `row_filter_not_applied` on the StarRocks profile; its red was seen on the Task 10 code.

### Task 12

- `openapi.json` changes only by the Task 10/11 shapes: `StarRocksTableOnly`, `ExplainClearanceGap`,
  `RuleEffect.starrocksTableOnly` and `ExplainProfile.clearance_not_enforced` out; `ExplainClearanceMaskGap`,
  `ClearanceMaskGapReason` and `ExplainProfile.clearance_masks_missing` in.
- `npm run gen:api` (orval 8.24) neither deletes the model files of removed schemas nor their lines in
  `model/index.ts`; both were removed by hand and `npm run gen:api` re-run, so the committed tree is what a fresh
  emit gives (the second run re-added nothing). The digest covers 817 files.
- `egret-server`: `committed_openapi_json_is_current` and `generated_client_was_emitted_from_the_committed_spec` pass.
- `npx tsc -b` errors left for Task 13, both its removals: `RuleDialog.tsx` (`effect.starrocksTableOnly`) and
  `RulePreview.tsx` (the `StarRocksTableOnly` import).
- The worktree had no `frontend/node_modules`; `npm ci` installed it (untracked, ignored).

### Task 13

- Removed `StarRocksTableOnlyNotice`, `starrocksTickReason` and their RuleDialog wiring (`starrocksWarnId`, its
  `aria-describedby` entry); `needsTick = (secondMask || trinoColumnOnly !== null) && !ticked`.
- Copy as §5.8: `TRINO_ROW_FILTER_HINT`, `TABLE_LEVEL_NOTE`, `COMPILES_TO`.
- The "never shows a StarRocks warning" test feeds a preview that still carries the removed `starrocksTableOnly`
  (an older server), so it was red on the old code.
- Not changed: the Clearance tab's ceiling sentence ("if more than 500 of this tag's columns are visible to one
  StarRocks profile…"); with whole-table attachments the count is 1 + columns per table, which the server's refusal
  now names.
- Not verified in a browser (FE-TEST-2); `npm run build` and the bundle tests were not run (the coordinator runs the
  gate).

### Task 14

- `harness_support::starrocks_table_clearance_deny` builds the body with the compiler's own `starrocks_deny`,
  `WHOLE_TABLE_COLUMN` and `withheld_accesses` (made `pub(crate)`).
- `differential::a_whole_table_clearance_deny_decides_as_designed_on_the_real_jars`: on 2.8.0 and 2.9.0, with a
  `select` grant on `wh.*.*.*`, the jars and the evaluator agree on all 18 probes, as designed: the uncleared reader
  is denied a classified column, the column-less TABLE request (what Explain's `other_columns_access` asks),
  `_file` and `___count___`; the cleared reader is allowed each; another table is unaffected.
- `just test-ranger-gates`: exit 0, every line green (the new test runs in its third line).

### Review fixes (2026-10-26)

- `Target::Update` is now `Target::TableCommit`: `egret-workflow`'s `execution_timestamps` guard reads `UPDATE {` in
  any source file as a SQL write.
- **Undrop (review finding 1).** §10's "Hook bypass: none known" was wrong: the recovery page's undrop calls
  lakekeeper's management service in process, past the Iceberg mount. It now goes through
  `TableHook::undrop` (`egret-bin`'s `LakekeeperBrowser::undrop`; one `TableHook` is shared with the mount). For a
  table, the affected StarRocks profiles are found from the soft-deleted row (`table_columns::dropped_table_columns_via`,
  by tabular id), the recompile is dry-run with the table's columns overlaid, a refusal is a 409 (`Conflict`, "egret
  refused the undrop: …") and nothing is undropped (U5 applies to undrop like to a column add). After the undrop the
  profiles are recompiled before the call returns. A view's undrop is unchanged.
- **Register (review finding 2).** A register is still not dry-run before its commit: its schema is in the
  metadata file the body points to, and reading it needs lakekeeper's storage profile and secrets, which the hook does
  not hold. Instead it is checked right AFTER its commit, inside the same request: the affected profiles are
  recompiled in ONE transaction (`recompile_services_in_tx`); if that is refused, the hook drops the table again
  through lakekeeper's own router (`purgeRequested=false`, the files are the client's) and answers 409 "egret refused
  the table change: <refusal>; the table was not registered". The profile's stored compile never changed, so it is not
  wedged (U5 holds). Between the commit and the drop the table is registered with the deny alone (milliseconds). If
  the drop itself fails, the failure is logged and recorded as a failed compile and the 2xx passes (the old,
  wedging outcome, now only on a second failure).
- **Dry-run lock order (review finding 4).** The dry run calls `recompile_services_in_tx(&mut tx, &profiles)` (every
  profile locked ascending, then one pre-mint) instead of `recompile_service_in_tx` per profile. No deterministic red
  test was built for the interleaving (it needs a concurrent writer holding the second profile and wanting a minted
  `users` row); `a_column_add_reaches_every_bound_starrocks_profile` covers the two-profile path and passes before and
  after.
- **Dry-run spelling (review finding 6): not changed.** The overlay still carries the request's spelling of the
  schema and table (`table_hook::pending`), so a client committing to `SALES/ORDERS` while the catalog holds
  `sales.orders` dry-runs clearance-mask names in the request's spelling. Ceilings are unaffected; the risk is a
  stored mask tie looking new to the dry run's overlap check, which would refuse a change the real compile accepts
  (a 409 the client can retry with the catalog's spelling; never a wedge). Fixing it for an update is cheap
  (`current_columns_via` gives the catalog spelling), but no red test through the handler was found that is cheaper
  than the fix's risk, and a create needs the namespace's stored spelling from another Lakekeeper table the drift
  test does not cover. Left as is.
- **Clearance tab ceiling copy (review finding 7):** now says a whole table costs 1 plus its column count and that the
  write causing the excess is refused.
- **Hook tests (review finding 7).** `a_rename_of_a_classified_table_is_dry_run_and_commits` now also renames a table
  onto a classified name that the moved columns push over the ceiling and expects the 409 (fails with the rename dry
  run removed). New DB tests drive a multi-table `transactions/commit`, a staged create's committing update and a
  recursive namespace drop through the hook (each fails with the layer removed). They pass on the code as it was:
  they are coverage, not reds.
- **Concurrent schema changes (review finding 3).** The hook now takes a session-level advisory lock per affected
  profile (ascending, on a connection detached from the pool, `COMMIT_LOCK_WAIT` = 30 s, timeout → 409 "retry") before
  the dry run and holds it through the lakekeeper commit and the after-commit recompile; `TableHook::undrop` takes
  the same locks. Two schema changes over one profile therefore run one after the other, and the second dry-runs
  against the first's committed columns
  (`concurrent_column_adds_that_cross_the_ceiling_together_do_not_wedge`, red before). Not covered: a classification
  or rule write between a dry run and its commit (§11 item 6).
- **After-commit failures (review finding 5): not mitigated.** Recorded in §11 item 5 with the window. The module doc
  and C10's "the boot pass retries it" now read "the next recompile of the profile, or the next boot".

### Live check fixes (2026-10-26)

- **F1: a ceiling refusal is a 409 on every write path.** `CompileError::expansion_ceiling_body()` (egret-perm) gives
  `{error: <the compiler's sentence>, code: "expansion_ceiling"}` for `ExpansionCeiling`,
  `ClearanceExpansionCeiling` and `ProfileExpansionCeiling`. Before → after:
  - tag assignment and tag rename (`BiError`): 502 → coded 409;
  - clearance write (`PUT /tags/{id}/access-policy`, via `BiError` → `CpError`) and a role-membership write
    (`CpError` directly): 500 → coded 409;
  - rule writes (`PermHttpError`): 400 → coded 409;
  - catalog-workspace assignment (`EngineHttpError`): 500 → 409 `Conflict` with the sentence; an engine bind
    already answered 409.
  - A genuine engine failure keeps 502 (`BiError::Engine`), a database failure 500.
  The frontend already shows a write's `error` sentence (`ApiError.message`); `AddTagDialog` gained a test pinning
  it for the 409. No API schema changed.
- **F7 (cleared reader on a table not in the catalog): not changed.** Explain reads "cleared" from the served
  clearance MASK (the reader is named in its items). With no mask served (U4), an allowed `select` does not say why:
  the reader may be excepted by the whole-table deny (cleared) or, for a column whose deny was not compiled yet
  (`not_compiled`), simply not denied. Saying "(cleared for X)" there would need a second evaluation of the deny's
  exceptions, so the line keeps "reads email as the real value" plus the masks-missing sentence, which stays true
  for that profile (a view over the table shows the values to the uncleared).
