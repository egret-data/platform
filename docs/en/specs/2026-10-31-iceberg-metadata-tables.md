# Iceberg metadata tables under governance — short plan

Package M of the DG debt waves. Rows: Iceberg metadata tables (`t$files`, `t$partitions`, …) bypass masks, row
filters and denies on Trino, and bypass Ranger entirely on StarRocks.

## Premise (measured on a compose stack, StarRocks 4.1.6 with `access_control = ranger`, Trino 483 provisioned by egret)

| Engine | Who | What they read |
|---|---|---|
| StarRocks | user with no policy at all (refused on `SELECT * FROM orders`) | `orders$files` (record counts, per-column lower/upper bounds = real e-mails), `$snapshots`, `$manifests` (partition bounds), `$partitions` (partition values, counts), `$history`, `$refs`, `$metadata_log_entries` |
| Trino | masked user (`email` MASK_NULL) | `orders$files` bounds of `email`, `$entries`, `$manifests` partition summaries |
| Trino | row-filtered user (`region = 'us'`) | bounds and partition values of the `eu` rows, record counts |

Trino checks a read of `orders$files` as a read of a table named `orders$files` (the Ranger plugin builds the resource
from the name as written, and asks `getRowFilters`/`getColumnMask` about that name). A grant on `table: *` matches it;
a mask, row filter or deny on `table: orders` does not. StarRocks' `ColumnPrivilege.check` skips `MetadataTable`, so
Ranger is never asked; StarRocks 4.1.x has no config, session variable or catalog property that turns metadata tables
off (`MetadataTableFactoryProvider` always serves them for Iceberg catalogs).

## What changes

- **Trino (compiler).** For every restricting policy of a Trino profile — compiled or hand-written mask, row filter,
  select deny, deny-all-else — the compiler adds `perm-hidden-meta-{source}`: a deny on the same catalog and schema,
  `table: <each table value>$*`, `column: *`, `select`, for the people the source restricts, excepting the people it
  exempts. A Trino tag mask or clearance (tag service) gets one such deny per table carrying the tag that the profile
  reaches. Reaped by the existing `perm-hidden-` sweep.
- **StarRocks.** Nothing egret can enforce. Known issue (S1) with the exposure and the workaround; no SQL-text gate.

## Decisions taken (conservative)

1. All of a table's metadata tables are denied (`$*`), not only the value-bearing ones: Trino adds metadata tables
   across versions, and `$history`/`$refs`/`$properties` are of little use to someone who cannot read the values.
2. Only the people a policy restricts lose them; everyone else (including users with a plain grant) is unaffected.
3. Ranger's first-match order: only exempt items before the first restricting item become exceptions. A person whose
   first match is a later exempt item is refused metadata tables although the source shows them clear values (an
   over-refusal, never a leak).
4. An excluding table level (`!orders`) cannot be turned into `!orders$*` (that matches every ordinary table), so it
   becomes `*$*`: the metadata tables of every table in the scope.
5. Deny grants are included: a deny on `orders` with an allow on `*` elsewhere left `orders$files` readable.
6. Profiles that are not compiled by the permission model (hand-written `manual` profiles) get nothing: the compiler
   does not run for them.
