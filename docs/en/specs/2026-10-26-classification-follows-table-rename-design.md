# Classifications follow a table or column rename: design

Status: design, 2026-10-26. Not implemented. Base: `origin/main` `28fb8693` (#755). Branch
`fix/classification-follows-table-rename`.
Plan: [`../plans/2026-10-26-classification-follows-table-rename.md`](../plans/2026-10-26-classification-follows-table-rename.md).
Programme: Control Plane & DG, governance defects. Parent: the StarRocks table-level classification spec
[`2026-10-25-starrocks-table-classification-design.md`](2026-10-25-starrocks-table-classification-design.md) §11 item 1
(the defect this package fixes) and its §5.5 (the table-commit hook this package extends).
Precedent: the view hold (`crates/egret-catalog-engine/src/view_hold.rs`, view governance B2/B3).

The user settled nine questions on 2026-10-26 (§3): R1–R4 before the design, R5–R9 answering its first draft's open
questions. The controller set the other defaults (§4, C1–C20); the user may revisit those. Every UI string uses the classification glossary
(`2026-10-21-classification-ux-design.md` §4).

## 1. Problem

A classification on a catalog table or column is stored **by name**: `(tag, warehouse, schema, table, column)`, with
`column = ''` for the whole table (E1). Nothing stores the Lakekeeper table id or the Iceberg field id. So:

- **A table rename** leaves every classification of the table on the old name. On both engines the renamed table is
  unclassified the moment the rename commits: no clearance deny, no mask, no row filter (E9, E10, E21, E22). The old
  name keeps classifications for a table that no longer exists.
- **A column rename** leaves the column's classifications on the old column name. Trino keeps masking a column that no
  longer exists and leaves the renamed column unmasked; StarRocks compiles names verbatim and does the same (E12).
- **The table-commit hook of #755** sees renames, but only recompiles StarRocks profiles, and only for a whole-table
  classification with a role-bearing clearance. It never moves rows and never bumps the Trino tag version (E9, E10).
- **Rows left by past renames, drops and purges** cannot be told from live ones anywhere in the product (E26).

A move done only **after** the rename commits still leaves a window: up to two plugin polls on Trino, and up to one
policy poll on StarRocks, in which the renamed table is readable unprotected (E23, E24). The view hold closed the same
window for views by writing first, waiting for the plugins, and only then letting the commit through (E16).

## 2. Evidence (verified at `28fb8693`)

Paths are relative to the repo root. `VH` = `crates/egret-catalog-engine/src/view_hold.rs`, `TH` =
`crates/egret-catalog-engine/src/table_hook.rs`, `TC` = `crates/egret-perm-compile/src/table_columns.rs`, `TR` =
`crates/egret-perm-compile/src/trigger.rs`, `CT` = `crates/egret-bi/src/catalog_tags.rs`, `PD` =
`crates/egret-policy/src/plugin_downloads.rs`, `DL` = `crates/egret-policy/src/ranger/api/download.rs`, `GO` =
`crates/egret-controlplane/src/governance_overview.rs`, `LK/` = the git-pinned Lakekeeper checkout at rev
`791650571d` (`crates/egret-catalog-engine/Cargo.toml:32-36`), under its `crates/`.

### 2.1 Storage and identity

| id | fact | where |
|---|---|---|
| E1 | `catalog_tag_assignments` is keyed `(tag_id, warehouse_id, schema_name, table_name, column_name)`; `column_name = ''` is a table-level tag; `warehouse_id` has no FK; the header says a catalog resource "is NOT uuid-keyed" | `migrations/0081_catalog_tag_targets.sql:1-21` |
| E2 | `from_lineage` separates view rows the lineage derived from manual rows | `migrations/0188_view_tag_provenance.sql:15` |
| E3 | A CHECK refuses empty schema/table and any `*`/`?` in a level. Only single-level namespaces are recorded; multi-level is `ns.contains('\u{1f}')` | `migrations/0177_catalog_tag_levels_are_present_and_literal.sql:147-157`; `VH:396-398` |
| E4 | Lakekeeper's `tabular_id` is the Iceberg table-uuid; a rename is `UPDATE tabular SET name` by id; an undrop is `SET deleted_at = NULL` by id, so the table returns under its old name. Name uniqueness admits any number of soft-deleted tables beside one live one | `LK/lakekeeper/src/server/tables.rs:353,444`; `LK/lakekeeper-storage-postgres/src/tabular/mod.rs:1238-1300,1537-1570`; `LK/lakekeeper-storage-postgres/migrations/20250904142650_reusable_table_id.sql:334` |
| E5 | egret already reads `lakekeeper.*` over the compile connection: current columns by folded name, a dropped table's columns by `tabular_id`, any stored schema by id. `top_level_columns` reads each field's `id` and `name` but returns names only | `TC:76`, `TC:163`, `TC:213`, `TC:246-268` |

### 2.2 Rename and drop paths

| id | fact | where |
|---|---|---|
| E6 | egret serves only Lakekeeper's Iceberg REST router; the layers are guard → view hold → table hook → Lakekeeper | `crates/egret-catalog-engine/src/mount.rs:271-310` |
| E7 | The table hook classifies create, register, update (counts for `add-schema`/`set-current-schema`), multi-table commit, rename `POST …/tables/rename`, drop, and namespace drop; for a rename `tables_of` returns both sides; view paths are not its own | `TH:147-186`, `TH:189-200`, `TH:220-253`; test `reads_and_view_requests_are_not_this_hooks` |
| E8 | The hook's namespace-drop classification ignores the query string, so it cannot tell a recursive drop from a plain one | `TH:147-162` |
| E9 | The hook acts only through `affected_starrocks_profiles_for_tables`: a StarRocks engine bound to the warehouse AND a whole-table row whose tag has `cardinality(required_roles) > 0`. Column-only classifications, group-only clearances, row-filter-only tags and Trino-only deployments are not seen | `TR:1613-1645`; `TH:323-345` |
| E10 | When it acts on a rename it dry-runs with the source's columns overlaid on the destination and `None` on the source, then recompiles StarRocks after the commit. It never writes `catalog_tag_assignments` and never bumps a tag version (no `tag_version`/`bump_bound_tag_versions` in the file); its module doc states the defect | `TH:347-420` (Rename arm `TH:396-407`), `TH:422-455`, `TH:66-69` |
| E11 | Undrop runs in process through `TableHook::undrop`, which finds the dropped table by `tabular_id` and recompiles **by that table's name** | `crates/egret-bin/src/main.rs:1068-1081`; `TH:502-575` |
| E12 | A column rename is an update with `add-schema` and `set-current-schema`; the hook replays them (`new_current_columns`) into a column list; the field id survives an Iceberg rename and is in the request's `add-schema` | `TH:255-301`; `TC:246-268` |
| E13 | A recursive namespace drop hard-deletes its children and needs `force` under soft deletion, so nothing in it can be undropped; the flag is the query parameter `recursive` | `LK/lakekeeper/src/server/namespace.rs:536-545`, `:709-730`, `:839`; `LK/lakekeeper/src/api/iceberg/v1/namespace.rs:26-45` |
| E14 | The view hold has no namespace-drop handling: views' lineage and derived rows in a dropped namespace remain | `VH:335-372` |
| E15 | Purge of soft-deleted tables is a Lakekeeper task no egret code observes | findings E14 (Lakekeeper `tabular_expiration`) |

### 2.3 The precedent and the write path

| id | fact | where |
|---|---|---|
| E16 | View rename: resolve; a classified source with either side multi-level is refused (`Refusal::multi_level`); a source carrying nothing passes with no lock; try-locks on both names; the rest runs uncancellable; re-read under the lock; `write()` copies rows with each row's own `from_lineage`, `INSERT … ON CONFLICT DO NOTHING` (a union), `bump_and_recompile`, `refuse_created_overlaps_via`, and reads the Trino tag versions in the same transaction; wait; forward; `compensate` on non-2xx; after 2xx delete the source rows and recompile, a failure only logged | `VH:1801-1863`, `VH:1866-1948`, `VH:975-1084`, `VH:825-862`, `VH:745-764`, `VH:1096-1155`, `VH:1157-1170` |
| E17 | One deadline per held request: 3 × the 30 s poll = 90 s; Trino 483's REST client tolerated a full 90 s hold | `VH:56-68`, `VH:139-143`; `crates/egret-engine-mgmt-api/src/ranger_config.rs:35` |
| E18 | Name locks: `pg_try_advisory_lock` per `lock_key(prefix, ns, name)` on a detached connection; busy refuses at once; holds are capped (`DEFAULT_MAX_HOLDS` = 8) | `VH:928-972`, `VH:2189`, `VH:132` |
| E19 | `assign_catalog` runs, in one transaction: coordinate check, unreadable-table-mask guard, `ViewReach::of` (view-tag lock, dependent views, their warehouses, profile locks then compile-mint lock), overlap snapshot, INSERT, `retag_bump_and_recompile`, overlap refusal. `ViewReach` is private to `egret-bi`; `egret-catalog-engine` does not depend on `egret-bi` | `CT:84-200`, `CT:395-490`; `crates/egret-catalog-engine/Cargo.toml` (no `egret-bi` in `[dependencies]`) |
| E20 | #755's hook takes session-level advisory locks per affected profile (`egret-table-hook` keyspace, ascending, `COMMIT_LOCK_WAIT` 30 s) before its dry run and holds them through the commit and the after-commit recompile; `TableHook::undrop` takes the same locks | `TH:612-670`, `TH:713-830` |

### 2.4 Engines

| id | fact | where |
|---|---|---|
| E21 | `download_tags` returns 304 while `lastKnownVersion == tag_version`; otherwise it serves one resource per distinct `(catalog, schema, table, column)`, empty levels omitted (a table-level row is an ancestor of every column), names verbatim, matched case-insensitively | `DL:270-317`; `crates/egret-policy-api/src/dto.rs:142-220` |
| E22 | On a rename today no row changes and no tag version moves: the plugin keeps tagging the old name. A case-only rename is harmless: matcher and resolvers fold case | `TR:1631-1633`; E21 |
| E23 | `wait_for_tag_version` polls the ledger every 500 ms until each `(service, tag version)` is reported by every plugin that polled within 90 s; with no plugin polled it never confirms (`NO_PLUGIN_POLLED`) | `PD:20-30`, `PD:136-200` |
| E24 | Policy downloads are recorded in the same ledger (`DownloadKind::Policies`, `lastKnownVersion` against `policy_version`), but only a tag-version wait exists. StarRocks' plugin sends `pluginId` and `lastKnownVersion` on its policy polls (measured) | `DL:129-136`; `PD:33-36`; `docs/en/plans/2026-10-20-ranger-surface-auth-live-result.md:938` |
| E25 | A recompile bumps a profile's `policy_version` only when its compiled bodies change | `TR:1320-1340`, `TR:1344-1356` |

### 2.5 Detection, audit, surfaces

| id | fact | where |
|---|---|---|
| E26 | No check finds assignments on a name with no live table; "Where used" counts only rows whose warehouse no catalog names (`orphaned`) | `crates/egret-controlplane/src/governance_tag_usage.rs:150-200` |
| E27 | Governance events are one INSERT on the write's own transaction (`record_change`); it binds the actor's user id. The feed renders a NULL actor as "egret". `tag:assign`/`tag:unassign` on `catalog` are legacy best-effort sink events | `crates/egret-audit/src/governance.rs:1-15`, `:217-254`, `FAMILIES` `:74-145`; `crates/egret-controlplane/src/governance_changes.rs:286-293` |
| E28 | The catalog guard returns `Ok(())` only; no caller identity reaches the layers behind it | `crates/egret-catalog-engine/src/mount.rs:245-254`, `:334-353` |
| E29 | Overview findings are `{kind, severity, count, sentence, sample ≤ 5, ref}`; the frontend renders any kind generically and names each check in `CHECKS` for its "Not checked" line | `GO:126-140`, `GO:211-315`; `frontend/src/routes/governance/overview/AttentionSection.tsx:9-26` |
| E30 | "Where used" lists every table of a classification (`TableUsage`), the full list the Overview's five samples cannot be | `crates/egret-controlplane/src/governance_tag_usage.rs:44-120`; `frontend/src/routes/governance/classifications/tabs/WhereUsedTab.tsx` |
| E31 | Other name-keyed state that a rename leaves behind: path grants and rules (`perm.grants`, `perm.rules`; a deny or a mask/row filter is escaped), hand-written Ranger policies, embedded-catalog RBAC `catalog.catalog_grants` | `migrations/0154_perm_model.sql:13-90`; `migrations/0007_authz.sql:18`; `crates/egret-catalog-engine/src/store/embedded.rs:457-473` |
| E32 | Highest migration on main: `0209`. No open PR (`gh pr list --state open` is empty, 2026-10-26) | `migrations/`; `gh pr list` |
| E33 | **The caller of an Iceberg request.** The guard verifies the bearer JWT and reads its claims (`claims` from `verify_with_jwk`), accepting only `aud = "catalog"`; such a token is minted only by the machine IdP for a registered client, with `sub` = that client's `client_id` (`catalog.catalog_clients.client_id`, `consumer_type` `engine`/`notebook`/`external`). No user is ever the subject of a catalog token: an engine reaches the catalog with its own catalog client, whatever user runs the query. The guard returns `Ok(())` and drops the claims | `crates/egret-bin/src/main.rs:512-560` (`EgretCatalogGuard::check`, claims at `:530`, audience `:545`); `crates/egret-machine-idp/src/lib.rs:506-530` (`mint`, `sub` at `:515`); `migrations/0053_embedded_catalog_rbac.sql:30-41`; `crates/egret-catalog-engine/src/mount.rs:245-254`, `:334-353` |
| E34 | `audit.audit_log` has one actor column, `actor_user_id uuid`; the feed labels it through `users` | `migrations/0076_audit_log.sql:5-17`; `crates/egret-controlplane/src/governance_changes.rs:176-184`, `:286-293` |
| E35 | **A plain (non-recursive) namespace drop is refused when the namespace holds any tabular, live or soft-deleted** ("Contains N tables…, M soft-deleted tables…"; a staged table hits the foreign key and is `NamespaceNotEmpty` too). So a plain drop never removes a namespace that still has a dropped table | `LK/lakekeeper-storage-postgres/src/namespace.rs:651-740` (refusal `:716-736`), `:760-775` |
| E36 | Lakekeeper keeps every schema of a table (`lakekeeper.table_schema`, by `schema_id`), so a column name absent from the current schema can be looked up in older schemas by field id | `TC:213-240`; findings E37 |

## 3. User decisions (2026-10-26, not reopened)

- **R1. Move by name, before the commit, with a hold** (view-hold style). On a table rename whose source carries
  classification rows, lock both names. In one transaction: copy the rows to the destination name, bump the tag
  version, and recompile the affected profiles (Trino and StarRocks). If the recompile or the overlap check refuses,
  answer 409 and do not rename. Otherwise wait until the Trino plugins confirm the new tag version, forward the rename,
  and undo the copy if the rename fails. After a 2xx, delete the source rows and recompile. Hold only when the source
  carries rows.
- **R2. Drop keeps the rows dormant on the name** (today's behaviour; keeps undrop correct). Add a read-only Governance
  report (an Overview finding) for rows whose table is purged or does not exist. A recursive namespace drop, which
  cannot be undropped, deletes the rows under it.
- **R3. A column rename follows by Iceberg field id**, paired from the schema update request (`add-schema` /
  `set-current-schema`), under the same hold. Rows for a dropped column stay dormant. Hold only commits that rename a
  classified column.
- **R4. Classifications only.** Path-selector deny grants and rules, hand-written Ranger policies and
  `catalog.catalog_grants` are not moved; they are related defects (§11), tracked in the docs as KI rows.

Answers to the first draft's open questions (2026-10-26):

- **R5. A bound engine whose plugins are silent:** after the 90 s deadline the classified rename is refused with 409
  and the copy is undone (the view hold's "confirmation is never vacuous"; was C4's second half).
- **R6. The hold does not cover #755's column adds.** KI-305 (a view over a whole-table-classified table reads a new
  column's real value on StarRocks until the next policy poll) stays open. The policy-version wait built here
  (§5.5) is public and general so a later package can hold those commits with it (§11 item 6, §12).
- **R7. The governance events record the real caller.** The token subject is passed from the Iceberg guard into the
  hook, and the `tag:move`/`tag:purge` event's actor is that principal, not "egret" (§5.7). Today every catalog token's
  subject is a catalog client (E33); the design records whichever principal the token names.
- **R8. A third finding** for column-level rows that name no current column of a live table, each with a field-id
  rename candidate from the schema history when one exists (§5.8).
- **R9. A plain namespace drop over soft-deleted tables:** purge those tables' rows too, if the case exists. **It
  cannot happen:** Lakekeeper refuses a plain drop while any tabular, live or soft-deleted, is in the namespace (E35).
  Nothing is built for it; a plain drop only ever removes an empty namespace, whose pre-placed rows (if any) the
  report shows as `missing`.

## 4. Controller decisions (defaults; the user may revisit)

| # | Decision | Why |
|---|---|---|
| C1 | **Renaming onto a name that already has rows unions them** (`INSERT … ON CONFLICT DO NOTHING`), guarded by the overlap check; the governance event lists the rows that were already there | Fails closed (the table ends up with at least its own classifications); it is the view hold's rule (E16); refusing would block a rename on dormant rows a steward may not know exist, and the stale report (§5.8) shows them |
| C2 | **A classified source going into a multi-level namespace is refused** (409, "use a single-level schema"). A multi-level source carries nothing and passes | No row can be stored under a multi-level name (E3), so the classification could not follow; the view hold refuses the same way (E16) |
| C3 | **A cross-namespace rename moves the rows as normal** (both namespaces single-level, same warehouse) | Lakekeeper allows it and keeps the table id (E4); a cross-warehouse rename cannot happen (the prefix scopes it) |
| C4 | **No Trino engine bound: not refused, no Trino wait.** (A bound engine whose plugins are silent is R5: refused after the deadline, copy undone.) | Unlike a view, a table is useful with no Trino; refusing would block renames on StarRocks-only deployments |
| C5 | **Build a StarRocks policy-version wait** (not a Limitation): the hold also waits until every StarRocks plugin bound to the warehouse reports the `policy_version` the copy transaction left, by the same ledger, same 90 s window, same non-vacuous rule; no StarRocks bound means no StarRocks wait | Without it a renamed table is unprotected on StarRocks for up to one 30 s policy poll, the exact leak R1 closes on Trino. The ledger already records StarRocks policy polls with `pluginId` and `lastKnownVersion` (E24), and versions move only when bodies change (E25), so the wait is a sibling of `plugins_hold_tag_version` reading `kind = 'policies'`. Both waits share one deadline and run in one loop |
| C6 | **In-transaction governance events `tag:move` and `tag:purge`** (`resource_type` `catalog`, family Tag), recorded in the after-2xx transaction that deletes the source rows (move) or the namespace's rows (purge), with the caller of R7 as actor | The event exists exactly when the move completed (a refused or undone move leaves none, E27's rule) |
| C6a | **How R7's actor is stored: a typed actor in the event, no migration.** A user subject (none today) goes to `actor_user_id` as every event does. A catalog-client subject cannot (it is not a user, E34): `actor_user_id` stays NULL and `detail.actor = {"kind": "catalog_client", "id", "client_id", "name", "consumer_type"}`; the feed's actor phrase reads it ("Catalog client *trino-main*"). Only a request with no guard (tests) has no caller and reads "egret" | A new `actor_client_id` column would need a migration in a namespace shared with every open PR for one event family; the feed is the consumer and reads `detail` already. §13 notes the alternative |
| C7 | **One hook pipeline.** The move is a branch of `table_hook.rs`, not a third layer. For a rename (or column rename) whose source carries rows, the hold's copy transaction IS the pre-commit compile: it replaces #755's dry run, so a held rename compiles twice (copy, then delete) like the view hold, never three times. The #755 per-profile session locks are taken over every profile bound to the warehouse, ascending, in the same keyspace and wait, after the name try-locks and before any transaction; the copy, compensation and delete transactions take the compiler's profile locks in ascending order (`recompile_services_in_tx`) | #755's review finding 3 needs every change of a profile's column set to serialize from compile to recompile; a move changes Trino and StarRocks profiles alike. Name locks are try-locks, so they can never wait in a cycle; profile locks are always ascending, so two holds cannot deadlock |
| C8 | **The copy's compile counts each moving coordinate once.** The hook overlays the destination with the source's columns and marks the source (whole table, or the renamed columns) as `leaving`; the ceilings skip policies on a leaving coordinate. The policies are still emitted | During the hold the source table still exists and must stay protected, so both names compile; counting both would refuse a wide table's rename that fits once it is done (a 300-column whole-table classification would count 602 > 500) |
| C9 | **Views keep their own path.** `POST …/views/rename` stays the view hold's. A `POST …/tables/rename` whose source is not a live table in Lakekeeper (a view, a dropped table, nothing) is forwarded with no move and no hold; Lakekeeper answers it | A view's rows already move with the view hold (E16); handling them twice would double-insert and double-wait |
| C10 | **A case-only rename** (same folded namespace and name) is forwarded with no move and no hold | Both engines and every resolver fold case (E22); a hold would cost up to two polls to change spelling only |
| C11 | **Dependent views are re-tagged** in the copy transaction (views reading the destination name) and in the delete transaction (views reading the source name), with `ViewReach`'s lock order, as `assign_catalog`/`unassign_catalog` do. `ViewReach` moves to `egret-perm-compile` (public) so both callers share it | A view's derived rows must equal what its lineage gives; skipping the re-tag would leave a broken view over the old name over-tagged and leave the sweep to fix it later. One implementation of the lock order (E19) |
| C12 | **"Renames a classified column" (R3)** = a field id present in both the stored current schema and the commit's new current schema, whose folded name changed, and which carries a column-level row, or whose table carries a whole-table row and a StarRocks engine is bound (StarRocks masks per column name). Pairs come from the request and the stored schema only; rows for an id that vanished stay dormant | Field ids are the only rename evidence (E12). A whole-table classification covers every column, and on StarRocks its masks are named per column, so a renamed column of such a table needs the StarRocks wait too; on Trino it is an ancestor tag and needs nothing |
| C13 | **A recursive namespace drop deletes**, after its 2xx: every row whose folded schema is the namespace (any table, any column, manual or derived), the lineage of the views in it, and re-tags the views elsewhere that read its tables; one transaction, bump and recompile; a failure is logged and the rows stay (the stale report shows them). A non-recursive drop changes nothing | R2. A non-recursive drop removes only an empty namespace, never one with dropped tables (E35, R9) |
| C14 | **Stale-row report: SQL against `lakekeeper.*` in `table_columns.rs`** (the one module coupled to Lakekeeper's tables, guarded by the drift test), three states per distinct `(warehouse, folded schema, folded table)`: `live` (a live table or view of that name), `dropped` (only soft-deleted tabulars of that name), `missing` (none: renamed away, purged or pre-placed, which cannot be told apart) | E26; the same coupling point and guard as #755's column source |
| C15 | **Surfaced twice, no new endpoint:** three Overview findings, `classifications_on_missing_tables` (warning), `classifications_on_dropped_tables` (info) and `classifications_on_missing_columns` (warning, R8), each with up to five samples linking to the classification; and, on each `TableUsage` row of the existing "Where used" endpoint, a `state` and a `missingColumns` list, shown in the Where used tab | The Overview shows up to five samples (E29); the Where used tab already lists every table of a classification (E30), so the full list belongs there, not in a second endpoint. "Missing" table or column is a warning because a renamed-away table or column may be unclassified under its new name; "dropped" is info because undrop needs those rows |
| C16 | **The hold's deadline and cap:** one deadline per held request, the view hold's `default_hold_limit()` (90 s), covering locks, copy, wait and forward; at most `DEFAULT_MAX_HOLDS` (8) moves in flight, a semaphore of the table hook's own; past it, 409 at once | Same timing evidence as the view hold (E17); a separate cap keeps a burst of view commits and table renames from refusing each other |
| C17 | **Name locks share the view hold's keyspace** (`view_hold::lock_key(prefix, ns, name)`), with `pg_try_advisory_lock` (busy → 409 at once) | Tables and views share Lakekeeper's tabular namespace; a view hold and a table move on one name must exclude each other |
| C18 | **Row selection is folded, row writes keep spelling.** Source rows are those whose `lower(schema_name)`, `lower(table_name)` (and `lower(column_name)` for a column) match; each copy keeps its own column spelling and `from_lineage` and takes the request's destination spelling for the renamed level | Rows may be stored in any case (E21's resolvers fold); the destination spelling is the catalog's new name |
| C19 | **No migration.** The hold keeps no queue; the report and the events need no table | Migration numbers are shared; none is needed (E32) |
| C20 | **Platform docs in a separate docs PR after merge**, with KI rows for §11's related defects | The flow the user chose on 2026-10-23; R4 |

## 5. Design

### 5.1 Where the move runs

`hook_table_commits` (`TH:713`) gains one branch, decided after `classify` and the body read and before #755's
affected-profile lookup:

```text
classify → read body →
  Rename:        move_plan_for_rename(...)        ┐
  TableCommit /  move_plan_for_columns(...)       ├─ Some(plan) → hold_move(plan)   (§5.3)
  Transaction:                                    ┘   None       → #755 path, unchanged
  DropNamespace (recursive=true): #755 path, then purge after 2xx (§5.6)
  everything else: #755 path, unchanged
```

A **move plan** is the set of coordinate moves one request makes, each `from → to` at table level (a rename) or at
column level (a column rename inside a table commit), plus the rows each source carries. `None` when nothing moves:
the source carries no row (R1), the rename is case-only (C10), the source is not a live table (C9), or the commit
renames no classified column (C12). A plan whose source is classified and whose destination namespace is multi-level
is refused (C2).

The plan lookup is one indexed query per rename and, for a schema commit, one stored-schema read and one indexed
query; an append (no `add-schema`/`set-current-schema`) does no query at all (#755's C12, unchanged).

### 5.2 The move plan

**Table rename** (`move_plan_for_rename`): parse the body with `view_hold::rename_of`. Then, on one pooled
connection:

1. `resolve(prefix)` → `(warehouse_id, catalog)`; none → no plan.
2. Source multi-level → no plan (it carries nothing). Case-only → no plan.
3. Source rows: `SELECT tag_id, schema_name, table_name, column_name, from_lineage FROM catalog_tag_assignments WHERE
   warehouse_id = $1 AND lower(schema_name) = lower($2) AND lower(table_name) = lower($3)`. None → no plan.
4. The source must be a live **table**: `table_columns::current_fields_via` (§5.4) returns `Some`. A view, a dropped
   table or nothing → no plan (C9); Lakekeeper answers the rename.
5. Destination multi-level → refuse (C2).
6. Plan: one table-level move `(src_ns, src) → (dst_ns, dst)`; the rows to insert are the source rows with schema and
   table replaced by the destination's spelling; the overlay is `dst = src's current columns`, `src` leaving (C8).

**Column rename** (`move_plan_for_columns`, for `TableCommit` and each table change of a `Transaction`):

1. Only a change whose `updates` set a new current schema (`new_current_fields`, the field-id version of
   `new_current_columns`, `TH:255`).
2. `old = current_fields_via(warehouse, ns, table)` (stored current schema, `(id, name)` per top-level field);
   `new` = the request's new current schema fields.
3. `renames = { (id, old_name, new_name) : id in both, fold(old_name) != fold(new_name) }`. Swaps (`a→b`, `b→a` in one
   commit) are two pairs.
4. Keep the pairs that are classified (C12): rows with `lower(column_name) = lower(old_name)` on the table (folded), or
   any whole-table row on the table while a StarRocks engine is bound to the warehouse.
5. None → no plan. Otherwise the plan has one column-level move per pair; the rows to insert are each pair's column
   rows under `new_name`; the overlay is the table with `columns = old ∪ new` (the old columns still exist during the
   hold) and `leaving = old names not in new` (renamed and dropped).

The target state is written as **sets**, which handles swaps and unions with no special case:

- `Before` = the rows on the affected sources and destinations, read under the locks (§5.3 step 4);
- `Target` = `Before` with each moved coordinate's rows re-pointed to its destination, unioned with the destination's
  rows already there (C1);
- the copy inserts `Target \ Before`; the after-2xx step deletes `Before \ Target`; compensation deletes exactly what
  the copy inserted.

### 5.3 The hold (`hold_move`)

Order, under one deadline of `limit` (90 s, C16) from the request's arrival:

1. **Cap:** take a permit of the hook's semaphore, or 409 "egret is holding 8 classified table changes already, its
   maximum; retry in a minute".
2. **Name locks** (C17): `pg_try_advisory_lock(hashtextextended(lock_key(prefix, ns, name), 0))` on a detached
   connection for every source and destination table name of the plan, sorted, deduped. Busy → close, 409
   "another change to this table or its new name is being held; retry when it has finished (at most 90 s)".
3. **Profile locks** (C7): `lock_profiles(services_bound_to_warehouse(warehouse))` (#755's function, `TH:633`), the
   same `egret-table-hook` keyspace, ascending, `COMMIT_LOCK_WAIT`. Busy → 409 retry.
4. From here the work runs **uncancellable** (`view_hold::uncancellable`, made `pub(crate)`): a client that disconnects
   does not stop the wait, the forward or the compensation.
5. **Copy transaction** (`copy`), with `lock_timeout` = the deadline's remainder, at least 1 s:
   1. `ViewReach::of(tx, warehouse, dst)` (C11) for each destination table: view-tag lock, dependent views, their
      warehouses, profile locks then compile-mint lock.
   2. Re-read `Before` under the locks; rebuild `Target`; if the source has lost its rows meanwhile, the plan shrinks
      (nothing to insert → skip to step 7 with no wait).
   3. Overlap snapshot over the reach's services.
   4. `INSERT … ON CONFLICT DO NOTHING RETURNING` each row of `Target \ Before`; keep the inserted ones.
   5. `SELECT set_config('egret.pending_table_columns', <overlay>, true)` (C8): the destination's columns, the leaving
      marks.
   6. `retag_bump_and_recompile` (views, bump of the written and touched warehouses, `recompile_services_in_tx` over
      the services, notify the sweep).
   7. `refuse_created_overlaps_via`.
   8. Read the required versions in the same transaction: Trino `(service, tag_version)` (`VH:745`, moved to a shared
      helper) when rows were inserted, and StarRocks `(service, policy_version)` for the StarRocks profiles bound to
      the warehouse.
   9. Commit. A compile refusal (ceiling, undecidable fold, overlap, the 0177 CHECK on a destination name holding `*`
      or `?`) rolls everything back: 409 "egret refused the table change: the table's classifications cannot move to
      `<dst>`: <compile message>; resolve it, then retry". The rename is not forwarded (R1).
6. **Wait** (C4, C5, R5): `wait_for_versions(policy, &tags, &policies, remaining)` (§5.5). Empty Trino and StarRocks sets
   → no wait. Timeout → compensate, then 409 "engines did not confirm the moved classifications within 90 s: <plugin
   ids>; check that the engines are running and polling egret's Ranger endpoint, then retry".
7. **Forward** to Lakekeeper. Non-2xx → compensate, return Lakekeeper's response as it is.
8. **After 2xx** (`finish`), one transaction: `ViewReach::of(tx, warehouse, src)`; delete `Before \ Target` (the
   source rows; for a column move, the old column rows); `retag_bump_and_recompile` with **no** overlay (Lakekeeper
   now has the destination); record `tag:move` (C6). A failure is logged and recorded as a failed compile of each
   profile (`record_compile_failure`): the old rows stay, which over-tags and never under-tags.
9. Release the profile locks, then the name locks and the permit. Return the response.

**Compensation** (`compensate`): one transaction under `ViewReach::of(dst)`: delete exactly the inserted rows that
still carry the provenance they were inserted with (`VH:1960`'s rule), `retag_bump_and_recompile` with no overlay. On
failure: `error!`, the copied rows stay (over-tag on a name that did not get the table).

What each engine serves at each point:

| point | rows | Trino tags | StarRocks policies |
|---|---|---|---|
| before | source | source | source (masks per source column) |
| after copy, during the wait | source ∪ destination | both names | both names; destination masks from the overlay |
| after rename, before `finish` | source ∪ destination | both | both |
| after `finish` | destination | destination | destination (columns read from Lakekeeper) |

The renamed table is protected from its first read under the new name, on both engines.

### 5.4 Column source additions (`table_columns.rs`)

- `top_level_fields(schema: &Value) -> Vec<(i64, String)>`, the field-id version of `top_level_columns` (which becomes
  a map over it; behaviour unchanged).
- `current_fields_via(conn, warehouse_id, schema, table) -> Result<Option<TableFields { schema, table, fields }>>`,
  the same SQL as `current_columns_via` (`TC:76`) returning the raw schema's fields. Only an active table counts.
- `PendingColumns` gains `leaving: Vec<String>` and `leaving_table: bool` (`#[serde(default)]`), so #755's overlays
  stay valid.
- `table_states_via(conn, &[TableKey]) -> Vec<TableState>` for the report (§5.8).

The ceilings (#755 C9: per-clearance count, profile total, rule row-filter tables, rule mask columns) skip a policy
whose table is `leaving_table` or whose column is in `leaving` (C8). Only the hook sets the overlay, with `SET LOCAL`.

### 5.5 The wait (`egret-policy`)

- `plugins_hold_policy_version(store, service_id, version, recent) -> HoldState`: `PD:136`'s query with
  `kind = 'policies'`.
- `wait_for_versions(store, tags: &[(i64, i64)], policies: &[(i64, i64)], deadline) -> Result<(), WaitError>`: one
  500 ms loop over both lists; `waiting_on` entries are the plugin ids (prefixed `tags:`/`policies:` only when one id
  is behind on both); an empty pair list is skipped, not a timeout. `wait_for_tag_version` becomes
  `wait_for_versions(store, required, &[], deadline)` and keeps its "empty is a timeout" contract for the view hold
  (which refuses an empty set itself before calling, `VH:1157-1161`).

### 5.6 Drops

- **Table drop:** unchanged (R2). Rows stay dormant; #755 still reaps StarRocks masks; undrop restores the name and
  `TableHook::undrop` recompiles (E11).
- **Plain namespace drop:** nothing to do. Lakekeeper refuses it while any tabular, live or soft-deleted, is in the
  namespace (E35), so it never removes a dropped table (R9).
- **Recursive namespace drop** (`DELETE …/namespaces/{ns}?recursive=true`, C13): `classify` reads the query
  (`Target::DropNamespace { ns, recursive }`). #755's path runs as today (its locks, its after-commit recompile). After
  the 2xx, one transaction: `lock_view_tags_in_tx`; the views in the namespace (`view_lineage` keyed by view schema)
  lose their lineage; `ViewReach` over the namespace's tables (views elsewhere that read them); delete every row with
  `lower(schema_name) = lower(ns)`; retag, bump, recompile; record `tag:purge` with the counts. Failure: logged, rows
  stay. A multi-level namespace carries no row and is skipped.

### 5.7 Governance events

`egret_audit::governance`:

- `action::CATALOG_TAG_MOVE = "tag:move"`, `action::CATALOG_TAG_PURGE = "tag:purge"`, both on `resource::CATALOG =
  "catalog"`, `Family::Tag`, in `FAMILIES` and `GOVERNANCE_ACTIONS`.
- `Actor` = `User(&Principal)` | `CatalogClient { id, client_id, name, consumer_type }` | `Egret`, and
  `record_change_by(conn, actor, action, resource_type, resource_id, summary)`: a user goes to `actor_user_id` (as
  `record_change`, which becomes `record_change_by(User(..))`); a catalog client sets `actor_user_id` NULL and
  `detail.actor` (C6a); `Egret` sets neither.

**Where the caller comes from (R7).** `CatalogGuard::check` returns `Result<CatalogCaller, (StatusCode, String)>`
instead of `Result<(), …>`, where `CatalogCaller { subject: String, audience: String }` is read from the claims the
guard already verified (`claims["sub"]`, `claims["aud"]`, `crates/egret-bin/src/main.rs:530-545`).
`guard_then_inject_metadata` (`mount.rs:334-353`) inserts it into the request's extensions beside the
`RequestMetadata`. The hook reads `req.extensions().get::<CatalogCaller>()` before it spawns its uncancellable task
and carries it to `finish`, which resolves it in the same transaction: `aud = "catalog"` → `SELECT id, client_id,
name, consumer_type FROM catalog.catalog_clients WHERE client_id = $1` → `Actor::CatalogClient` (a client deleted
since keeps its `client_id` with `id`/`name` null); a subject that is a user id of `users` (a future user-bearing
catalog token) → `Actor::User`; no `CatalogCaller` (no guard, tests) → `Actor::Egret`. The recursive namespace drop
does the same. An engine's catalog client names the engine, not the human whose query renamed the table: Trino and
StarRocks reach the catalog with their own client (E33).

`tag:move`: `resource_id` `"{warehouse_id}:{src_schema}.{src_table}"`; summary
`{"before": {schema, table, column|null}, "after": {schema, table, column|null}, "tags": [{id, name}], "rows": n,
"kept": [rows already on the destination], "trigger": "iceberg_rename" | "iceberg_column_rename"}`, one event per
moved coordinate. `tag:purge`: `resource_id` `"{warehouse_id}:{ns}"`, summary `{"schema": ns, "rows": n, "views": m,
"trigger": "iceberg_drop_namespace"}`.

`governance_changes.rs` sentences, with the actor phrase extended to `detail.actor` ("Catalog client
*trino-main*", or "Catalog client `cid-…` (deleted)"): "Catalog client *trino-main* moved classification *PII* from
`sales.orders` to `sales.orders_v2` (table renamed)"; "… from `sales.orders.email` to `sales.orders.mail` (column
renamed)"; "Catalog client *spark-etl* removed 12 classification assignments with namespace `sales` (namespace
dropped)". More than one tag: "classifications *PII*
and *Finance*"; over three: "4 classifications".

### 5.8 Stale-row report

`table_states_via` (C14), batched over every distinct folded `(warehouse_id, schema, table)` of
`catalog_tag_assignments` whose warehouse has a Lakekeeper warehouse:

```sql
SELECT k.ord,
       bool_or(t.deleted_at IS NULL AND t.metadata_location IS NOT NULL) AS live,
       bool_or(t.deleted_at IS NOT NULL)                                 AS dropped
  FROM unnest($1::uuid[], $2::text[], $3::text[]) WITH ORDINALITY AS k(warehouse_id, schema, name, ord)
  JOIN catalog.warehouses w ON w.id = k.warehouse_id
  LEFT JOIN lakekeeper.tabular t
    ON t.warehouse_id::text = w.lakekeeper_warehouse_id
   AND cardinality(t.tabular_namespace_name) = 1
   AND lower(t.tabular_namespace_name[1]) = lower(k.schema) AND lower(t.name) = lower(k.name)
 GROUP BY k.ord ORDER BY k.ord
```

`live` → `Live`; else `dropped` → `Dropped`; else `Missing`. A view counts as live (its rows are the view hold's). The
exact column names and casts are a sketch; the plan's report task pins them with the drift test, as #755's Task 3
did.

- **Overview** (`GO`), two sources, `requires: Requires::All(&[TAG_READ])`:
  - `classifications_on_missing_tables`, warning: "3 tables carry classifications but do not exist in their catalog:
    renamed before egret moved classifications, purged, or classified before they were created." Sample: label
    `catalog.schema.table`, detail the classification names, `ref: Tag {id, name}` of the first classification.
    Finding ref `Tags`.
  - `classifications_on_dropped_tables`, info: "2 dropped tables keep their classifications, which apply again if
    they are restored." Same samples.
  - `classifications_on_missing_columns`, warning (R8): "4 classified columns are not in their table: 3 were
    renamed (egret names the new column), 1 was dropped or never existed." Sample: label
    `catalog.schema.table.column`, detail "renamed to `mail`" or "not in the table", `ref: Tag`. Finding ref `Tags`.
  - Frontend: three `CHECKS` entries ("classifications on tables that do not exist", "classifications on dropped
    tables", "classifications on columns that do not exist").
- **Missing columns (R8):** `table_columns::missing_columns_via(conn, &[(TableKey, Vec<column>)]) ->
  Vec<MissingColumn { key, column, renamed_to: Option<String> }>`, for the column-level rows of tables whose state is
  `Live`, in one query:
  1. the table's current top-level fields `(id, name)`;
  2. a row's column is **missing** when no current field folds equal to it;
  3. its **candidate** is the field id that carried that (folded) name in the most recent older schema of the same
     table that had it (`lakekeeper.table_schema`, highest `schema_id` below the current one, E36); if that id is in
     the current schema under another name, `renamed_to` is that name, else `None` (dropped, or the name never
     existed). A view is not a table and has no missing columns.
  The candidate is a suggestion, never applied: a steward re-attaches through the normal write path. Caveat: a name
  dropped and later re-added for a new field resolves to the newest field that carried it, which may not be the
  field the row was written for; the sample's wording ("renamed to") is therefore a candidate, and the steward
  confirms it.
- **Where used:** `TableUsage.state: Option<TableState>` (`live` | `dropped` | `missing`, absent when the warehouse
  is not an embedded catalog) and `TableUsage.missing_columns: Vec<MissingColumn { column, renamed_to }>` (empty
  unless live); the Where used tab shows a `StatusBadge` "Dropped" or "Not in the catalog" beside the table name, and
  beside a missing column "Not in the table" or "Renamed to `mail`" (FE-VISUAL-8, FE-A11Y-4: text, never colour
  alone), nothing for live tables and present columns.

### 5.9 Interaction with #755, summarised

- A rename or column rename **with** a move plan: the hold replaces #755's dry run (C7); #755's after-commit StarRocks
  recompile is the hold's `finish` (it recompiles every bound profile, a superset); #755's locks are taken over a
  superset of its profiles.
- **Without** a plan (source carries nothing, e.g. a rename onto a name with dormant whole-table rows): #755's path is
  unchanged, including its dry run and its 409 (the existing test that renames `small` onto a classified `target`
  stays green).
- `TableHook::undrop`: unchanged.
- The hook's module doc loses "does not move a table's classifications" (`TH:66-69`) and gains the move.

## 6. Principles

- A classification follows its table and its columns; the renamed table is protected from its first read on both
  engines.
- Write, wait, then commit; undo exactly what was written when the commit does not happen; never under-tag.
- What can come back (a dropped table) keeps its rows; what cannot (a recursive namespace drop) loses them.
- Say what is stale; never repair by guess.

## 7. Backend changes (enumerated)

1. `egret-policy/src/plugin_downloads.rs`: `plugins_hold_policy_version`, `wait_for_versions`;
   `wait_for_tag_version` on top of it.
2. `egret-perm-compile`:
   - `table_columns.rs`: `top_level_fields`, `current_fields_via`, `PendingColumns.leaving{,_table}`,
     `table_states_via`, `TableState`, `missing_columns_via`, `MissingColumn`;
   - `clearance.rs`, `rules.rs`, `trigger.rs`: ceilings skip leaving coordinates;
   - `view_tags.rs`: `ViewReach` (moved from `egret-bi`, public), `Cargo.toml` gains `egret-catalog-registry`.
3. `egret-bi/src/catalog_tags.rs`: uses `egret_perm_compile::view_tags::ViewReach`; behaviour unchanged.
4. `egret-audit/src/governance.rs`: two actions, `resource::CATALOG`, `Actor`, `record_change_by`.
4a. `egret-catalog-engine/src/mount.rs`: `CatalogCaller`; `CatalogGuard::check` returns it;
    `guard_then_inject_metadata` puts it in the extensions. `egret-bin/src/main.rs`: `EgretCatalogGuard::check`
    returns it from the verified claims.
5. `egret-catalog-engine`:
   - `table_hook.rs`: move plans, `hold_move`, copy, wait, finish, compensate, namespace purge, `recursive` in
     `classify`; `TableHook` gains `policy: PolicyStore`, `limit`, a hold semaphore, `with_limit`;
   - `view_hold.rs`: `uncancellable`, `trino_tag_versions`, `delete_inserted_rows` made `pub(crate)`.
6. `egret-controlplane`: `governance_overview.rs` three sources; `governance_tag_usage.rs` `TableUsage.state`,
   `TableUsage.missing_columns`; `governance_changes.rs` two sentences and the `detail.actor` phrase.
7. `egret-bin/src/main.rs`: `TableHook::new` unchanged in signature (it builds its own `PolicyStore`, as `ViewHold`
   does).
8. `openapi.json`, `frontend/src/api/generated/**` regenerated (`npm run gen:api`).
9. Frontend: `AttentionSection.tsx` `CHECKS`; `WhereUsedTab.tsx` badge.

## 8. Wire shapes

Iceberg REST refusals keep the hook's shape (`TH:304-318`):

```json
{"error": {"message": "egret refused the table change: the table's classifications cannot move to sales.orders_v2: compiling the clearance on tag PII would expand to 734 policies on StarRocks, over the ceiling 500: …; resolve it, then retry",
           "type": "CommitFailedException", "code": 409}}
```

Added to `TableUsage` (control plane):

```json
{"catalogId": "…", "catalogName": "lake", "warehouseId": "…", "schema": "sales", "table": "orders",
 "onTable": true, "columns": ["email", "region"], "state": "live",
 "missingColumns": [{"column": "email", "renamedTo": "mail"}]}
```

`state` is `skip_serializing_if = "Option::is_none"`, `missingColumns` `skip_serializing_if = "Vec::is_empty"`;
schemas `TableState` = `"live" | "dropped" | "missing"` and `MissingColumn` = `{column: string, renamedTo: string |
null}`. Overview findings use the existing `Finding` shape with the three new `kind` values. Governance feed entries
use the existing `ChangeEntry` with the two new actions; the `tag:move` event's stored `detail`:

```json
{"before": {"schema": "sales", "table": "orders", "column": null},
 "after": {"schema": "sales", "table": "orders_v2", "column": null},
 "tags": [{"id": "…", "name": "PII"}], "rows": 2, "kept": [], "trigger": "iceberg_rename",
 "actor": {"kind": "catalog_client", "id": "…", "client_id": "trino-main-7f3a", "name": "trino-main",
           "consumer_type": "engine"}}
```

## 9. Migration and compatibility

- No database migration (C19).
- Behaviour on upgrade: renames of tables or columns that carry classifications are held for up to two plugin polls
  (typically 30-60 s, at most 90 s) and may be refused with a 409 the client prints. Unclassified renames, appends
  and every other request are unchanged.
- Rows left by renames before the upgrade are not moved: they show in the two Overview findings and in Where used.
  A steward re-attaches the classification to the new name and removes it from the old one (both through the normal
  write path, which bumps, recompiles and checks overlaps). No migration rewrites rows: a migration cannot bump or
  recompile (0177's reasoning).
- Wire: two optional fields and two schemas added; no removal. `CatalogGuard` is an in-repo trait; its one production
  implementation and the test guards change together.
- Release note: classified renames are now held; the Iceberg client must tolerate a 90 s request (Trino does, E17);
  check the two new Overview findings after upgrade.

## 10. Risks

- **Client timeouts.** Trino's REST client was measured to tolerate 90 s (E17). Spark, PyIceberg and StarRocks
  clients issuing renames are unmeasured; a client that times out and retries gets either the 409 "being held" or,
  after the first completed, Lakekeeper's "no such table".
- **A bound but silent engine refuses classified renames** for the whole deadline (R5). Loud by design.
- **Concurrent permission writes during the hold.** A classification, rule or grant write on a bound profile while a
  move waits recompiles without the hold's overlay: on StarRocks the destination's per-column masks drop out of the
  stored compile until `finish`, so a plugin that polls in that interval and a reader of a view over the renamed
  table in the moment between the rename and the next poll can see real values (the `*` deny still refuses direct
  reads). Same class as #755 §11 item 6; not mitigated.
- **Concurrent schema changes wait.** A schema change of any table bound to the same profiles waits for the hold's
  profile locks up to 30 s, then gets a 409 "retry" (C7).
- **StarRocks ledger dependence.** C5 relies on StarRocks reporting `lastKnownVersion` on its policy polls (E24,
  measured on plugin 2.8 in the 2026-10-20 live run); a plugin build that stops doing so makes every classified
  rename on that warehouse time out (loud). The live check re-measures it.
- **Union surprises** (C1): a table renamed onto a dormant name inherits that name's classifications. Fail-closed;
  shown in the event's `kept`.
- **Lakekeeper coupling** grows by two queries (`current_fields_via`, `table_states_via`); the drift test covers both.

## 11. Related defects (found, not fixed here)

1. **Path-selector grants and rules stay on the old name** (`perm.grants`, `perm.rules`; E31). An allow is lost
   (fail-closed); a **deny is escaped and a path mask or row filter stops applying (a leak)**; a new table at the old
   name inherits them. KI row; a follow-up package can reuse this hold.
2. **Hand-written Ranger policies** (`policy.authz_policies`) naming the old table: same effect as item 1. KI row.
3. **Embedded-catalog RBAC** `catalog.catalog_grants` (`namespace[]`, `table_name`): a client loses access to the
   renamed table; a new table at the old name gains it. KI row.
4. **View lineage and BI references** (`view_lineage` base names, datasets, Data API SQL, charts) keep the old name:
   they break (not a leak). Out of scope.
5. **Warehouse delete** leaves `catalog_tag_assignments` orphaned (no FK); "Where used" counts them as `orphaned`.
   Unchanged.
6. **#755's schema-growing commits are not held (KI-305, stays open, R6)**: an `ADD COLUMN` on a
   whole-table-classified table gets its StarRocks NULL mask at the next policy poll, so a view over the table reads
   the new column's real value for up to 30 s (the `*` deny refuses direct reads). A later fix can reuse this
   package's `plugins_hold_policy_version`/`wait_for_versions` (public in `egret-policy`, no move-specific argument)
   and the hold's copy-free path (a plan with no rows still waits on StarRocks, C12).
7. **Register with overwrite** registers a different table under a classified name (E4); the rows apply to it.
   Unchanged (fail-closed).
8. **The event actor of an engine-initiated rename is the engine's catalog client**, not the person whose query
   renamed the table (E33, R7). Naming the person needs the engine to pass a user-bearing catalog token.

## 12. Non-goals

- Re-keying assignments by table uuid or field id.
- Moving any state other than classification rows (R4).
- An automatic repair of stale rows (a later, steward-confirmed action through `assign_catalog`/`unassign_catalog`).
- Holding schema-growing commits that rename nothing (§11 item 6, KI-305, R6); the policy-version wait is built to be
  reused for it.
- A purge for plain namespace drops (it cannot remove a namespace with dropped tables, R9).
- Platform docs in this PR (C20).

## 13. Open questions

None. The first draft's five were answered on 2026-10-26 (R5–R9). One alternative stays on record for C6a: a
`actor_client_id` column on `audit.audit_log` (a migration) instead of `detail.actor`, if the feed ever needs to
filter by catalog client.

## 14. Implementation notes (append-only)

Deviations the code forces are recorded here, per task, as they happen.

### Spec revision (2026-10-26, before implementation)

- The user answered §13's five questions (R5–R9). Changes: C4 lost its silent-engine half (now R5); C6 points to R7
  and C6a stores the caller; C13 and §5.6 record that a plain namespace drop cannot remove dropped tables (E35, R9);
  C15 and §5.8 add the missing-columns finding and `missingColumns` (R8); §11 item 6 is KI-305 with the wait marked
  reusable (R6); new evidence E33–E36; §13 emptied.

### Step 0: evidence re-checked (2026-10-26)

- `origin/main` is still `28fb8693`; no rebase. Every Step 0 row holds as written (E1, E7/E8, E9, E10, E16, E19,
  E20, E23/E24, E27, E33, E35 at the pinned rev `7916505`, the inverted test). E19's wording is exact only for
  `[dependencies]`: `egret-bi` IS a dev-dependency of `egret-catalog-engine` (its race tests drive `CatalogTagRepo`).
  E32: highest migration still `0209`; `gh pr list` could not be re-run (GitHub TLS timeout from this machine), so
  open PRs' migrations are unchecked; no migration is added (C19), so nothing depends on it.

### Task 1: the policy-version wait

- As designed. `plugins_hold_tag_version` and `plugins_hold_policy_version` share one query with the kind bound as a
  parameter. `wait_for_tag_version(&[])` now returns its `Timeout` at once instead of after the deadline; the view
  hold refuses an empty set before calling it, so no caller sees the difference. `WaitError::Timeout`'s message says
  "the version" rather than "the tag version".

### Task 2: field ids, and a moving coordinate counted once

- `Leaving` is keyed by folded catalog NAME, not warehouse id: a compiled policy (and a `TaggedColumn`) names its
  catalog only, so `leaving_via` resolves the overlay's warehouses through `registry.catalogs` (globally unique names).
- `compile_rule`'s public signature is unchanged (46 call sites); it delegates to a crate-private
  `compile_rule_counting` that takes the leaving set. The profile total subtracts the profile's desired policies on a
  leaving coordinate (read from each body's resources), computed in `compile_desired` and passed to
  `apply_diff_in_tx` (which now carries `#[allow(clippy::too_many_arguments)]`).
- `current_fields_via` and `current_columns_batch_via` share one private query (`stored_current_schemas`); the
  overlay is applied only by the latter. The drift test reads field ids from Lakekeeper's Iceberg REST load response
  (`load_table_detail` carries no ids).
- One test beyond the plan pins the profile-total site (`a_moving_table_is_counted_once_against_the_profile_total`);
  the plan's two tests reach only the per-clearance ceiling.

### Task 3: the table rename moves its classifications

- `ViewReach`'s errors are a typed `ViewReachError { Db, Compile, ViewChanged }` instead of `CompileError`: a
  database error, a compile refusal and "a view changed while this tag write ran" each map in `egret-bi` exactly as
  before (500, the compiler's refusal, 409); folding them into `CompileError` would have changed two of the three
  responses. `ViewReach` gained `of_tables` (a column move in a multi-table commit reaches several tables of one
  warehouse; one lock pass, ascending). `egret-bi`'s private bump and recompile helpers moved with it; `cargo test -p
  egret-bi` passes 222/222 before and after.
- The copy always runs its compile and the after-2xx/compensation transactions always re-tag, bump and recompile,
  also when no row was inserted or removed: the copy's compile may change StarRocks masks through the overlay alone,
  and compensation must undo that. §5.3 step 5.2's "nothing to insert → skip to step 7 with no wait" is therefore not
  a separate path: with nothing inserted the Trino pair list is empty and nothing is waited on for Trino.
- Lakekeeper answers a rename onto an existing table `404 NoSuchTableException` ("Error getting tabular from
  catalog"), not 409; `a_failed_rename_undoes_the_copy` asserts a non-2xx carrying Lakekeeper's own body.
- Found, not fixed: `view_hold::lock_key` (shared by the view hold and now the move, C17) does not fold case, so a
  hold on `Orders` and one on `orders` do not exclude each other, although Lakekeeper's names are case-insensitive.

### Task 4: StarRocks wait, no Trino, silent engines

- As designed: the copy reads the StarRocks `(profile, policy_version)` pairs after its recompile, always (the
  overlay alone can change StarRocks policies); Trino pairs only when rows were inserted; an empty list is skipped.
- Test harness: `the_rename_waits_for_the_starrocks_policy_version` runs the fake StarRocks plugin five times slower
  than the Trino one. At equal cadence it passed on the Task 3 code too (StarRocks happened to report within the
  Trino wait), so it could not tell the wait apart from its absence. The Task 3 tests now start both fake plugins: a
  bound StarRocks profile whose plugin never polls refuses the move (R5).

### Task 5: the rename defaults

- C2 and C10 were red on the Task 4 code and are implemented here. C1 (union), C3 (cross-namespace) and C18 (folded
  match, destination spelling) were already green: the set formulation (`Target`, `Before`) and the folded row
  selection came with Task 3, because the move cannot be written without them. Their tests, and the guards, were
  mutation-checked instead (compensation deleting `Target`, an exact-match row read, no name lock, no profile lock,
  no live-table check, no empty-source check: each turns its test red).
- `a_case_only_rename_is_not_held` asserts the Trino tag version and the rows unchanged, not every version: a
  case-only rename of a whole-table-classified table still takes #755's path, whose after-commit recompile respells
  the StarRocks policies as the catalog now spells the table, so the StarRocks `policy_version` moves. That is #755's
  behaviour, not a move.
- A rename refused by C2 or a check error is answered before any lock; `move_plan_for_rename` returns the refusal.
