# Grants, rules and catalog grants follow a table rename: design

Status: design, 2026-10-27. Not implemented. Base: `main` `4807c3e3` (#757). Branch `fix/grants-follow-table-rename`.
Plan: [`../plans/2026-10-27-grants-follow-table-rename.md`](../plans/2026-10-27-grants-follow-table-rename.md).
Programme: Control Plane & DG, governance defects (docs KI-308, KI-309).
Parent: [`2026-10-26-classification-follows-table-rename-design.md`](2026-10-26-classification-follows-table-rename-design.md)
("the #757 spec"), §11 items 1-3, which this package fixes. Its hold, its move plan, its waits, its events and its
stale report are the mechanism extended here; read its §3-§5 first.

**Prerequisite, merged first:** the separate PR on `fix/catalog-rename-authorizes-source` (finding F-A: the catalog
guard authorizes `POST …/tables/rename` as a catalog-root operation, so it never checks the source and destination
tables, E14). It is not designed here. Without it, moving catalog grants does not stop the escalation F-A describes:
the attacker's grant names the destination.

The user settled four questions on 2026-10-27 (§3, G1-G4), then answered the first draft's two open questions the
same day (G5, G6). The controller set the other defaults (§4, D1-D21); the user may revisit those.

## 1. Problem

Four kinds of state name a catalog table by its name, not by its identity, and none of them follows a rename:

- **`perm.grants`** (allow and deny, table- or column-level) and **path-selected `perm.rules`** (masks and row
  filters) store `(catalog_id, schema_name, table_name, column_name)` (E1, E2).
  - After `sales.orders → sales.orders_v2`, a deny grant, a path mask and a row filter no longer apply to the
    renamed table. That is a leak on both engines from the first poll after the commit.
  - An allow stops applying, which fails closed.
  - A new table created later at the old name inherits all of them, which is the swap pattern's problem
    (`t → t_old`, `t_new → t`).
- **Column-level grants and path masks** have the same defect for a column rename: a mask on `email` stops
  applying when `email` becomes `mail` (E1, E2).
- **Hand-written Ranger policies** (`policy.authz_policies`) name tables as literals or globs in raw Ranger JSON,
  including exclusion lists (E8).
- **`catalog.catalog_grants`** (embedded-catalog RBAC, KI-309) store `namespace text[]` and `table_name`. A client
  holding a table grant loses the renamed table, and a new table at the old name gains it (E10, E13).

#757 moved classifications only (its R4). Its hold runs only for classified tables (E22), and it waits for the Trino
tag version and the StarRocks policy version, never for the Trino policy version (E25). A moved grant or rule
changes exactly that Trino version.

## 2. Evidence (verified at `4807c3e3`)

Paths are relative to the repo root. `TH` = `crates/egret-catalog-engine/src/table_hook.rs`, `PS` =
`crates/egret-perm/src/store.rs`, `PR` = `crates/egret-perm-compile/src/routes.rs`, `TR` =
`crates/egret-perm-compile/src/trigger.rs`, `TC` = `crates/egret-perm-compile/src/table_columns.rs`, `VT` =
`crates/egret-perm-compile/src/view_tags.rs`, `PD` = `crates/egret-policy/src/plugin_downloads.rs`, `MAIN` =
`crates/egret-bin/src/main.rs`, `EMB` = `crates/egret-catalog-engine/src/store/embedded.rs`, `GO` =
`crates/egret-controlplane/src/governance_overview.rs`, `GV` = `crates/egret-audit/src/governance.rs`. The
investigation that found these is summarised in the package brief (findings E1-E30, D1-D10); the rows below are the
ones this design relies on, re-checked.

### 2.1 Storage

| id | fact | where |
|---|---|---|
| E1 | `perm.grants`: `effect allow/deny`, `actions ⊆ {read, write}`, `catalog_id` (FK, cascade), nullable `schema_name`/`table_name`/`column_name` (no level may be skipped), `origin explicit/workspace_assignment`. Principals live in `perm.grant_principals` (cascade). No version, `updated_at` or owner column | `migrations/0154_perm_model.sql:13-52`; later ALTERs only add CHECKs (`0178:137,157`, `0179:167`) |
| E2 | `perm.rules`: `name text NOT NULL UNIQUE`, `kind mask/row_filter`, `selector_kind tag/path`, the same path columns, exemptions in `perm.rule_exemptions` (cascade). No version or owner | `migrations/0154_perm_model.sql:54-104` |
| E3 | A level is matched case-folded, then as a Ranger glob (`*`, `?`): `resource_level_matches` is public. A stored `ord*` is legal and enforced as a prefix set | `crates/egret-perm/src/eval.rs:322-370` |
| E4 | The compiler copies each named level verbatim into the Ranger resource; a row filter names catalog, schema and table only; both service-defs declare `ignoreCase` and `wildCard` | `crates/egret-perm-compile/src/grants.rs:315-380`; `crates/egret-policy/src/service-defs/{trino,starrocks}.json` |
| E5 | A grant or path rule reaches the profiles bound to its catalog. A catalog has at most one embedded warehouse, so these are `services_bound_to_warehouse`, the set #757 locks and recompiles | `TR:1729-1739`, `TR:1774-1786`; `crates/egret-catalog-registry/src/tag_version.rs:141-158` |
| E6 | `workspace_assignment` grants are always catalog-wide (the INSERT names no schema) | `crates/egret-perm/src/assignment.rs:102-106` |
| E7 | `update_grant_in_tx` / `update_rule_in_tx` UPDATE in place and keep the id (the compiled policy is `perm-grant-{id}`, explain deep-links by id); principals and exemptions are replaced. `create_grant_in_tx` / `create_rule_in_tx` insert with principals | `PS:366-503`, `PS:566-600` |
| E8 | Hand-written policies: `policy.authz_policies(id, service_id, name, policy_jsonb, enabled, version, updated_at)`, `managed_by ∈ {compiler, infra, operator}`; resources are Ranger values lists with `isExcludes` | `migrations/0007_authz.sql:18-27`; `migrations/0180_policy_managed_by.sql:58-61` |
| E9 | Policy migration stores content hashes of the grants and rules it created (`grant_hash`, `rule_hash`); undo re-hashes and refuses when one changed | `crates/egret-perm-compile/src/migrate.rs:718-735`, `:1099-1128` |
| E10 | `catalog.catalog_grants(id, catalog_role_id, securable_type ∈ {catalog, namespace, table}, namespace text[], table_name, privilege)`; add checks duplicates in the application only (no unique index); there is no update; admin routes need `catalog_registry:*` and record no event | `migrations/0053_embedded_catalog_rbac.sql:21-28`; `EMB:405-440`; `crates/egret-catalog-engine/src/warehouse_admin/mod.rs:292-293`, `:743` |

### 2.2 Enforcement

| id | fact | where |
|---|---|---|
| E11 | The catalog guard reads the role's catalog grants from the database on every request; no cache, so no plugin wait applies to them | `MAIN:605-612`; `EMB:458-474` |
| E12 | `catalog_grant_covers` compares the namespace array and table name exactly: no globs, case-sensitive; a `table` grant also covers `views/{name}` | `MAIN:618-625`, `MAIN:666-698` |
| E13 | Engines' catalog clients hold catalog-wide `manage`; table-level catalog grants matter to notebook and external clients | `crates/egret-engine-mgmt/src/engines.rs:2759-2790` |
| E14 | (F-A) `target_ns` is set only when `segs[1] == "namespaces"`; a rename (`segs[1] == "tables"`) passes with any write/manage grant | `MAIN:618-640` |

### 2.3 The #757 hold

| id | fact | where |
|---|---|---|
| E15 | A rename move plan exists only when the source carries classification rows (`rows.is_empty() → None`); a column plan only for classified renamed fields | `TH:1018-1120` (`:1048`), `TH:1129-1280` (`:1225-1230`) |
| E16 | `hold_move`: semaphore → name try-locks → profile locks over every bound profile → uncancellable `run_move` (copy, wait, forward, `committed` check, compensate or finish) | `TH:1284-1418` |
| E17 | `copy` reads the Trino tag versions only when rows were inserted and the StarRocks policy versions always; **no Trino policy version** | `TH:1549-1563`; `TH:984-1000`; `crates/egret-catalog-engine/src/view_hold.rs:745-761` |
| E18 | `wait_for_versions` checks every pair in one pass and returns only when **all are confirmed in the same pass**; a pair confirmed earlier is re-checked every 500 ms | `PD:219-246`, `PD:153-187` |
| E19 | `finish` deletes `Before \ Target`, re-tags, recompiles every reached profile and records `tag:move`; `compensate` deletes exactly the inserted rows | `TH:1596-1696`; `TH:1889` |
| E20 | Catalog-tag writes try the folded name lock on their own transaction and answer 409 `table_change_held` while a hold holds the name | `VT:77-112` |
| E21 | The copy's compile skips, for the profile total, every desired body whose coordinates are all leaving (`Leaving::counts_body`), whatever produced the body | `TR:681-685`; `TC:105-140` |
| E22 | `table_states_via` answers live/dropped/missing for **single-level** namespaces only (`cardinality = 1`) | `TC:345-392` |
| E23 | Overview sources receive a connection only (`SourceFn`); a source cannot filter its samples by the reader's permissions. #757's `renamed_names_reused` is gated on `tag:read` | `GO:183-184`, `GO:313-334`, `GO:1577` |
| E24 | Families gate the feed: `Policy` = `policy:read`, `Tag` = `tag:read`, `User` = `user:read` | `GV:67-73`, `GV:81`; `crates/egret-controlplane/src/governance_changes.rs:143-144` |

### 2.4 Trino reports its policy version (the wait's premise)

| id | fact | where |
|---|---|---|
| E25 | egret records every policy poll with the plugin's `lastKnownVersion` (`DownloadKind::Policies`), for any engine type | `crates/egret-policy/src/ranger/api/download.rs:120-137` |
| E26 | **Measured:** Trino 483's plugin polls `GET /service/plugins/policies/download/{profile}?…&pluginId=trino@<host>-<profile>&…&lastKnownVersion=N`, and the next poll's `lastKnownVersion` proves activation | `docs/en/plans/2026-10-13-view-governance-live-result.md:231-254` |
| E27 | **Measured:** policy confirmation takes two polls on both engines (8.7 s for Trino at a 5 s poll), and **Trino's `tags` row regresses to `-1` on the first tag poll after every policy download** (the tag retriever restarts with the policy engine) | `docs/en/plans/2026-10-18-dg-overview-live.md:54-66` |
| E28 | The plugin-health judgement already reads the Trino `policies` kind from the same ledger | `crates/egret-controlplane/src/plugin_judgement.rs:109-170` |

The plugin is a Java jar, so "Trino reports `lastKnownVersion` on policy polls" cannot be proved from this
repository's code. It was measured twice on Trino 483 (E26, E27) and the ledger reads it today (E28). No Step 0
measurement is needed; the live check re-measures it (plan Task 19), and §10 states the fallback.

## 3. User decisions (2026-10-27, not reopened)

- **G1. Scope.** One package covering table-level `perm.grants` and path-selected `perm.rules`; column-level grants
  and rules, following #757's field-id column pairing; `catalog.catalog_grants` (KI-309); the hand-written policy
  check; the Trino policy-version wait the hold needs; a stale-reference report; recursive namespace drop handling.
  F-A is separate and comes first. View renames (F-C) are the controller's call (D19).
- **G2. Restrictive rows stay; allows move.** Allow grants move to the new name **after** the commit. Deny grants,
  mask rules and row-filter rules are **copied** to the new name **before** the commit, and the old-name copies
  **stay** (dormant) after it. A new table created at the old name, including the swap pattern, is therefore still
  restricted. This deliberately differs from classifications' R1 (which move). The report lets a steward clean up
  dormant copies.
- **G3. Hand-written Ranger policies.** If an enabled one names the source table literally with a deny, a mask, a
  row filter or an allow with an exclusion list (`isExcludes`), the rename is refused with 409 naming those
  policies, so their author fixes them first. Plain literal allows are only reported. egret never rewrites
  user-authored policy JSON.
- **G4. Patterns.** If a grant or rule pattern (`ord*`, `?`, `*`, or a schema-level scope) matched the old name but
  does not match the new one, a deny, mask or row filter makes the rename 409 and names the rows; an allow is left
  alone and reported. Patterns are never split or rewritten.

Answers to the first draft's open questions (2026-10-27):

- **G5. A narrow exception to G4: a covered destination.** A restrictive pattern row that matched the old name but
  not the new one does **not** refuse the rename when the new name is already covered by a restriction of the same
  kind and equal effect, whether that restriction is an exact-name row or another pattern. A partial cover (for
  example, fewer subjects denied on the new name) still refuses, and the 409 says what is missing. §5.9 defines
  "same kind and equal effect" for each kind; D21 records that egret does not create the cover itself.
- **G6. The allow gap is accepted.** Allow grants move only after the commit (formerly the controller's D2). For up
  to about two policy polls after a rename, a holder of an allow on the table cannot read the renamed table. This is
  a documented Limitation (§9), not a defect.

## 4. Controller decisions (defaults; the user may revisit)

| # | Decision | Why |
|---|---|---|
| D1 | **Restrictive rows: insert a copy before the commit; never touch the original.** For each deny grant and path rule that follows (D3), the copy transaction INSERTs a new row on the destination (`create_grant_in_tx` / `create_rule_in_tx`, principals or exemptions copied). The original keeps its id, body and history on the old name, which is G2's end state. A rule copy is named `"<name> (<dst_schema>.<dst_table>)"` (`.<column>` for a column move), with ` #2`, ` #3` … on a `perm.rules.name` collision. Compensation deletes exactly the inserted ids; `finish` has nothing to do for them | G2 keeps the old row, so there is nothing to "move"; an insert-copy is the only write. Leaving the original untouched keeps a migrated row's hash (E9), so a migration stays undoable; compensation is exact (delete by id); a crash at any point leaves at worst an inert or protective copy on the destination (never a leak). The findings' alternative (update the original, shadow on the old name) needs a revert of a replaced principal set on compensation and changes a migrated row's hash, for no gain once the old row stays forever |
| D2 | **Allows: an in-place UPDATE after the 2xx, in `finish`; no copy before the commit** (the timing is now user decision G6; the mechanism stays a default). `update_grant_in_tx` re-points `schema_name`/`table_name` (or `column_name`) and keeps the id; the recompile diffs `perm-grant-{id}` in place. Nothing waits for it | G2 says "after the commit". A pre-commit copy on the destination is inert only while nothing else creates that name; name locks do not cover a concurrent create (findings §3.4), and an allow copy would grant that stray table. In-place keeps the id (explain links, history) with no shadow row. The cost is fail-closed: holders of the allow lose the renamed table for up to two Trino policy polls, which G6 accepts as a Limitation (§9) |
| D3 | **What follows.** A row follows a table move when its schema and table levels are both **literal** (no `*` or `?`, the 0177 test) and fold equal to the source; its column level is carried verbatim. It follows a column move when schema, table and column are literal and fold equal to the old column. Only `origin = 'explicit'` grants (E6). Selection folds; the copy or update writes the request's destination spelling for the renamed level and keeps every other level's spelling (#757 C18) | The same literal test as classifications; a pattern at any level is a statement about names (G4), not about this object, so it never follows |
| D4 | **Pattern rows (G4).** A row *matched* a coordinate when its catalog is the warehouse's and every named level `resource_level_matches` the coordinate (an unnamed level matches anything, E3). A row that matched the source, does not follow (D3), and does not match the destination is a pattern row. Restrictive → the rename is refused (409) listing up to ten rows (kind, id or rule name, scope), what each one misses on the new name (§5.9), and the count, **unless the new name is fully covered** (G5, §5.9). Allow → left as it is and reported (D9's `perm_grant:left`, plus a WARN log). A row that newly matches the destination is the author's rule and is left alone. Applies to column renames for column-level rows | G4. The check runs at plan time and again under the locks in the copy transaction (a pattern write is not name-locked, D10) |
| D5 | **Hand-written policies (G3).** Considered: enabled, `managed_by = 'operator'` policies on the profiles bound to the warehouse. A policy *names the source literally* when its catalog level matches the warehouse's registry catalog name (glob, honouring `isExcludes`), its schema/database level matches the source schema (same), and its table level carries a value with no `*`/`?` that folds equal to the source table, in either polarity. Refused (409 naming `profile/policy name (id)`) when it is a mask (`policyType` 1), a row filter (2), has `denyPolicyItems` or `denyExceptions`, or has `isExcludes` on the table level. Otherwise (a plain literal allow) it is reported (`ranger_policy:left`, WARN). For a column rename the same test runs one level down (the table level matches, the column level names the old column literally). Glob-only hand-written policies are not checked (§11 item 3) | G3. Infra policies are all-wildcard and compiler policies are compiled from grants and rules, so only operator policies can carry an author's literal. The catalog name mapping is the compiler's (registry name, E4); Step 0 re-checks it |
| D6 | **Trino policy-version wait.** The copy transaction reads `(Trino profile, policy_version)` for the Trino profiles bound to the warehouse, after its recompile, **when it inserted any grant or rule copy**; the pairs join the StarRocks pairs in `wait_for_versions`' policy list. No Trino bound → no Trino pairs (#757 C4); a bound Trino whose plugins are silent → 409 at the deadline and the copy undone (#757 R5) | E25-E28: the plugin reports `lastKnownVersion` on policy polls (measured), so the wait is a sibling of #757's StarRocks wait, not new machinery. Only a copy changes a Trino body before the commit (allows move after it, D2) |
| D7 | **A confirmed pair stays confirmed for the rest of one wait.** `wait_for_versions` latches each pair the first time every recent plugin reports it | E27: Trino's tag row regresses to `-1` after each policy download. With both a Trino tag pair and a Trino policy pair (a classified table with a deny grant), the all-in-one-pass rule (E18) would wait for the regression to heal, about one more tag poll, which can cross the 90 s deadline. A version once reported was applied; the regression is the tag retriever restarting, which re-downloads at least that version. The view hold's single-list wait is unchanged in effect |
| D8 | **`catalog_grants` follow after the 2xx, with no hold and no wait.** For every rename whose source is a live table (#757 C9) and that is not case-only, after Lakekeeper's 2xx (or a non-2xx that `committed` shows landed), one transaction UPDATEs each `securable_type = 'table'` grant of the warehouse's roles whose namespace folds equal to the source namespace element by element and whose `table_name` folds equal to the source: `namespace` and `table_name` take the request's destination spelling. A moved grant that would duplicate one already on the destination (same role and privilege) is deleted instead (a union, #757 C1). `namespace` grants stay (they name a location). Multi-level namespaces are supported. Uncancellable; a failure is logged and leaves the grants on the old name | E11: enforcement reads the table per request, so there is nothing to wait for and nothing to hold. Case-only renames are skipped: catalog grants match the request path exactly (E12) and changing their spelling could cut off a client that spells the old way (F-B, §11 item 2) |
| D9 | **Events**, with #757's catalog-client actor (`Actor`, `record_change_by`): `perm_grant:move` (an allow re-pointed, in `finish`), `perm_grant:copy` and `perm_rule:copy` (a restrictive copy, recorded in `finish` so a refused or undone move records nothing), `perm_grant:left` and `ranger_policy:left` (an allow pattern or a plain literal hand-written allow that no longer covers the renamed table; nothing written), `catalog_grant:move`, and `catalog_grant:delete` (namespace purge). The purge of allow grants reuses `perm_grant:delete` with `detail.trigger = "iceberg_drop_namespace"`. Each carries `detail.trigger` (`iceberg_rename` / `iceberg_column_rename` / `iceberg_drop_namespace`) and `detail.rename = {from, to}`. Families: the perm and Ranger events `Policy`; the catalog-grant events a new family `Catalog`, read with `catalog_registry:read` | The brief proposed `perm_rule:move`: under G2 no rule ever moves (every path rule is restrictive), so that action would never be recorded; `perm_grant:copy` is added instead, so the feed tells a copy from a move. `*:left` is the only durable record that a pattern stopped covering a table, since Lakekeeper keeps no rename history (findings E30). Catalog grants are administered under `catalog_registry:*` (E10), so their events are gated the same way rather than shown to `policy:read` |
| D10 | **Grant and rule writes take the hold's name lock.** `write_grant`, `write_rule` and the two delete routes call `try_name_lock_in_tx` (E20) for each literal `(schema, table)` the row names before and after the write, mapped from `catalog_id` to its warehouse; busy → 409 `{"code": "table_change_held"}`. Pattern-scoped writes take no name lock. The hold's name locks are unchanged (they already cover source and destination) | A steward's PATCH opened before the rename and saved during it would otherwise re-point a moved allow back, or remove a copy compensation then cannot find. A pattern names no lockable name; D4's re-check under the locks catches a pattern written before the copy |
| D11 | **The hold widens, and only as far as needed.** A rename or column rename gets a move plan when the source carries classification rows (as today), **or** any following perm row (D3, any effect), **or** a refusal (D4, D5). A plan whose only work is allows takes the hold without a wait (cap, name locks, profile locks, the copy transaction as dry run, forward, `finish`). A rename whose only work is catalog grants takes no hold (D8) | Restrictive copies need the pre-commit write and the wait (G2). Allows need `finish` and its recompile under the name locks (D10), not a wait. A hold costs about two polls only when something restrictive was copied |
| D12 | **The copy counts dormant restrictive rows.** The profile-total subtraction (`Leaving::counts_body`, E21) skips only bodies the classification compile produced; a body compiled from a path grant or path rule (`perm.compiled.source_kind`) always counts | Under G2 the originals stay after `finish`, so the copy's compile must count what will be served afterwards; skipping them would let the copy pass and `finish` fail on the same ceiling |
| D13 | **Restrictive copies union.** A copy is skipped when an equivalent row already exists on the destination: same canonical body as `grant_hash` / `rule_hash` (E9) with the scope re-pointed, ignoring id and rule name | A rename onto a name holding dormant copies (the swap pattern run twice) must not stack duplicates; fail-closed either way |
| D14 | **Multi-level namespaces.** perm rows are single-level. A plan whose source carries a following restrictive row and whose destination is multi-level is refused (409, as #757 C2); following allows into a multi-level destination are left and reported (`perm_grant:left`). A multi-level source carries no following perm row. Catalog grants follow at any depth (D8) | No perm row can name a multi-level namespace, so a restriction could not follow; refusing is the restrictive rule, reporting the allow rule (G4's split) |
| D15 | **Recursive namespace drop.** After the 2xx, in #757's purge transaction: delete the explicit allow grants whose schema and table levels are literal and whose folded schema is the namespace (any table, any column), with `perm_grant:delete` events; then, in its own transaction, delete the `table` catalog grants whose `namespace[1]` folds equal to it (children included), with `catalog_grant:delete` events. Keep deny grants, path rules, schema-level and pattern scopes, `namespace` catalog grants and hand-written policies; the report shows the kept literal rows (D16) | A recursive drop cannot be undone (#757 E13). An allow on a dead name is exposure if the name returns; a restriction on it fails closed (G2's asymmetry). Location scopes belong to the namespace, which may be re-created |
| D16 | **Stale-reference report** (Overview, no new endpoint): `permissions_on_missing_tables` (warning, `policy:read`: literal grants, path rules and literal table values of enabled operator policies on bound profiles, whose table is missing), `permissions_on_missing_columns` (warning, `policy:read`: literal column-level grants and path rules of live tables whose column is missing, with #757's field-id candidate), `catalog_grants_on_missing_tables` (warning, `catalog_registry:read`: `table` catalog grants whose table is missing, multi-level included, through a new `table_states_ns_via`). A sample names the kind and, when a `*:copy`/`*:move` event names the table, "kept on purpose when the table was renamed to X" or "renamed to X". Dropped tables are not reported (an undrop restores them, as for classifications) | Reuses #757's primitives (E22). Three findings, not one, because the Overview cannot filter samples by permission (E23) |
| D17 | **"Name lost its grants to a rename"** (R10's note) is two sibling findings beside #757's `renamed_names_reused`: `renamed_names_lost_grants` (`policy:read`, from `perm_grant:move`) and `renamed_names_lost_catalog_grants` (`catalog_registry:read`, from `catalog_grant:move`), each kept when the old name is a live table again and carries none of the moved allows | The brief asked for a note on R10's finding; that finding is gated on `tag:read` and a source cannot filter by permission (E23), so the note is a sibling with R10's wording. Restrictive rows stay on the old name (G2), so only allows can be lost |
| D18 | **No migration.** Copies are tracked in memory for the hold (as #757's inserted rows); events and the report need no table. Main is at `0209` | Migration numbers are shared |
| D19 | **View renames (F-C) are out of this package** (§11 item 1) | The view hold (`view_hold.rs`) is a second rename pipeline with its own plan, copy and finish. Doing G2-G4 there doubles every task of this plan for a rarer case. The follow module (§5.2) is public and takes coordinates, so a view package reuses it unchanged |
| D20 | **Platform docs in a separate docs PR after merge**, updating KI-308/KI-309, adding rows for §11 and the G6 Limitation (§9) | The flow the user chose on 2026-10-23 |
| D21 | **egret does not copy an exact-name restriction for the new name to satisfy G5.** The cover must already exist on the destination, or be created by this rename's own copies of literal rows (D1), which count as covers because they will exist when the table arrives. Otherwise the rename is refused and the steward adds the cover | Creating a cover from a pattern row is splitting the pattern, which G4 rules out ("never split or rewritten"): the steward wrote a statement about names, and an egret-made exact-name row would silently decide that this object stays restricted when the rename may be meant to leave the pattern (promoting `staging_x` to `x`). Refusing is the safer side (nothing escapes, nothing is invented) and the simpler one (no generated rows, no names, no events, no compensation); the 409 names exactly what to add |

The deadline, cap and lock order are #757's (90 s per held request, 8 holds, name try-locks then ascending profile
locks).

## 5. Design

### 5.1 Where it runs

`hook_table_commits` keeps #757's branch. The move plan gains two parts:

```text
classify → read body →
  Rename:       move_plan_for_rename    ┐  plan.tags      (#757, unchanged)
  TableCommit / move_plan_for_columns   ├─ plan.perm      (§5.2: copies, allows, left, refusals)
  Transaction:                          ┘  plan.catalog   (§5.5: rename only)
     refusal (D4, D5, D14, #757 C2)    → 409, nothing locked
     tags or perm work                 → hold_move (§5.3)
     catalog work only                 → #755 path, then catalog follow after 2xx (§5.5)
     nothing                           → #755 path, unchanged
  DropNamespace (recursive): #757 purge + §5.6
```

### 5.2 The follow module (`egret-perm-compile/src/follow.rs`, new, public)

Pure selection plus transaction helpers, coordinate-based so the view hold can reuse it (D19):

- `Move { catalog_id, from: Coord, to: Coord }` where `Coord { schema, table, column: Option<String> }`.
- `plan_via(conn, mv) -> PermFollow` reads every explicit grant and path rule of `catalog_id` (one query each, the
  catalog's rows only) and sorts them:
  - `copy_grants` (deny) and `copy_rules`: following rows (D3), restrictive;
  - `move_grants`: following allows;
  - `refused`: restrictive pattern rows (D4) and, for a multi-level destination, following restrictive rows (D14);
  - `left`: allow pattern rows (D4) and following allows into a multi-level destination (D14).
- `handwritten_via(conn, services, catalog_name, mv) -> (refuse, report)`: D5 over `policy.authz_policies`.
- `copy_in_tx(tx, plan, mv) -> Copies { grants: Vec<(source_id, new_id)>, rules: Vec<(source_id, new_id)> }`:
  D13's equivalence check, then `create_grant_in_tx` / `create_rule_in_tx`; rule names per D1 (`SELECT … FOR UPDATE`
  is not needed: a `23505` on `perm.rules_name_key` retries with the next suffix).
- `move_allows_in_tx(tx, ids, mv)`: re-read each id under the lock, re-point it with `update_grant_in_tx` (only the
  renamed level changes), return before/after summaries.
- `delete_copies_in_tx(tx, copies)`: compensation.
- `cover_via(conn, refused_row, mv, pending_copies) -> Cover`: G5, §5.9.
- `refusal_message(refused, handwritten) -> String`: §8, with each refused row's `missing`.

### 5.3 The hold, extended

#757 §5.3 with these changes (step numbers are #757's):

- **Plan** (`move_plan_for_rename`, `move_plan_for_columns`): after resolving the warehouse, also resolve its
  catalog id and name (`registry.catalogs.warehouse_id`, E5). Rows of classification, `follow::plan_via` and
  `follow::handwritten_via` run on the same connection. A column plan keeps a renamed field when it is classified
  (#757 C12) **or** `plan_via` returns work or a refusal for its old name. Refusals answer 409 before any lock.
- **Step 5, copy transaction:** after the classification INSERTs, re-run `plan_via` and `handwritten_via` under the
  profile locks (a refusal now rolls back: 409, nothing renamed), then `copy_in_tx`. The overlay, the recompile and
  `refuse_created_overlaps_via` run as today and now see the copies (a copied mask colliding with a mask already on
  the destination is refused there, #757's rule). Versions read: Trino tag pairs when classification rows were
  inserted; **Trino policy pairs when a grant or rule copy was inserted** (D6); StarRocks policy pairs always.
- **Step 6, wait:** `wait_for_versions(tags, trino_policies ∪ starrocks_policies)` with D7's latch.
- **Compensation:** also `delete_copies_in_tx` in the same transaction as the classification rows.
- **Step 8, `finish`:** in the same transaction, after the classification delete: `move_allows_in_tx` (D2), the
  recompile (already there), and the events of D9 (`perm_grant:move`, `perm_grant:copy`, `perm_rule:copy`,
  `perm_grant:left`, `ranger_policy:left`). A failure is logged and recorded as a failed compile, as today; the
  allows then stay on the old name (fail-closed for the renamed table) and the copies stay on the new one.
- **After `finish`**, still uncancellable: the catalog follow of §5.5.

What each engine serves at each point, for a table with a deny grant `D`, an allow `A` and a path mask `M`:

| point | rows | Trino and StarRocks |
|---|---|---|
| before | `D`, `A`, `M` on `orders` | `orders` restricted, readable to `A`'s holders |
| after copy, during the wait | `D`, `M` on both names; `A` on `orders` | both names restricted; `orders_v2` has no table yet |
| after the rename, before `finish` | same | `orders_v2` restricted from its first read; `A`'s holders cannot read it yet |
| after `finish` | `D`, `M` on both; `A` on `orders_v2` | `orders_v2` restricted and readable to `A`'s holders from the next poll; `orders` (no table) restricted, dormant |

### 5.4 Column renames

The column plan (#757 §5.2) pairs field ids exactly as today. For each pair `(old, new)` the follow move is `Coord
{schema, table, column: Some(old)} → Coord {…, Some(new)}`. Table-level and schema-level rows are unaffected by a
column rename. Row filters have no column level and never take part. The StarRocks wait already runs for every held
commit; the Trino policy wait runs when a copy was inserted. A column swap in one commit (`a → b`, `b → a`) is two
moves; restrictive rows copy both ways and the originals stay on both names, so each column ends restricted by both
sets until a steward removes the dormant ones (fail-closed, shown by the missing-columns finding only when a column
name truly disappears).

### 5.5 Catalog grants (`egret-catalog-engine`)

- `EmbeddedCatalog::table_grants_named_via(conn, warehouse_id, ns: &[String], table) -> Vec<CatalogGrant>`
  (folded, element by element) and `move_table_grants_in_tx(tx, ids, dst_ns, dst_table) -> Vec<Moved>` (D8's union
  rule: delete instead of update when `(role, privilege, dst)` exists).
- The plan records the source and destination; the grants are selected in the after-2xx transaction itself, so a
  grant added during the request is moved too.
- Events `catalog_grant:move` (`resource_id` = the grant id; summary `{role, privilege, before: {namespace, table},
  after: {…}, merged: bool, trigger, rename}`).
- Runs for every 2xx rename of a live, not case-only source: unheld renames through the #755 path then this step, held
  renames after `finish`.

### 5.6 Recursive namespace drop

#757 §5.6's purge gains D15: in its transaction, the allow delete and `perm_grant:delete` events (`detail.trigger =
"iceberg_drop_namespace"`, the caller as actor), before its recompile; after it, a second transaction deletes the
catalog grants (`catalog_grant:delete`). A failure of either is logged; the rows stay and the report shows them.

### 5.7 Events

`egret_audit::governance` gains the actions of D9, `resource::CATALOG_GRANT = "catalog_grant"`, `Family::Catalog`
(read with `catalog_registry:read`, added to `governance_changes.rs`' family map) and the `FAMILIES` /
`GOVERNANCE_ACTIONS` rows. Summaries reuse `governance_events::grant_summary` / `rule_summary` (made `pub` for the
hook's use, or wrapped by a `follow::summaries` helper).

Feed sentences (plain text, as #757 Task 7 found):

- "Catalog client trino-main moved allow grant (read) from sales.orders to sales.orders_v2 (table renamed)".
- "Catalog client trino-main copied deny grant (read, write) from sales.orders to sales.orders_v2; the original stays
  on sales.orders (table renamed)". Rules: "copied mask rule pii-email to …".
- "Catalog client trino-main renamed sales.tmp_a to sales.a; allow grant on sales.tmp_* no longer covers it".
- "Catalog client trino-main renamed sales.orders to sales.orders_v2; hand-written policy lake-trino/orders-read still
  names sales.orders".
- "Catalog client spark-etl moved catalog grant read of role analysts from sales.orders to sales.orders_v2 (table
  renamed)"; "… removed catalog grant read of role analysts on sales.orders (namespace dropped)".

### 5.8 Stale-reference report

- `table_states_ns_via(conn, &[(warehouse_id, Vec<String>, table)])`: E22's query with `t.tabular_namespace_name`
  compared element-wise folded (`lower` over `unnest … WITH ORDINALITY`, or an array of lowered elements); the
  single-level `table_states_via` becomes a wrapper. Pinned by the drift test.
- Findings (D16, D17), each with up to five samples, sentences in singular and plural (FE-COPY-4):
  - `permissions_on_missing_tables`: "4 table names carry grants, rules or hand-written policies but have no table:
    left by a rename (egret keeps restrictions on the old name on purpose), a purge, or written before the table
    existed. A table created under such a name is governed by them." Sample label `catalog.schema.table`, detail
    "deny grant, mask rule pii-email; kept when the table was renamed to sales.orders_v2", `ref` `Grant`/`Rule`/
    `Profile` of the first row. Finding ref `Data`.
  - `permissions_on_missing_columns`: "2 grants or rules name columns that are not in their table: 1 was renamed
    (egret names the new column), 1 was dropped or never existed." Sample `catalog.schema.table.column`, detail
    "renamed to mail" / "not in the table".
  - `catalog_grants_on_missing_tables`: "3 catalog grants name tables that do not exist: they cover nothing until a
    table takes the name." Sample `warehouse.ns.table`, detail "role analysts, read", `ref` `Catalog`.
  - `renamed_names_lost_grants`, `renamed_names_lost_catalog_grants`: R10's sentence with "allow grants" / "catalog
    grants" for "classifications", `ref` `Object` of the renamed table / `Catalog`.
- Frontend: five `CHECKS` entries in `AttentionSection.tsx`.

### 5.9 A covered destination (G5)

`follow::cover_via(conn, refused_row, mv, pending_copies) -> Cover` decides, for each restrictive pattern row D4
would refuse, whether the destination is already covered. **Candidates** are the catalog's explicit deny grants and
path rules that match the destination coordinate (D4's matcher, any mix of literal and pattern levels), plus the
copies this rename is about to insert (D21). Tag-selected rules and hand-written policies are never candidates
(their reach depends on classifications or on JSON egret does not interpret). A candidate **covers** the refused row
when all of these hold:

| kind | scope | body | subjects |
|---|---|---|---|
| deny grant | it matches the destination at the refused row's level or above: a refused table-level deny needs a candidate with no column level (or `*`); a refused column-level deny on `c` is covered by a candidate naming `c` (folded) or with no column level | `effect = deny` and its `actions` ⊇ the refused row's `actions` | its `include` set ⊇ the refused row's `include` set, and its `exclude` set ⊆ the refused row's `exclude` set (principals compared as `(kind, ref)`, refs exact, as stored) |
| mask rule | it names the same column as the refused row, folded, on a table level that matches the destination table | same `mask_type`; same `value_expr` (both NULL, or equal text, byte for byte, no SQL normalisation) | its exemption `include` set ⊆ the refused row's, and its exemption `exclude` set ⊇ the refused row's (a mask that exempts fewer people masks at least as many) |
| row filter | it matches the destination table | the same `filter_expr`, equal text | exemptions as for masks |

Several candidates may share the work for a deny grant: the union of their `actions` and principals counts, so a
`read` deny and a `write` deny to the same group cover a `read, write` deny. Masks and row filters are not combined
(two different expressions are not "equal effect"); exactly one candidate must cover.

"Equal effect" is read as "at least the same restriction on at least the same subjects": a stricter candidate (more
actions, more subjects, fewer exemptions) covers; anything weaker is a **partial cover** and refuses. Text equality
for expressions is deliberately conservative: two expressions that mean the same but are written differently do not
cover, and the steward makes them equal. Principals are compared as stored, not expanded: a deny on a group does not cover a deny on
one of its members, and a deny on a member does not cover one on the group.

`Cover` is `Full` or `Partial { missing }`, where `missing` lists, per refused row: for a deny, the actions and the
subjects not denied on the new name ("read for group:contractors"); for a mask, "no mask on `<column>` with type
`<t>` and expression `<e>`" or the exemptions too many; for a row filter, "no row filter `<expr>`" or the exemptions
too many. The 409 (§8) prints them. The decision runs at plan time and again under the locks in the copy
transaction, like D4: a cover deleted meanwhile refuses then. After the rename nothing ties the cover to the old
pattern: a steward who later deletes the cover removes the restriction from the renamed table, through the normal
write path and its event.

## 6. Principles

- A restriction never leaves a name; an access follows its table.
- Write, wait, then commit; undo exactly what was written; never under-restrict.
- egret rewrites only rows it can rewrite exactly (literal names); a pattern or a hand-written policy is the
  author's, and egret refuses or reports instead.
- Say what is stale; never repair by guess.

## 7. Backend changes (enumerated)

1. `egret-policy/src/plugin_downloads.rs`: D7's latch in `wait_for_versions`.
2. `egret-perm-compile`:
   - `follow.rs` (new, public): §5.2, §5.9;
   - `routes.rs`: D10's name lock in `write_grant`, `write_rule`, `delete_grant`, `delete_rule`;
   - `trigger.rs`, `table_columns.rs`: D12 (`counts_body` skips only classification-compiled bodies);
     `table_states_ns_via`;
   - `governance_events.rs`: summaries reachable from `follow`.
3. `egret-catalog-engine`:
   - `table_hook.rs`: plan parts, refusals, copy/compensate/finish additions, Trino policy pairs, catalog follow,
     namespace purge additions;
   - `store/embedded.rs`: §5.5 queries.
4. `egret-audit/src/governance.rs`: D9 actions, `CATALOG_GRANT`, `Family::Catalog`.
5. `egret-controlplane`: `governance_changes.rs` sentences and the `Catalog` family; `governance_overview.rs` five
   sources.
6. Frontend: `AttentionSection.tsx` `CHECKS`. No wire schema changes (existing `Ref` variants and `Finding` shape);
   if the feed's family list is part of the OpenAPI, regenerate (`npm run gen:api`).

## 8. Wire shapes

Iceberg REST refusals keep the hook's shape (`CommitFailedException`, 409). Examples:

```json
{"error": {"message": "egret refused the table change: 2 restrictions cover sales.orders through a name pattern and would not cover sales.archive_orders: deny grant 7c1e… (sales.ord*) — sales.archive_orders lacks a deny of read for group:contractors; row-filter rule eu-only (sales.*) — sales.archive_orders has no row filter \"region = 'eu'\". egret does not rewrite patterns: add these restrictions for the new name, change the patterns, or choose a name they cover. Nothing was renamed.",
           "type": "CommitFailedException", "code": 409}}
```

```json
{"error": {"message": "egret refused the table change: hand-written Ranger policies name sales.orders with a deny, mask, row filter or exclusion list: lake-trino/orders-mask (412), lake-sr/no-orders (98); egret does not rewrite hand-written policies: edit or migrate them first. Nothing was renamed.",
           "type": "CommitFailedException", "code": 409}}
```

`perm_grant:copy` detail:

```json
{"before": {"id": "…", "catalog": "lake", "schema": "sales", "table": "orders", "column": null, "effect": "deny", "actions": ["read"], "include": ["group:contractors"], "exclude": []},
 "after":  {"id": "…", "catalog": "lake", "schema": "sales", "table": "orders_v2", "column": null, "effect": "deny", "actions": ["read"], "include": ["group:contractors"], "exclude": []},
 "rename": {"from": "sales.orders", "to": "sales.orders_v2"}, "trigger": "iceberg_rename",
 "actor": {"kind": "catalog_client", "id": "…", "client_id": "trino-main-7f3a", "name": "trino-main", "consumer_type": "engine"}}
```

The grant-write refusal during a hold is the catalog-tag write's: HTTP 409 `{"code": "table_change_held", …}`.

## 9. Migration and compatibility

- No database migration (D18).
- Behaviour on upgrade:
  - renames of tables (or columns) carrying a deny grant, a path mask or a path row filter are held for up to two
    policy polls (typically 30-60 s, at most 90 s), like classified renames;
  - renames carrying only allows are held without a wait (milliseconds) and their allows apply to the new name from
    the next policy poll;
  - a rename blocked by a restrictive pattern or a hand-written policy answers 409 with the rows to fix;
  - catalog grants follow every rename.
- Rows left by renames before the upgrade are not moved; the report shows them.
- **Limitation (G6), for the platform docs:** after a rename, a holder of an allow grant on the table cannot read
  it under its new name until the engines' next policy polls apply the moved allow, up to about two polls (about
  60 s at the default 30 s poll). Restrictions apply from the first read; only access is late.
- Release note: renames now carry permissions; restrictions also stay on the old name by design (clean them up from
  the Overview); allows reach the renamed table one poll after the rename; check the five new findings after upgrade.

## 10. Risks

- **Deadline with both Trino waits.** A classified table with a deny grant waits for a Trino tag pair and a Trino
  policy pair. D7's latch removes the tag regression's extra poll (E27); two policy polls (~60 s at 30 s) remain
  inside 90 s with less margin than #757's ~55 s. The live check measures it. Fallback if it does not fit: a longer
  deadline for holds that wait on both kinds (Trino's REST client was measured at 90 s only; #757 E17).
- **Trino stops reporting `lastKnownVersion` on policy polls** (a plugin build change): every rename that copies a
  restriction on a Trino-bound warehouse times out with 409 naming the plugin. Loud, never silent (the confirmation
  is never vacuous). Remedy as for StarRocks in #757 §10.
- **Fail-closed gap for allows** (G6, accepted): up to two policy polls after the rename in which allow holders
  cannot read the renamed table.
- **Dormant restrictions accumulate** on retired names and count against the profile ceilings (D12). A long rename
  chain can push a profile toward its ceiling; the refusal then names the ceiling and the report lists the dormant
  rows to remove.
- **G4 still blocks promoting a table out of a name-pattern restriction** (`tmp_a → a` under `deny sales.tmp_*`)
  unless the new name is covered (G5): the steward changes the pattern first, or adds the cover if the table should
  stay restricted.
- **A cover is only as durable as its row** (G5): deleting the covering restriction later removes it from the
  renamed table; the delete is an ordinary audited write.
- **Concurrent hand-written policy edits** are not name-locked: an operator who adds a literal deny on the source
  between the copy's re-check and the commit is not caught (narrow; the policy then names a missing table and the
  report shows it).
- **Migration undo** (E9): moving an allow that a policy migration created changes its hash, so that migration's
  undo answers 409; undo needs the allow put back first. Restrictive copies leave originals untouched and do not
  affect undo.
- **Catalog grants and spelling** (F-B): a grant spelled differently from what a client sends never matched; moving
  it keeps it as useless as before.

## 11. Related defects (found, not fixed here)

1. **View renames (F-C, D19).** `perm.grants` and `catalog_grants` (a `table` grant covers `views/{name}`, E12) on a
   view name stay on the old name after `POST …/views/rename`. Same KI class; a follow-up reuses `follow.rs` in the
   view hold.
2. **Catalog grants are case-sensitive while the catalog is not (F-B, E12).** Fails closed; a case-only rename still
   cuts a holder off who spells the new way.
3. **Glob-only hand-written policies** that matched the old name are not checked (D5). Legacy and shrinking (a
   generated profile cannot gain new ones).
4. **Catalog-grant admin writes record no governance event** (E10). Only the hook's moves and purges do after this
   package.
5. **Expressions naming a renamed column** (a mask `value_expr` or a row-filter `filter_expr`) are not rewritten;
   the engine errors or fails closed (unmeasured).
6. **No optimistic concurrency** on grants, rules or hand-written policies (findings D10 b): last writer wins outside
   a hold.

## 12. Non-goals

- Rewriting patterns or hand-written policy JSON (G3, G4).
- Re-keying permissions by table uuid or field id.
- Engine grants (`perm.engine_grants`), which are not table-keyed.
- F-A (the prerequisite PR) and view renames (§11 item 1).
- An automatic cleanup of dormant restrictions (a steward removes them through the normal write path).
- Platform docs in this PR (D20).

## 13. Open questions

None. The first draft's two were answered on 2026-10-27 (G5, G6).

## 14. Implementation notes (append-only)

Deviations the code forces are recorded here, per task, as they happen.

### Step 0 (2026-10-28): rebased onto `ff25210b`

- **Base.** The branch is rebased onto `main` `ff25210b` (#761). Two security PRs landed after the plan's base
  `4807c3e3`: #758 (F-A, `1e0a3cf8`, spec `2026-10-27-catalog-rename-authorizes-source-design.md`) and #760 (F4,
  `2d2c3f35`, spec `2026-10-27-vended-credentials-follow-grants-design.md`). The prerequisite is met.
- **F-A changes nothing this design depends on.** The guard now reads the body of `POST …/tables/rename` and of
  multi-table and single-table commits (`GuardedBody`), requires drop-equivalent write on the source and
  namespace-level write on the destination, and puts the same bytes back, so the table hook still reads the body it
  read before. The guard runs before the hook and still inserts `CatalogCaller` (now with `warehouse` and `role`) into
  the request extensions; `Caller::resolve` reads only `subject` and `audience`, so the hook's actor is unchanged.
  Task 13 adds a test through the real guard that the events name the verified client.
- **E12/E13 are stale.** `catalog_grant_covers` in `egret-bin` is now a `#[cfg(test)]` wrapper; the rule is
  `egret_catalog_engine::grant_match::catalog_grant_covers_by`, which compares names with lakekeeper's ICU
  `case_insensitive` collation (`CatalogNameEq`, asked of Postgres through `CatalogStore::names_equal_in_catalog`).
  So a grant on `Orders` now covers `orders`. Consequences for D8: the grants a rename selects are those whose
  namespace elements and table name equal the source **under `lakekeeper.case_insensitive`** (in SQL, the same
  collation), not under `lower()`; the union check (D8's delete-instead-of-update) uses the same collation; a
  rename whose source and destination are equal under the collation moves nothing (the grant already covers both
  spellings), which replaces D8's "case-only" test. F-B (§11 item 2) is fixed on main by #758; it stays listed for
  history.
- **F4: catalog grants now also decide vended credentials** (`EgretAuthorizer`, ReadData/WriteData from
  `catalog.catalog_grants` through `grant_match`). A grant left on the old name therefore also loses the renamed
  table's read/write credentials and gives them to a new table at the old name. Task 13 adds a test that the
  authorizer's decision for the renamed table follows the moved grant.
- **Rows re-checked.** E1/E2 (0154; only 0178/0179 ALTER `perm`), E3 (`eval.rs:350`), E5 (`trigger.rs:1730`,
  `:1775`), E7 (`store.rs:300-340`, no `origin` in the SET list; `create_rule_in_tx` exported), E9
  (`migrate.rs:718`, `:727`), E10 (no index on `catalog.catalog_grants`), E15 (`table_hook.rs:1048`, `:1225`),
  E17 (`table_hook.rs:1554-1563`), E18 (`plugin_downloads.rs:211-238`), E20 (`view_tags.rs:94`; the 409 lives in
  `egret-bi/src/catalog_tags.rs:317-345`), E21 (`trigger.rs:682-685`; `DesiredPolicy` carries `source_kind` and
  `rule_id`, so D12 can tell a classification body from a grant or path-rule body without a name prefix), E22
  (`table_columns.rs:365`), E23 (`governance_overview.rs:183`), E26/E27 (both passages present). Line numbers
  otherwise as cited.
- **D5.** The compiler emits `registry.catalogs.name` at the catalog level (`grants.rs::scope_to_resources`). No
  migration seeds an `operator` policy; a fresh stack's hand-written policies are the infra defaults (all `*`).
- **D12 sites.** Only `trigger.rs:682-685` (the profile total). `rules.rs:536`/`:562` and `clearance.rs:459` count
  tag expansions and clearance masks, which move with the classifications and stay correctly uncounted; path rules
  and grants never reach them.
- **Migrations.** `main`'s newest is `0210`. None is planned (D18).

### Tasks 1-9 (2026-10-28)

- **Task 6, D11 widened by two cases.** A move whose only work is an allow pattern that stops covering the table
  (`left`), a plain hand-written allow to report (D5), or a covered pattern row (G5) also takes the hold, without a
  wait: the `*:left` events are recorded in `finish`, which only the hold has, and a covered pattern needs its cover
  to stay put until the commit. The hold's name locks give that: a route write deleting the cover on the destination
  name answers 409 `table_change_held` (D10). Cost: milliseconds, as for allows.
- **Task 6, waits.** A plan with no classification rows and no restrictive copy waits for nothing (StarRocks
  included): the overlay of such a plan changes no StarRocks body that #755's after-commit recompile would not.
- **Task 6, test plugin.** The fake Trino plugin (`tests/common/plugins.rs`) polls `/plugins/policies/download` too,
  under the same plugin id, as E26 measured.
- **Task 7.** The allow move in `finish` landed with Task 6's commit (the code path was the same edit); Task 7's reds
  were shown on the pre-Task-6 hook and by deleting the move (`take(0)`).
- **Task 8, the plan-time read.** The plan's refusals were already in Task 6's commit; Task 8's reds were run on the
  pre-Task-6 hook. The "pattern written during the plan" test holds the hook's own profile advisory lock from the
  test's connection, so the hook plans, waits, and re-reads under the lock after the test writes the pattern: no test
  hook in production code.
- **Task 9, covers.** `follow::plan` applies G5 itself: a covered pattern row goes to `PermFollow::covered` with no
  refusal. Two follow tests of Task 2 met G5: `ord*` covered `orders*` (same body), and a column move's own mask copy
  (D21) covered a mask pattern `e*`; the tests now give those rows another subject or mask type.
- **Task 11, no fix needed.** Every red of Task 11 already passed on the Task 10 code (the swap, D13, the
  non-2xx-after-commit path); they were shown red on the pre-Task-6 hook and by mutation (D13's skip, the case-only
  test), and the commit is `test(catalog)`. The migration-undo pair drives the real `migrate` and `undo` routes in
  `egret-perm-compile/tests/migrate_route.rs`, with the follow module's writes (the ones the hook makes) standing in
  for the rename: a catalog-engine test cannot reach the perm router. `an_allow_only_rename_is_held_without_a_wait` is
  Task 7's `a_renamed_tables_allows_move_and_keep_their_ids` (no plugin runs; a wait would 409).
- **Task 12, swapped masked columns.** The column plan already carried grants and path rules since Task 6's commit;
  Task 12's reds ran on the pre-Task-6 hook. Swapping two columns whose path masks differ (`a` HASH, `b` NULL)
  would leave both masks on each column, a pair the copy's overlap check refuses (#757's rule, §5.3): the commit is
  409 and nothing is renamed. Swapped denies carry both sets as §5.4 says. Pinned by
  `swapped_columns_with_different_masks_are_refused_by_the_overlap_check`; the steward removes one mask first.
- **Task 13, catalog grants.** Selection and the union check use lakekeeper's `case_insensitive` collation element
  by element (Step 0); a rename whose two names are one name under it moves nothing (D8's case-only rule). The source
  must be a live table at any depth (`store::embedded::live_table_id_via`); a non-2xx is followed when the
  destination now carries the source's table id. The follow runs in its own transaction after the hold's `finish`,
  or after #755's path, inside one uncancellable task. Each event carries `warehouse_id`, so the feed links the
  registry catalog (`Ref::Catalog`). The end-to-end test through the real `EgretCatalogGuard` and F4's `MockS3` is in
  `egret-bin/src/catalog_vended_credentials_tests.rs`: the moved read grant vends read-only credentials on the renamed
  table, the moved write grant read-write-delete, a new table at the old name is 403 for both, and the events name
  the verified client.
- **Task 14.** The purge runs for single-level namespaces only, as #757's (a multi-level recursive drop keeps its
  rows; the reports show them). The allow events reuse `perm_grant:delete` with `detail.trigger =
  "iceberg_drop_namespace"`, and the feed adds "(namespace dropped)" to that sentence.
