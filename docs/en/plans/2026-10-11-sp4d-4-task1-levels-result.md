# SP4d-4 Task 1 — what an absent level grants, measured on the real Ranger jars

**Measured:** 2026-10-11, branch `feat/sp4d-4-closing-review`, code commit `52cacaf20` (on top of
`75a7de1ff`). Gate: `editor_shaped_levels_on_the_real_ranger_jars`
(`crates/egret-ranger-eval/tests/differential.rs`), which runs the real `RangerPolicyEngineImpl`
through `tests/ranger` (`EgretCaseRunner`) under egret's `trino.json` and `starrocks.json`, at
`ranger-plugins-common` **2.5.0 and 2.8.0** each.

**Verdict, in one line:** the evidence supports **B1** on every engine × version measured. A policy
that stops at the table (`d3`), schema/database (`d2`) or catalog (`d1`) is **denied `select` at
column depth** on all four runs, and both deployed plugins check `SELECT col` at **column** depth.
Both Part A pins held: the broader mask policy wins inside an equal-priority band, and a tag mask
beats a path mask at equal priority — at both versions.

## 1. What was asked

Every set is **one** policy and nothing else (no background), granting user `alice` the accesses
`select, insert, delete` (StarRocks adds `update`), `policyType 0`, priority 0. `h` = the def's
hierarchy (`catalog, schema, table, column` on Trino; `catalog, database, table, column` on
StarRocks); request values `wh / sales / orders / email`.

| id | resources | what it is |
|---|---|---|
| `d1` | `{h0: wh}` | editor: stops at catalog |
| `d2` | `{h0: wh, h1: sales}` | editor: stops at schema/database |
| `d3` | `{h0: wh, h1: sales, h2: orders}` | editor: stops at table |
| `d4star` | `{… h2: orders, h3: *}` | what egret's compiler emits for `d3` |
| `gap` | `{h0: wh, h2: orders}` | editor: a middle level emptied |
| `d1star` | `{h0: wh, h1: *, h2: *, h3: *}` | *added*: what B1 would save for `d1` |
| `d2star` | `{h0: wh, h1: sales, h2: *, h3: *}` | *added*: what B1 would save for `d2` |

Requests at depth 1 (`{h0}`) … 4 (`{h0..h3}`), each concrete access at the default scope
(`SELF`), `_any` at `SELF` (`_any:S`) and at `SELF_OR_DESCENDANTS` (`_any:D`), and — *added* —
`select` at `SELF_OR_DESCENDANTS` (`sel:D`). `sel:D` is the control that proves the new
`resourceMatchingScope` field reaches the engine: `_any` already matches descendants at `SELF`, so its
two columns could not differ, while `sel:D` differs from `sel` wherever a policy is deeper than the
request (e.g. `d2` at depth 1: `sel` = `-`, `sel:D` = `A`).

`A` = allowed, `-` = denied. Cells are Java's `isAllowed`.

## 2. The four tables

### levels matrix [trino @ ranger 2.5.0] — 172 cases in 6.79 s

```text
  policy   | depth 1:    sel    ins    del _any:S _any:D  sel:D | depth 2:    sel    ins    del _any:S _any:D  sel:D | depth 3:    sel    ins    del _any:S _any:D  sel:D | depth 4:    sel    ins    del _any:S _any:D  sel:D
  d1       |               A      A      A      A      A      A |               -      -      -      -      -      - |               -      -      -      -      -      - |               -      -      -      -      -      -
  d2       |               -      -      -      A      A      A |               A      A      A      A      A      A |               -      -      -      -      -      - |               -      -      -      -      -      -
  d3       |               -      -      -      A      A      A |               -      -      -      A      A      A |               A      A      A      A      A      A |               -      -      -      -      -      -
  d4star   |               -      -      -      A      A      A |               -      -      -      A      A      A |               A      A      A      A      A      A |               A      A      A      A      A      A
  gap      |               -      -      -      -      -      - |               -      -      -      -      -      - |               -      -      -      -      -      - |               -      -      -      -      -      -
  d1star   |               A      A      A      A      A      A |               A      A      A      A      A      A |               A      A      A      A      A      A |               A      A      A      A      A      A
  d2star   |               -      -      -      A      A      A |               A      A      A      A      A      A |               A      A      A      A      A      A |               A      A      A      A      A      A
```

### levels matrix [trino @ ranger 2.8.0] — 172 cases in 6.88 s

```text
  policy   | depth 1:    sel    ins    del _any:S _any:D  sel:D | depth 2:    sel    ins    del _any:S _any:D  sel:D | depth 3:    sel    ins    del _any:S _any:D  sel:D | depth 4:    sel    ins    del _any:S _any:D  sel:D
  d1       |               A      A      A      A      A      A |               -      -      -      -      -      - |               -      -      -      -      -      - |               -      -      -      -      -      -
  d2       |               -      -      -      A      A      A |               A      A      A      A      A      A |               -      -      -      -      -      - |               -      -      -      -      -      -
  d3       |               -      -      -      A      A      A |               -      -      -      A      A      A |               A      A      A      A      A      A |               -      -      -      -      -      -
  d4star   |               -      -      -      A      A      A |               -      -      -      A      A      A |               A      A      A      A      A      A |               A      A      A      A      A      A
  gap      |               -      -      -      -      -      - |               -      -      -      -      -      - |               -      -      -      -      -      - |               -      -      -      -      -      -
  d1star   |               A      A      A      A      A      A |               A      A      A      A      A      A |               A      A      A      A      A      A |               A      A      A      A      A      A
  d2star   |               -      -      -      A      A      A |               A      A      A      A      A      A |               A      A      A      A      A      A |               A      A      A      A      A      A
```

### levels matrix [starrocks @ ranger 2.5.0] — 199 cases in 5.50 s

```text
  policy   | depth 1:    sel    ins    del    upd _any:S _any:D  sel:D | depth 2:    sel    ins    del    upd _any:S _any:D  sel:D | depth 3:    sel    ins    del    upd _any:S _any:D  sel:D | depth 4:    sel    ins    del    upd _any:S _any:D  sel:D
  d1       |               A      A      A      A      A      A      A |               -      -      -      -      -      -      - |               -      -      -      -      -      -      - |               -      -      -      -      -      -      -
  d2       |               -      -      -      -      A      A      A |               A      A      A      A      A      A      A |               -      -      -      -      -      -      - |               -      -      -      -      -      -      -
  d3       |               -      -      -      -      A      A      A |               -      -      -      -      A      A      A |               A      A      A      A      A      A      A |               -      -      -      -      -      -      -
  d4star   |               -      -      -      -      A      A      A |               -      -      -      -      A      A      A |               A      A      A      A      A      A      A |               A      A      A      A      A      A      A
  gap      |               -      -      -      -      -      -      - |               -      -      -      -      -      -      - |               -      -      -      -      -      -      - |               -      -      -      -      -      -      -
  d1star   |               A      A      A      A      A      A      A |               A      A      A      A      A      A      A |               A      A      A      A      A      A      A |               A      A      A      A      A      A      A
  d2star   |               -      -      -      -      A      A      A |               A      A      A      A      A      A      A |               A      A      A      A      A      A      A |               A      A      A      A      A      A      A
```

### levels matrix [starrocks @ ranger 2.8.0] — 199 cases in 6.05 s

```text
  policy   | depth 1:    sel    ins    del    upd _any:S _any:D  sel:D | depth 2:    sel    ins    del    upd _any:S _any:D  sel:D | depth 3:    sel    ins    del    upd _any:S _any:D  sel:D | depth 4:    sel    ins    del    upd _any:S _any:D  sel:D
  d1       |               A      A      A      A      A      A      A |               -      -      -      -      -      -      - |               -      -      -      -      -      -      - |               -      -      -      -      -      -      -
  d2       |               -      -      -      -      A      A      A |               A      A      A      A      A      A      A |               -      -      -      -      -      -      - |               -      -      -      -      -      -      -
  d3       |               -      -      -      -      A      A      A |               -      -      -      -      A      A      A |               A      A      A      A      A      A      A |               -      -      -      -      -      -      -
  d4star   |               -      -      -      -      A      A      A |               -      -      -      -      A      A      A |               A      A      A      A      A      A      A |               A      A      A      A      A      A      A
  gap      |               -      -      -      -      -      -      - |               -      -      -      -      -      -      - |               -      -      -      -      -      -      - |               -      -      -      -      -      -      -
  d1star   |               A      A      A      A      A      A      A |               A      A      A      A      A      A      A |               A      A      A      A      A      A      A |               A      A      A      A      A      A      A
  d2star   |               -      -      -      -      A      A      A |               A      A      A      A      A      A      A |               A      A      A      A      A      A      A |               A      A      A      A      A      A      A
```

All four tables are **identical cell for cell** apart from StarRocks' extra `upd` column: neither the
plugin version nor the service-def changes any answer here. StarRocks' `accessTypeRestrictions`
(e.g. `select` only on `column`) pruned nothing — `d1` is allowed `select` at depth 1 on StarRocks too.

What the tables say:

- **`d1` / `d2` / `d3` grant every concrete access at exactly their own depth and nothing below it.**
  `select` at depth 4 is `-` for all three, everywhere. Above their depth they answer only `_any`
  (`A`), which is how `SHOW`/filter checks find them.
- **`d3` vs `d4star` differ in exactly one place: depth 4** (every access). Everywhere else they are
  equal. So egret's compiled form of a table-stopping policy is a strict superset of it.
- **`d1star` ⊇ `d1` and `d2star` ⊇ `d2`, cell by cell.** The `*`-filled form keeps every allow the
  editor-saved form has and adds every deeper depth. B1's rewrite therefore never takes an allow
  away; it only adds the deeper ones (which, for `d1star`, means the whole catalog's tables and
  columns — the steward's intent question, below).
- **`gap` decides nothing anywhere** — denied at every depth, every access, both scopes, `_any`
  included. It is a dead policy on both engines at both versions, so it must be fixed in **both**
  branches (refuse it, or save the emptied level as `*`).

## 3. Rust / Java agreement

Concrete-access `SELF` cells were also evaluated by `egret_ranger_eval::eval::evaluate`:

- **On the measured pairing** (Trino @ 2.5.0, StarRocks @ 2.8.0 — `EngineType::plugin_version`):
  **196 / 196 agree.** Asserted.
- **Off the pairing** (Trino @ 2.8.0, StarRocks @ 2.5.0): the evaluator **refuses all 196** with
  `PluginVersionMismatch` and nothing else, by design (`eval.rs:3069-3077`). The brief expected the
  evaluator to answer at both versions; it cannot — it only speaks for the pairing it was read
  against. The test asserts that exact refusal instead. With `plugin_version: None` the same 196
  cells **agree 196 / 196** (printed, not asserted). Relevant to Task 7 (external engines): an
  external Trino on 2.8.0 or StarRocks on 2.5.0 gets `Undecidable` from the evaluator today.
- Total Java verdicts in the run: **742** (cases sent = verdicts received, asserted).

## 4. The two Part A pins (policyType 1, request `{wh, sales, orders, email}`, `select`, `alice`)

| pin | set | 2.5.0 (Trino def) | 2.8.0 (Trino def) | 2.5.0 (StarRocks def) | 2.8.0 (StarRocks def) |
|---|---|---|---|---|---|
| 1 | `m-broad` `{wh,sales,*,email}` MASK (id 21) + `m-narrow` `{…orders,email}` MASK_NULL (id 22) | **MASK** (21) | **MASK** (21) | **MASK** (21) | **MASK** (21) |
| 1 control | names swapped: `z-broad` MASK (21) + `a-narrow` MASK_NULL (22) | **MASK** (21) | **MASK** (21) | **MASK** (21) | **MASK** (21) |
| 1 control (added) | names, ids **and** download order swapped: `a-narrow` (21) listed first, `z-broad` MASK (22) | **MASK** (22) | **MASK** (22) | **MASK** (22) | **MASK** (22) |
| 2 | path MASK_NULL `{wh,sales,orders,email}` (31) + tag MASK on `pii` (931), request tagged `pii` @ SELF | **MASK** (931) | **MASK** (931) | — | — |

**Both facts hold on both jars.** The broader policy wins whatever its name, id or position in the
download — it is `evalOrder`. The Rust evaluator answers pin 1 with
`Undecidable[EvalOrderAmbiguous{21, 22}]` on the measured pairing (the modelling gap the spec
names) and pin 2 with `Masked{MASK, policy 931}` on Trino @ 2.5.0; off the pairing it answers
`PluginVersionMismatch`, as above.

## 5. How each plugin asks — re-read at the deployed tags

Checkouts: `../trino` at tag `470` (the image egret provisions, `crates/egret-provisioner/src/trino.rs:1556`;
its `plugin/trino-ranger/pom.xml:19` pins `dep.ranger.version` **2.5.0**), and `../starrocks` at tag
`4.1.4` (the `@@version_comment` recorded in `crates/egret-engine-mgmt/tests/starrocks_ddl_live.rs:12`;
its `fe/pom.xml` pins `ranger-plugins-common` **2.8.0**). Every line below was read with
`git -C ../<repo> show <tag>:<path>`, not from the working tree (`../trino` HEAD is `482-75-g280b81bbc4e`,
`../starrocks` HEAD is `148a7fdc44e` — neither is the deployed version).

### Trino 470 (`plugin/trino-ranger/src/main/java/io/trino/plugin/ranger/`)

`hasPermission` builds the request and calls `rangerPlugin.isAccessAllowed` with the default scope
(`RangerSystemAccessControl.java:961-966`); `hasPermissionForFilter` is the only place a scope is set:

```java
// RangerSystemAccessControl.java:968-977 (and the Identity overload, :986-995)
RangerTrinoAccessRequest request = createAccessRequest(resource, context, accessType, action);
request.setResourceMatchingScope(RangerAccessRequest.ResourceMatchingScope.SELF_OR_DESCENDANTS);
```

```java
// RangerSystemAccessControl.java:523-530
public void checkCanSelectFromColumns(SystemSecurityContext context, CatalogSchemaTableName table, Set<String> columns)
{
    for (RangerTrinoResource resource : RangerTrinoResource.forColumns(table.getCatalogName(), …, columns)) {
        if (!hasPermission(resource, context, SELECT, "SelectFromColumns")) {
```

```java
// RangerTrinoResource.java:102-109
public static List<RangerTrinoResource> forColumns(String catalogName, String schema, String tableName, Set<String> columns)
{
    if (columns.isEmpty()) {
        return ImmutableList.of(forTable(catalogName, schema, tableName));
    }
    return columns.stream().map(columnName -> forColumn(catalogName, schema, tableName, columnName)).collect(Collectors.toList());
}
```

```java
// RangerSystemAccessControl.java:431-443
if (!hasPermission(createTableResource(table), context, INSERT, "InsertIntoTable")) {   // :433
if (!hasPermission(createTableResource(table), context, DELETE, "DeleteFromTable")) {   // :441
// :1004-1007 createTableResource = RangerTrinoResource.forTable(catalog, schema, table)
```

```java
// RangerSystemAccessControl.java:415-419
if (!hasPermission(RangerTrinoResource.forSchema(schema.getCatalogName(), schema.getSchemaName()), context, _ANY, "ShowTables")) {
// :455-462 filterTables
RangerTrinoResource resource = RangerTrinoResource.forTable(catalogName, tableName.getSchemaName(), tableName.getTableName());
if (!hasPermissionForFilter(resource, context, _ANY, "filterTables")) {
```

Also: `canAccessCatalog` → catalog, `_any`, SELF (`:247-250`); `checkCanShowSchemas` → catalog, `_any`,
SELF (`:330-335`); `filterCatalogs` / `filterSchemas` → catalog / schema, `_any`, SELF_OR_DESCENDANTS
(`:282`, `:343`); `checkCanUpdateTableColumns` (UPDATE) → table, **`insert`**, SELF (`:533-537`);
`checkCanShowColumns` → table, `_any`, SELF (`:515-519`).

### StarRocks 4.1.4 (`fe/fe-core/src/main/java/com/starrocks/`)

No call site in `fe/fe-core/src/main` sets a matching scope (`git -C ../starrocks grep -n
ResourceMatchingScope 4.1.4 -- fe/fe-core/src/main` → nothing), so every StarRocks request is SELF.
`RangerAccessController.hasPermission` maps `PrivilegeType.ANY` to `RangerPolicyEngine.ANY_ACCESS`
(`_any`) and everything else through `convertToAccessType`, which is `privilegeType.name().toLowerCase(ENGLISH)`
(`authorization/ranger/RangerAccessController.java:113-133`; `RangerStarRocksAccessController.java:478-480`).

```java
// authorization/ColumnPrivilege.java:133-138 — the SELECT check for a table behind an external controller
Set<String> columns = scanColumns.getOrDefault(tableName, new HashSet<>());
for (String column : columns) {
    try {
        Authorizer.checkColumnAction(context,
                tableName, column, PrivilegeType.SELECT);
```

(`scanColumns` is the optimised plan's PRUNED scan set — `:98-123`, with the comment at `:101-106`.)

```java
// authorization/ranger/starrocks/RangerStarRocksAccessController.java:158-168
public void checkColumnAction(ConnectContext context, TableName tableName,
                              String column, PrivilegeType privilegeType) throws AccessDeniedException {
    hasPermission(RangerStarRocksResource.builder()
                    .setCatalog(tableName.getCatalog())
                    .setDatabase(tableName.getDb())
                    .setTable(tableName.getTbl())
                    .setColumn(column)
                    .build(),
```

```java
// sql/analyzer/AuthorizerStmtVisitor.java
Authorizer.checkTableAction(context, tableName, PrivilegeType.INSERT);   // :325
Authorizer.checkTableAction(context, tableName, PrivilegeType.DELETE);   // :346
Authorizer.checkTableAction(context, tableName, PrivilegeType.UPDATE);   // :365
// RangerStarRocksAccessController.java:103-117 checkTableAction → catalog/database/table, that privilege
```

```java
// qe/ShowExecutor.java:593 visitShowTableStatement … :633-636
Authorizer.checkAnyActionOnTable(context,
        new TableName(catalogName, db.getFullName(), table.getName()));
// RangerStarRocksAccessController.java:119-133 checkAnyActionOnTable → catalog/database/table, PrivilegeType.ANY (= _any)
```


### Per engine: which depth, access and scope each statement asks at

| statement | Trino 470 (Ranger 2.5.0) | StarRocks 4.1.4 (Ranger 2.8.0) |
|---|---|---|
| `SELECT col FROM t` | **column** (one request per column), `select`, SELF | **column** (one per pruned scanned column), `select`, SELF |
| `SELECT count(*) FROM t` | **table** (`forColumns` with an empty set), `select`, SELF | one request per column the optimiser keeps in the scan; with an empty scan set the loop at `ColumnPrivilege.java:134-135` asks **nothing** — read, not run |
| `INSERT` | table, `insert`, SELF | table, `insert`, SELF |
| `DELETE` | table, `delete`, SELF | table, `delete`, SELF (plus column `select` for the WHERE, `AuthorizerStmtVisitor.java:352`) |
| `UPDATE` | table, **`insert`**, SELF | table, `update`, SELF |
| `SHOW TABLES` | schema, `_any`, SELF (`checkCanShowTables`) then per table `_any`, SELF_OR_DESCENDANTS (`filterTables`) | per table, `_any`, SELF (`checkAnyActionOnTable`) |
| table filter | table, `_any`, **SELF_OR_DESCENDANTS** | (no filter* path; the per-table SHOW check above) |

## 6. Timings and gates

| run | elapsed |
|---|---|
| Step 2: existing differential (2 tests, `--skip` migration) after the `ask_ranger_json` refactor | 69.52 s (2 passed; baseline 59.8 s) |
| Step 4: the matrix, final form (7 policies, 742 verdicts) | 25.94 s; per Maven run 5.5–6.9 s |
| `just test-ranger-gates`, line 1 `ranger_engine_gate` | 10.03 s (baseline 13.3 s) |
| line 2 differential (`--skip` migration and the matrix) | 66.10 s (baseline 59.8 s) |
| line 3–4 the matrix (5-row form at the time of the recipe run) | 44.90 s; after `d1star`/`d2star` were added, lines 3–4 were re-run alone at `52cacaf20`: listing guard OK, 25.94 s, 1 passed |
| line 5 migration differential | 581.05 s (baseline 491.7 s; another worktree's `cargo check` was running concurrently) |
| `cargo test -p egret-ranger-eval --no-fail-fast` (no E2E) | 266 passed / 0 failed across 10 suites (baseline 265 + this test) |

## 7. Recommendation

The rule (from the plan):

- **B1** if `d3` (and `d2`, `d1`) is denied `select` at depth 4 on every profile × version **and**
  both engines check `SELECT col` at column depth.
- **B2** if `d3` is allowed `select` at depth 4 on every profile × version.
- **Mixed** otherwise.

**Measured: B1, on every engine × version.**

- Trino def @ 2.5.0 and @ 2.8.0: `d1`, `d2`, `d3` `select` at depth 4 = `-`. Trino 470 asks
  `SELECT col` at column depth (`RangerSystemAccessControl.java:523-530`, `RangerTrinoResource.java:102-109`). → **B1.**
- StarRocks def @ 2.5.0 and @ 2.8.0: same cells `-`. StarRocks 4.1.4 asks `SELECT col` at column
  depth (`ColumnPrivilege.java:133-138` → `RangerStarRocksAccessController.java:158-168`). → **B1.**

What a table-stopping policy grants today is only what is asked at table depth: `INSERT`, `DELETE`,
`UPDATE` (both engines) and `SELECT count(*)` on Trino. A steward who saved "table `orders`, all
columns" by emptying the column level gets a policy that **lets nobody read a column** — the live
result of 2026-09-24 (§3, T1-table) reproduced on the jars.

**B2 has no equivalence to accept.** `d3` and `d4star` decide differently at every depth-4 cell, so
the grammar cannot accept `d3` as "deciding the same" as anything egret's compiler emits today (it
always emits the column level); B2 would mean teaching the compiler to emit table-stopping access
policies, which reproduce a grant that reads no column.

**What B1 changes for an existing saved policy** (to state to the user): `d1star` ⊇ `d1` and
`d2star` ⊇ `d2` and `d4star` ⊇ `d3`, cell by cell — no allow is lost; the deeper levels are added.
For a catalog-stopping policy that is the whole catalog's tables and columns, which is a real widening
of what Ranger enforces today (though it matches what such a policy reads as in the editor).

**`gap`:** decides nothing on any engine at any version — fix it in both branches.

## 8. Things that contradict or refine the spec's premises

1. **The spec's premises hold.** Ancestor-match is not an allow (§2), Trino and StarRocks ask at
   column depth (§5), and the broader-wins / tag-beats-path evalOrder facts hold on both jars (§4).
   Part A may proceed.
2. **The evaluator does not answer off its pairing.** The brief assumed `evaluate` could be
   compared at both versions; it refuses `PluginVersionMismatch` for Trino @ 2.8.0 and StarRocks @
   2.5.0. For Task 7 (external engines that may run either version) this is a refusal every such
   request will hit until the evaluator declares the other pairing. With the version unstated it
   agreed 196/196 here — evidence for widening, not proof.
3. **Not run, only read:** StarRocks' `SELECT count(*)`. If the optimiser prunes a table's scan set
   to empty, `ColumnPrivilege.check` asks Ranger nothing for that table (`:134-135`). Worth a live
   probe; it does not move the B verdict.
4. The javap read of `RangerAccessRequestImpl.setResourceMatchingScope` (both versions) shows it
   also resets `ISREQUESTPREPROCESSED` to FALSE, like `setResource`; `EgretCaseRunner` sets it before
   seeding tags, so the seeded tag set is unaffected (pin 2 passing with policy 931 confirms it).
