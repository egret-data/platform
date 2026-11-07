# Warn that StarRocks serves Iceberg metadata tables without asking Ranger

## Premise

StarRocks 4.1.6 / 4.1.7 serves the metadata tables of an Iceberg table (`t$files`, `t$partitions`,
`t$manifests`, `t$snapshots`, `t$history`, `t$refs`, `t$metadata_log_entries`) without asking its
access controller: `ColumnPrivilege.check` skips `MetadataTable`, and no setting turns them off.
Measured live (package M, PR #821): a StarRocks user with no policy at all, refused
`SELECT id FROM orders`, reads `orders$files` (row counts, per-column lower/upper bounds),
`$partitions`, `$manifests`, `$snapshots`, `$history`, `$refs`, `$metadata_log_entries`. A masked
user reads the masked column's bounds; a row-filtered user reads the bounds and partition values
of the rows filtered away. egret cannot enforce this from Ranger policies, so it must say so
where an admin binds a StarRocks engine to a policy profile and where they look at that binding.

## What

One component, `StarRocksMetadataTablesWarning` (`routes/control-plane/`), built on the existing
`WarningCallout` (an `Alert` with the triangle icon and the word "Warning"). No new styling.
It renders only for a StarRocks engine that is bound to (or about to be bound to) a profile:

| Place | Shown when |
|---|---|
| Governance → Engine & policy → engine page (banner under the header) | engine type `starrocks` and the grants read names a profile |
| Control plane → provisioned engine detail (above the tabs) | engine type `starrocks` and a profile in the profile list names the engine |
| Control plane → external engine detail, Overview | engine type `starrocks` and a profile is bound |
| Control plane → external engine detail, Settings (the profile picker) | engine type `starrocks` and a profile is picked |
| Control plane → Register external engine (the profile picker) | type StarRocks and a profile is picked |

The provisioned-engine create wizard (`EngineCreate`) binds no profile for StarRocks (it sends no
`authzServiceId`; only `PUT /engines/{id}` can bind one, and no UI control does), so it gets no
warning; the engine detail shows it once a profile is bound.

## Decision: every StarRocks engine with a profile, not only profiles with a restriction

The task allowed narrowing to profiles with a mask, row filter, deny or classification rule. Not
taken, because the bypass is wider than those: eve, who had **no** grant on `orders`, read its
metadata tables. A grant is a restriction too — a profile that grants `orders` to some people
restricts everyone else — so every bound profile is affected, and narrowing would hide the
warning exactly where an admin relies on grants alone. It also needs no new API and no backend
change.

## Wording

> StarRocks serves the metadata tables of Iceberg tables (`$files`, `$partitions`, `$manifests`,
> `$snapshots`, `$history`, `$refs`, `$metadata_log_entries`) without asking Ranger. Anyone who can
> log in to this engine can read, for every table in an attached Iceberg catalog, its row counts,
> partition values, data file paths and each column's lowest and highest values, whatever the
> profile's grants, masks, row filters and classifications say. egret cannot block this. Give
> StarRocks logins only to people who may see these values, and serve masked or filtered data
> through Trino.

## Tests

Component tests (vitest): the warning is shown on each place for a bound StarRocks engine, hidden
for a Trino engine and for a StarRocks engine with no profile. Live: screenshots at 1440 and
390 px of each place.
