# StarRocks fork patch 0010: Paimon system tables under a base-table policy (design)

**Status:** design, 2026-11-12. Follow-up FU-1 of the unmeasured-paths probe (package C).

## Problem

A Paimon table's **system tables** — `t$files`, `t$snapshots`, `t$partitions`, and the other
`t$…` tables — expose data of the base table `t`: partition values (including partitions a row
filter is meant to hide), per-file object-store paths, record counts, sizes, and per-column
lower/upper bounds (actual column values, for a table whose files carry value statistics).

Fork patch `0001` already refuses this for StarRocks-native **metadata tables**: querying one
requires `SELECT` on the base table *and* is refused to a user whom a row access policy or a
column masking policy on the base table applies to (neither policy can be applied to file-level
statistics). But `0001` hooks only the `table instanceof MetadataTable` branch in
`com.starrocks.authorization.ColumnPrivilege`. A Paimon system table does **not** resolve as a
`MetadataTable`: the Paimon connector returns it as an ordinary
`com.starrocks.catalog.PaimonTable` whose `getCatalogTableName()` is the literal `orders$files`.
It therefore skips `0001`'s check and is authorised under its own name only.

Measured by the probe (check **C14-paimon**, 2026-11-09), on **both** the official
`starrocks/allin1-ubuntu:4.1.7` and the fork `4.1.7-egret.4` images: user `bob`, who holds a
row filter `region = 'us'` and a column mask on `paimon.sales.orders`, reads every row of
`orders$files`, `orders$snapshots` and `orders$partitions` — the `eu` partition, the full
`s3://…` file paths and the record counts the filter and mask are meant to withhold. User `eve`,
with no grant, is correctly DENIED (name-based authorisation still applies), so the leak is
specifically the **policy bypass**, not a missing grant.

This is hypothesis **H7** of the probe, confirmed. It is a pre-existing StarRocks behaviour, not
a fork regression; the existing fork patches never reached it.

## Goal

A user to whom a row access policy or a column masking policy on a Paimon base table `t` applies
cannot read `t`'s system tables (`t$files`, `t$snapshots`, `t$partitions`, and every other
`t$…`). The fix mirrors `0001`'s decided behaviour for the Paimon code path. No change to a user
who has unrestricted `SELECT` on `t`, and no change on the official image (which ships no fork
patches).

## Non-goals

- **Applying the filter/mask to the system-table rows.** The system tables' columns
  (`partition`, `record_count`, bounds) are not the base table's columns, so a row filter on
  `region` or a mask on `email` has no faithful mapping onto `$files` rows. We refuse the read,
  as `0001` does, rather than attempt a rewrite.
- **Column value statistics in `$files`.** The probe's seed (pypaimon) wrote null value
  statistics, so the measured leak is partition values, paths and counts; a Java/Flink writer
  would also expose the `email` min/max through `$files`. The refuse-under-policy fix covers that
  case too (any mask/filter user is refused outright), so it needs no separate work; measuring it
  would need Spark/Flink (excluded by the probe's open question 4).
- **Paimon branch tables** (`t$branch_x`) and **time-travel** are a different concern (reading
  another branch/snapshot's data, not a stats bypass) and are out of scope.
- **Adopting `0001`'s base-table-`SELECT` model for Paimon.** `0001` requires `SELECT` on the base
  table because a native metadata table is not otherwise name-authorised; a Paimon system table is
  (measured). Requiring base `SELECT` here would be a behaviour change — a `table = orders` grant
  would newly reach `orders$files`. `0010` keeps the engine's existing name-based authorisation and
  only overlays the policy refusal. The column-partial-grant bound leak `0001` also closes for
  native tables (a user with some of `t`'s columns reading all columns' bounds) is left to the
  name-based grant on Paimon, and was not measured (the seed wrote null value statistics).
- **The official image.** It is not part of the fork build; it stays as a probe baseline and
  keeps leaking, which is the evidence the patch's check detects what the patch fixes.

## Design

One patch, `patches/0010-Check-a-Paimon-base-table-policy-for-its-system-table.patch`, touching
`fe/fe-core/src/main/java/com/starrocks/authorization/ColumnPrivilege.java` and adding one unit
test. It depends on `0001` (reuses its `isRestrictedByPolicy` helper and error code) and applies
after it.

### What it adds, and what it leaves alone

The patch is an **overlay**, not a full adoption of `0001`'s model. The crucial difference from
`0001`'s metadata tables: a Paimon system table is **already authorised by its own name** by the
standard `Authorizer` (during statement authorisation, before `ColumnPrivilege` runs). The probe
measured this — a `table = orders` grant is DENIED on `orders$files`, while a `table = *` grant
reaches it. So `0010` does **not** need, and does **not** add, `0001`'s base-table-`SELECT`
requirement, and it does **not** change which grants reach the system table. It adds exactly one
thing: a Paimon system table of a base table `t` is refused to a user whom a **row access policy
or a column masking policy on `t`** applies to. That is the measured leak and nothing more.

### The hook

In `ColumnPrivilege`'s first loop over the query's tables (the same loop `0001` extended), add a
branch alongside the existing `table instanceof MetadataTable` branch:

```java
if (table instanceof PaimonTable paimonTable && isPaimonSystemTable(paimonTable)) {
    refusePaimonSystemTableUnderPolicy(context, tableName, paimonTable);
    continue;
}
```

The `continue` skips the column-masking/row-filter rewrite for the system table (that rewrite is
meaningless on a system table's columns), exactly as the `MetadataTable` branch does. The
standard name-based table authorisation has already run before this point, so a user who could
not reach the system table by name is already refused; a user who could (a `*` grant) now meets
the policy check.

### Detecting a system table and its base table

Paimon's own `org.apache.paimon.catalog.Identifier` already classifies the name:

```java
private static boolean isPaimonSystemTable(PaimonTable t) {
    return new Identifier(t.getCatalogDBName(), t.getCatalogTableName()).isSystemTable();
}
```

`Identifier.isSystemTable()` is true for every `t$…` table; `Identifier.getTableName()` returns
the base table name `t` (stripping the `$…` suffix). This covers all system tables uniformly —
not only the three the probe reads.

### The check

`refusePaimonSystemTableUnderPolicy`:

1. Skip when `context.isMetadataContext()` (a collect job running as root on behalf of an
   already-checked query), as `0001` does.
2. Build the base `TableName` from the system table's catalog, its `getCatalogDBName()`, and
   `Identifier.getTableName()`.
3. If `isRestrictedByPolicy(context, baseTableName)` — the helper `0001` already added, true when
   a row access policy or a non-empty column masking policy applies — throw
   `ErrorCode.ERR_ACCESS_DENIED_METADATA_TABLE_UNDER_POLICY` with the system table and base table
   names (the same error `0001` uses; its message reads "metadata table", which covers a Paimon
   system table).

No new error code, no new helper beyond `isPaimonSystemTable` and
`refusePaimonSystemTableUnderPolicy`. Note it does **not** call `Authorizer.checkTableAction` for
`SELECT` on the base table — that is `0001`'s behaviour for synthetic metadata tables, and is
unnecessary (and privilege-expanding) here because Paimon system tables are name-authorised
already.

### Why a new patch, not a change to 0001

`0001` is the metadata-table (`MetadataTable`) concern; this is the Paimon-connector
(`PaimonTable`) concern — a distinct class and code path, found separately by the probe. A
separate `0010` keeps each patch to one concern (the fork's convention), documents the FU-1 fix
on its own, and rebases independently. `0010` reuses `0001`'s `isRestrictedByPolicy` and error
code, so it must sit after `0001` in the patch order.

## Testing

### Unit (fe-core)

A test in the style of `0001`'s `ColumnPrivilegeMetadataTableTest`, using a stub access
controller, over a `PaimonTable` whose catalog name is `orders$files`:

- refused when a row access policy on `orders` applies (`ERR_ACCESS_DENIED_METADATA_TABLE_UNDER_POLICY`);
- refused when a column masking policy on `orders` applies;
- allowed when no policy applies (the overlay adds no base-`SELECT` requirement: name-based
  authorisation, which the test's controller lets through, still governs);
- a metadata context (`isMetadataContext`) is not refused;
- a non-system Paimon table (`orders`) is untouched by the new branch.

### End to end (probe)

The probe's **C14-paimon** check is the gate. After `0010`:

- on the fork image `4.1.7-egret.5`: `bob` and `eve` are DENIED on `orders$files`,
  `$snapshots`, `$partitions` → **C14-paimon = ENFORCED**;
- on the official `4.1.7` image (no patch): still **LEAK** — the evidence that the check detects
  exactly what `0010` fixes, as every patch-numbered check in `run.sh`/`probe.sh` does;
- the `alice` sight control (unrestricted user still reads the system tables) stays green, so a
  pass is not a blanket refusal.

The fork's existing `tests/e2e/run.sh` suite (15/15) must stay green — `0010` must not refuse a
user with plain `SELECT` and no policy, nor touch Iceberg metadata tables (`0001`'s path).

## Shipping

- `patches/0010-…patch` added; `UPSTREAM` `EGRET_PATCH_LEVEL` 4 → 5; rebuild and tag
  `4.1.7-egret.5` (FE and allin1); one pin bump in the platform
  (`ghcr.io/egret-data/egret-starrocks-{fe,allin1}:4.1.7-egret.5`).
- Fork README: add `0010` to the Patches table and record the e2e result line for `egret.5`.
- Platform docs: close the probe report's FU-1 and known-issues entry for C14-paimon once
  `egret.5` is the default image.

## Boundaries carried from the probe

- The official image keeps leaking (not part of the fork build).
- Column value statistics in `$files` via a Java/Flink writer remain unmeasured, but are covered
  by the refuse-under-policy behaviour.
- Paimon branch and time-travel reads are a separate, unmeasured concern.
