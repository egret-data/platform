# P8: vended storage credentials last 15 minutes (KI-315)

## Premise (main at 9417afe1d)

- egret's S3 storage profile (`egret_catalog_engine::s3_storage_profile`) turns
  STS on and does not set `sts-token-validity-seconds`, so lakekeeper uses its
  default 3600 s: `AssumeRole` is called with `DurationSeconds=3600`
  (lakekeeper `service/storage/s3.rs`). Nothing in compose, helm or kind sets
  it. Reproduces.
- The credential cache is lakekeeper's `S3_STC_CACHE` (moka, keyed by the
  request: table location and read or read-write-delete, plus the storage
  profile and credential hashes). An entry lives half the credential's
  lifetime, capped at one hour (`StcExpiry`), counted from when the STS call
  returned. So a cached key is never handed out with less than half its
  lifetime left (1800 s today, 450 s at 900 s) and never outlives the
  credential. No fix needed there.

## What changes

1. New config key `catalog.sts_token_validity_seconds`
   (`EGRET_CATALOG__STS_TOKEN_VALIDITY_SECONDS`), default 900, validated to
   900..=43200 (the STS `AssumeRole` bounds).
2. egret writes it into every S3 warehouse's storage profile as
   `sts-token-validity-seconds`, at creation.
3. At boot, before the catalog serves, egret sets it on every S3 warehouse the
   catalog already holds (`apply_vended_credential_lifetime`): warehouses
   created before this change carry 3600 s. One `UPDATE` of that one profile
   field in lakekeeper's `warehouse` table; lakekeeper's own storage update is
   not used because it re-tests the store and replaces the storage secret.

The default covers compose, helm and kind: none of them set the key, so they
all get 900 s. No migration (lakekeeper's schema holds the profile; the boot
step is idempotent).

## Decisions

- A boot step rather than a migration: egret's migrations do not touch
  lakekeeper's schema, and a later change of the setting must reach existing
  warehouses too.
- The boot step also lowers or raises a warehouse to the configured value; the
  configuration is the one source.
- Out of scope, as decided: callers with the same permission on the same table
  share one key (the cache key does not include the caller). KI-315 stays a
  limitation with "up to 15 minutes".

## Tests

- `egret-config`: default 900, bounds, read from the environment.
- `egret-bin` `catalog_vended_credentials_tests` (real guard + lakekeeper, mock
  STS that honours `DurationSeconds`):
  `vended_credentials_live_fifteen_minutes_by_default` (STS asked for 900 s,
  load result's `s3.session-token-expires-at-ms` 900 s ahead) and
  `a_warehouse_from_before_gets_the_configured_lifetime` (a 3600 s warehouse
  vends 900 s after the boot step).
