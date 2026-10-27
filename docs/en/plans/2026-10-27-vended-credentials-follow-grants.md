# Vended credentials follow catalog grants: plan

Spec: [`docs/en/specs/2026-10-27-vended-credentials-follow-grants-design.md`](../specs/2026-10-27-vended-credentials-follow-grants-design.md).
Branch `fix/vended-credentials-follow-grants`, on the F-A branch (`fix/catalog-rename-authorizes-source`).

Every task is red first: the test is written and run against the unchanged code, its failure recorded,
then the code changes until it passes.

## Task 1: an object store and STS the tests can read

- `crates/egret-bin/src/catalog_vended_credentials_tests.rs`: `MockS3`, an axum server on
  `127.0.0.1:0` that answers path-style S3 (PUT, GET, HEAD, DELETE, ListObjectsV2, DeleteObjects) and
  STS `AssumeRole`. Every minted key is recorded with the session policy lakekeeper sent; the store
  enforces that policy, as RustFS does.
- `env()` creates a lakekeeper warehouse with egret's own S3 profile (`s3_storage_profile`: STS on,
  signing off) against the mock, through `create_lakekeeper_warehouse`, so lakekeeper's own storage
  validation runs against it, then mounts the real guard and the table hook (`mount_catalog_engine`).

## Task 2: red tests on the base

Write and run against the base (`a381fc1b`):

1. a table `read` grant loads a read-only key (header `vended-credentials`);
2. the same with no delegation header;
3. a table `write` grant loads RWD (control, green on the base);
4. a namespace `read` grant is read-only on every table in it;
5. a namespace `write` grant loads RWD (control);
6. a `catalog` `manage` client (an egret engine) loads RWD (control);
7. a `catalog` `read` client is read-only;
8. `/credentials` follows the grant (reader read-only, writer RWD);
9. a reader after a writer is not handed the writer's cached key;
10. create, stage-create (on another table's location and on its parent) and register (of another
    table's metadata file) are refused, and no key minted in the attempt reaches the other table;
11. a stage-create with no location gets RWD on its own location only (control).

Record which fail on the base (expected: 1, 2, 4, 7, 8, 9) and which are pins.

## Task 3: one grant rule for the guard and the authorizer

- Move `grants_write`, `catalog_grant_covers_by` and `CatalogNameEq` from `egret-bin/src/main.rs` to
  `crates/egret-catalog-engine/src/grant_match.rs`. `CatalogNameEq::load` takes the requested names, not
  the guard's `Need`s. The guard calls the moved code; its unit tests stay where they are.
- Gate: `cargo test -p egret-bin` green (no behaviour change).

## Task 4: the egret authorizer

- `crates/egret-catalog-engine/src/authorizer.rs`: `EgretAuthorizer { server_id, system, grants }`.
  `ReadData`/`WriteData` on a table follow the catalog client's grants; everything else answers as
  `AllowAllAuthorizer`. Caller from the metadata: a catalog-client claim, egret's system principal
  (allowed), anyone else (data access refused).
- `catalog_client_metadata(&CatalogClientIdentity)` builds the principal metadata.
- `mount.rs`: `EngineAuthorizer = EgretAuthorizer` in `EngineState`/`EngineServer`; `build_state` builds
  it from the server id, the system principal and a `CatalogStore` on the same database. Replace
  `AllowAllAuthorizer` in `admin.rs`, `bootstrap.rs`, `recovery.rs`, `warehouse.rs`, `lib.rs`.

## Task 5: the mount names the caller

- `CatalogCaller` gains `warehouse` and `role`; the guard fills them from the verified token.
- `guard_then_inject_metadata`: an allowed request other than `/config` gets
  `catalog_client_metadata`; `/config`, and every request with no guard, keep the internal actor.
- Run Task 2's tests: all green.

## Task 6: mutation check

Mutate each new decision branch (the read/write mapping, the system/other/client split, the warehouse
scope check, the `/config` carve-out, the write flag passed to the grant rule) and confirm a test kills
each.

## Task 7: gate, live check, docs

- Full local gate as the spec lists it.
- Live check on stack `egret-f4creds` (M1–M4 again, Trino writes, #757 classification move).
- Record both in the spec. Draft the PR body.
