# Vended S3 credentials follow catalog grants: design

Date 2026-10-27. Branch `fix/vended-credentials-follow-grants`, rebased onto main `827da4bb` after
F-A merged (#758, `1e0a3cf8`; branch `fix/catalog-rename-authorizes-source`, spec
[`2026-10-27-catalog-rename-authorizes-source-design.md`](2026-10-27-catalog-rename-authorizes-source-design.md),
§10.7 recorded this defect). Plan: [`docs/en/plans/2026-10-27-vended-credentials-follow-grants.md`](../plans/2026-10-27-vended-credentials-follow-grants.md).

`LK/` below is `crates/lakekeeper/src/` of the pinned lakekeeper checkout (rev `791650571d`, 0.12.3).

**Commit ids.** The measurements in §6–§9 were taken before the rebase. `a381fc1b` is the F-A head
the branch was built on (its code is on main as `1e0a3cf8`). `12ba90c1` is the pre-rebase id of
`4599c6c5`; `git diff 12ba90c1 4599c6c5 -- crates/` is empty, so the gate and the live check apply
to the rebased code. §11 records the review and what changed after it.

## 1. Problem

A catalog client loads an Iceberg table and lakekeeper hands it short-lived S3 credentials (STS
`AssumeRole` with a session policy scoped to the table's prefix). The scope of that policy was
read-write-delete for **every** caller that could load the table, a client holding only a `read`
grant included. A reader could overwrite and delete the table's data and metadata files and break the
table for everyone.

Two causes, both needed:

1. lakekeeper picks the scope from its authorizer: `WriteData` allowed → `ReadWriteDelete`, else
   `ReadData` → `Read` (`LK/server/tables.rs` `interpret_authz_results_for_load_table`), and egret ran
   it on `AllowAllAuthorizer`.
2. The mount put `RequestMetadata::new_lakekeeper_internal` on every request after the guard.
   `LakekeeperInternal` bypasses every table action, data-plane included
   (`LK/service/authz/table.rs`, the `bypass` in `are_allowed_table_actions`), so even a real
   authorizer would never have been asked.

## 2. Measurements (stack `egret-f4creds`, RustFS 1.0.1, before the fix)

Roles minted through the control-plane API: `f4tread` (table read `f4sales.orders`), `f4nsread`
(namespace read `f4sales`), `f4twrite` (table write), `f4nswrite` (namespace write), `f4setup`
(catalog write, made the throwaway tables).

- **M1, the reader.** `load` with `X-Iceberg-Access-Delegation: vended-credentials` → 200 with
  credentials. `load` with **no header** → 200 with credentials (lakekeeper's `parse_data_access`
  reads no header as "server delegated, not specified", and STS-enabled storage then vends).
  `client-managed` → no credentials. `/credentials` → the same credentials. The session policy (read
  from the RustFS session token) allowed `GetObject, GetObjectVersion, PutObject, DeleteObject,
  AbortMultipartUpload, ListMultipartUploadParts` on the table prefix. RustFS honoured the prefix:
  other tables' prefixes and the bucket root were 403.
- **M2, the reader, destructive, on its own throwaway table.** Overwrote a data file (200), deleted a
  data file (204), deleted the metadata file (204).
- **M3, the writer afterwards.** `f4sales.orders` scan failed (`Parquet file size is 7 bytes`); the
  other tables were intact.
- **M4.** The namespace-read client got the same RWD policy on every table in the namespace. The
  read and write clients were handed the **same** access key: lakekeeper's STS cache is keyed by
  (location, permission, profile), not by caller.

## 3. Decisions

User decisions (2026-10-27), binding:

- **D1 Approach (a).** An egret `Authorizer` for lakekeeper answers from `catalog.catalog_grants`,
  by the guard's own rule (privilege ladder, catalog/namespace/table scope, the ICU case-insensitive
  name comparison of F-A §D12), and the mount passes a real caller principal instead of the internal
  actor.
- **D2** A read grant gets **read-only** vended credentials (still readable by pyiceberg and Trino).
  A write or manage grant covering the table gets read-write-delete. No grant: the guard refuses, as
  before.
- **D3** An engine attached with a read-only role becomes read-only. egret-provisioned Trino and
  StarRocks hold `catalog` `manage` and keep working unchanged.
- **D4** The credential lifetime (lakekeeper's 3600 s, the proposed 900 s) is **not** in this package
  (§9).

Controller defaults (C1…C14), each with its reason, are in §5.

## 4. Design

### 4.1 Shape

```text
client ── token ──▶ guard (egret-bin) ── allowed: CatalogCaller{sub, warehouse, role} ──▶ mount
mount ── RequestMetadata: Principal(egret-catalog~<sub>), claim egret_catalog_client ──▶ lakekeeper
lakekeeper load / credentials ── ReadData? WriteData? ──▶ EgretAuthorizer ── grants ──▶ catalog_grants
lakekeeper ── AssumeRole(policy: Read | ReadWriteDelete) ──▶ S3 STS
```

### 4.2 One grant rule

`grants_write`, `catalog_grant_covers_by` and `CatalogNameEq` moved from the guard to
`egret_catalog_engine::grant_match`. The guard and the authorizer call the same code, so the request
the guard lets through and the scope lakekeeper vends for it cannot disagree. `CatalogNameEq::load`
now takes the requested names (`&[&str]`) instead of the guard's `Need`s.

### 4.3 The authorizer, method by method

`EgretAuthorizer { server_id, system, grants }` (`crates/egret-catalog-engine/src/authorizer.rs`).
"Allow" means it answers `true` as `AllowAllAuthorizer` did; the right-hand column says why that is
safe.

| Method | Answer | Why |
|---|---|---|
| `are_allowed_table_actions_impl`: `ReadData`, `WriteData` | **the caller's grants** (§4.5) | the only question the guard cannot answer: the scope of vended credentials (load, `/credentials`) |
| `are_allowed_table_actions_impl`: `GetMetadata`, `Commit`, `Drop`, `Rename`, `IncludeInList`, `Undrop`, `GetTasks`, `ControlTasks`, `SetProtection` | allow | the guard decides every table route on its path or body (F-A); undrop/tasks/protection are management API, not mounted |
| `are_allowed_namespace_actions_impl` | allow | guard: namespace routes, create/register need write on the namespace, recursive drop needs catalog write |
| `are_allowed_warehouse_actions_impl` | allow | guard: the path prefix must be the token's warehouse; `/config` only the token's warehouse |
| `are_allowed_view_actions_impl` (incl. `Select`) | allow | guard decides view routes; a view load vends read-only credentials on the view's metadata location; `Select` is only asked on DEFINER chains, which need trusted engines egret does not configure |
| `are_allowed_generic_table_actions_impl` | allow | generic tables are management API, not mounted |
| `are_allowed_server/project/user/role_actions_impl`, `list_projects_impl`, `can_search_users_impl`, `check_assume_role_impl` | allow | management API, not mounted for catalog clients; only egret's in-process system principal calls it |
| `can_bootstrap`, `bootstrap` | `Ok` | egret's bootstrap (`ensure_bootstrapped`) |
| `create_*`/`delete_*` (user, role, project, warehouse, namespace, table, view, generic table) | `Ok`, nothing recorded | hooks for an authorizer with its own relation store (OpenFGA); egret's grants live in `catalog.catalog_grants` |
| `new_router` | empty | no authorizer API |
| `HealthExt` | no checks | nothing to check |

### 4.4 Who is asking

The guard returns `CatalogCaller { subject, audience, warehouse, role }` from the verified token. For
an allowed request other than `GET /config`, the mount builds the request metadata with
`catalog_client_metadata`: lakekeeper's public `new_lakekeeper_internal` then `set_authentication`
with `Actor::Principal(UserId "egret-catalog~<subject>")` and a claim
`egret_catalog_client: {warehouse, role, subject}`. The authorizer reads the claim; the user id only
labels the request (lakekeeper's events, logs).

The authorizer classifies the caller (`Caller::of`):

- the claim is present → a catalog client: its role's grants decide;
- the actor is egret's system principal (`bootstrap::system_user_id(catalog.system_user)`) →
  allowed. egret's browse, recovery, warehouse-admin and lineage-sweep paths already run as this
  principal (not as the internal actor), so they reach the authorizer and keep full access;
- anything else → no data access (fail closed). No code path produces one today.

### 4.5 The data decision

For each `ReadData`/`WriteData` lakekeeper asks about a table, as `(namespace, table, write)`:

0. If lakekeeper asks on behalf of another user (`ActionOnTable.user` is set: a DEFINER view chain
   or a for-user permission check, neither reachable in egret today), the answer is no: egret's grants
   belong to the caller (review finding 3).

1. The token's warehouse (by name) must map to the warehouse lakekeeper resolved
   (`lakekeeper_warehouse_id`). Otherwise every data action is refused.
2. The role's grants (`grants_for_role`), and lakekeeper's collation for every (grant name, table
   name) pair (`CatalogNameEq`), one query each.
3. Allowed iff some grant covers `(namespace, table)` with `write` for `WriteData`
   (`catalog_grant_covers_by`).

A store error is `AuthorizationBackendUnavailable`: the load fails rather than guessing.

### 4.6 What stays the internal actor, and why

- **`GET /config`.** It vends nothing, and lakekeeper's `get_config` registers the calling principal
  as a lakekeeper user (`maybe_register_user`) on first contact. With a principal every catalog client
  would add a row to `lakekeeper.users`.
- **Requests with no guard** (`mount_catalog_engine(.., None, ..)`, tests and in-process setup
  routers): no caller to name.
- **lakekeeper's own task workers** (GC: expiry, purge, log cleanup): they build their own metadata.

The view hold's and the table hook's sub-requests (`load_stored`, `drop_registered`) reuse the
original request's metadata, so they run as the catalog client; both ask only control-plane actions.

## 5. Controller defaults

- **C1 How the principal is carried.** A claim in the `Authentication` of an `Actor::Principal`,
  built from public lakekeeper APIs only (no `test-utils`). Reason: the claim carries warehouse and
  role, which a `UserId` cannot hold without a parser; the same construction `bootstrap.rs` already
  uses. A subject lakekeeper would refuse as a user id (empty, ≥ 128 chars, control characters) is
  named by a UUIDv5 hash; the claim keeps the real subject.
- **C2 `/config` keeps the internal actor** (§4.6). Pinned by `a_config_call_registers_no_lakekeeper_user`.
- **C3 Every non-data method answers as `AllowAllAuthorizer`** (§4.3). Reason: the guard already
  decides those routes, and a second, divergent decision in lakekeeper would turn into 403s the guard
  did not intend.
- **C4 egret's system principal keeps full data access; unknown principals get none.** Reason:
  unchanged behaviour for egret's own paths; fail closed for anything unforeseen.
- **C5 The warehouse is re-checked in the authorizer** (§4.5 step 1). Reason: defence in depth if
  the guard's prefix check ever changes. Through the guard it is unobservable (the guard refuses a
  prefix that is not the token's warehouse); `tests/authorizer_db.rs` pins it through lakekeeper's
  router with the metadata set directly.
- **C6 Grants are read on every data decision, not cached.** Reason: a revoked or narrowed grant takes
  effect on the next load. Cost (corrected after review finding 2): the guard already runs
  `get_warehouse_by_name`, `grants_for_role` and, when some grant name and requested name are not
  byte-equal, `names_equal_in_catalog`; the authorizer runs the same two or three again. So a table
  load or `/credentials` call costs 4–6 egret queries, on every load, including `client-managed` and
  `If-None-Match` loads, since lakekeeper asks `ReadData`/`WriteData` on every load. The authorizer's
  queries use lakekeeper's read pool. Reading the grants once per request (carrying the guard's in the
  claim or a request extension) is a possible follow-up (§10).
- **C7 The credential cache.** lakekeeper caches a vended key by (location, permission, profile)
  (`STCCacheKey`). The permission is part of the key, so a reader can never be handed a cached RWD key:
  measured by `a_reader_after_a_writer_is_not_handed_the_cached_writer_key` (writer first gets an RWD
  key, the reader then gets a different, read-only key, the writer's next load the cached RWD key).
  Callers with the same permission on the same table still share a key (§9, O2).
- **C8 Create, stage-create, register.** A creator still gets RWD on the location of the table it
  creates (lakekeeper hard-codes it, and the guard requires namespace write). **Within one
  warehouse**, a client-chosen location cannot be another table's: lakekeeper refuses, measured and
  pinned for create and stage-create on another table's location and on its parent
  (409 `LocationAlreadyTaken`), register of another table's metadata file (409, same table uuid) and
  register of a forged metadata file whose location is another table's (409 `LocationAlreadyTaken`);
  no key minted in the attempt reaches that table. No guard change. Across warehouses the claim does
  not hold (review finding 1): lakekeeper's location check is per warehouse, register does not check
  that the location is empty, and its warehouse-overlap check compares endpoint strings, so two
  warehouses on one bucket whose storage connections write the endpoint differently are not seen as
  overlapping. That needs an admin to have set up overlapping storage, and predates this branch
  (before it, every caller got RWD anyway). Recorded in §10.
- **C9 Views unchanged.** A view load vends read-only credentials on the view's metadata location
  (pinned: `a_view_load_vends_read_only_credentials`, even for a namespace writer); a view commit
  needs write in the guard.
- **C10 The grant store** is built in `build_state` on lakekeeper's read pool (same database; egret's
  queries are schema-qualified), so `build_state`'s signature is unchanged.
- **C11 `CatalogCaller` gains `warehouse` and `role`**, filled by the guard from the verified token.
- **C12 Store errors** fail the request (`AuthorizationBackendUnavailable`), never default to a scope
  (pinned: `a_grant_store_error_fails_the_decision`, on an unreachable store).
- **C13 The test object store** is an in-process S3 + STS (`MockS3`) that records every session
  policy lakekeeper sends and enforces it, as RustFS does (M1). It lets the tests observe the scope
  without S3, and lakekeeper's own storage validation of the warehouse runs against it.
- **C14 egret-bin's `catalog_grant_covers` unit tests stay in egret-bin**, calling the moved rule.

## 6. Tests

All in-process, through `mount_catalog_engine` with the real guard, the table hook and lakekeeper's
router, on a warehouse with egret's S3 profile over `MockS3`
(`crates/egret-bin/src/catalog_vended_credentials_tests.rs`), plus unit tests of the caller
classification (`authorizer.rs`).

| Test | On `a381fc1b` | After |
|---|---|---|
| `a_table_read_grant_loads_read_only_credentials` (store refuses PUT/DELETE with the key) | **FAILED**: RWD policy | ok |
| `a_read_grant_without_the_delegation_header_is_read_only_too` | **FAILED**: RWD | ok |
| `a_namespace_read_grant_is_read_only_on_every_table_in_it` | **FAILED**: RWD | ok |
| `an_engine_client_with_a_catalog_read_role_is_read_only` | **FAILED**: RWD | ok |
| `the_credentials_endpoint_follows_the_grant` | **FAILED**: reader RWD | ok |
| `a_reader_after_a_writer_is_not_handed_the_cached_writer_key` | **FAILED**: same key | ok |
| `a_table_write_grant_loads_read_write_delete_credentials` | ok (control) | ok |
| `a_namespace_write_grant_loads_read_write_delete_credentials` | ok (control) | ok |
| `an_engine_client_with_catalog_manage_loads_read_write_delete` | ok (control) | ok |
| `a_create_or_register_cannot_take_another_tables_location` | ok (pin, C8) | ok |
| `a_stage_create_gets_credentials_on_its_own_location_only` | ok (control) | ok |
| `egret_s_own_principal_still_loads_read_write_delete` (browse load) | ok (pin, C4) | ok |
| `a_config_call_registers_no_lakekeeper_user` | ok (pin, C2) | ok |
| `a_grant_in_another_case_vends_by_the_catalogs_collation` | fails (reader RWD), by construction | ok |
| unit: `a_catalog_client_is_named_by_its_claim`, `egret_s_own_principal_reads_and_writes`, `any_other_principal_gets_no_data_access`, `an_overlong_subject_still_names_the_client` | n/a (new code) | ok |
| after review: `a_mixed_role_writes_only_the_table_it_may_write` (namespace read + table write: RWD on that table, read-only on its sibling) | fails (sibling RWD), by construction | ok |
| after review: `a_view_load_vends_read_only_credentials` (C9) | ok (pin) | ok |
| after review, unit: `a_data_question_on_behalf_of_another_user_is_refused` | **FAILED** on the code before the fix (answered `Ask`) | ok |
| after review, unit: `a_grant_store_error_fails_the_decision` (C12), `answers_merge_in_the_order_lakekeeper_asked` | n/a | ok |
| after review, `tests/authorizer_db.rs` through lakekeeper's router: `a_client_of_the_warehouse_may_read`, `a_client_of_another_warehouse_gets_no_data_access` (C5), `an_unknown_principal_gets_no_data_access` (C4) | n/a | ok |

The red run (on `a381fc1b`, with only the test file added) is the first 11 rows: 6 failed exactly on
the defect (the session policy lakekeeper asked for was RWD, or the reader got the writer's key), 5
passed. The last three egret-bin tests and the unit tests were added with the mutation check (§7);
the collation test fails on the base for the same reason as the read tests (its reader gets RWD).

## 7. Mutation check

Each mutant was applied to the source, the authorizer's unit tests and every egret-bin `catalog_` test
(the 13 above and F-A's guard tests) were run, and the file was restored (`git diff` empty afterwards).

| Mutant | Result |
|---|---|
| M1 `ReadData` asked as a write | killed (6 read-grant tests) |
| M2 `WriteData` asked as a read | killed (6) |
| M3 system principal denied | killed (`egret_s_own_principal_*`, 2) |
| M4 unknown principal allowed | killed (`any_other_principal_gets_no_data_access`) |
| M5 a catalog client skips its grants | killed (7) |
| M6 the warehouse re-check ignored (C5) | **survived**: through the guard a token's warehouse is always the path's, so the check cannot be reached from a request; kept as defence in depth |
| M7 the write flag not passed to the grant rule | killed (6) |
| M8 non-data table actions denied | killed (12, F-A tests included) |
| M9 the no-data-plane early return denies | killed (8, F-A tests) |
| M10 the `/config` carve-out removed | killed (`a_config_call_registers_no_lakekeeper_user`) after that test was added; it survived the first run |
| M11 the mount keeps the internal actor | killed (6) |
| M12 the collation dropped (byte equality) | killed (`a_grant_in_another_case_vends_by_the_catalogs_collation`) after that test was added; it survived the first run |

**After the review** (on `bd80dbe5`; the unit tests, every egret-bin `catalog_` test and
`tests/authorizer_db.rs` per mutant): all 15 killed. M9 no longer exists (the early return now goes
through `merge`).

| Mutant | Result |
|---|---|
| M1 `ReadData` asked as a write | killed (10) |
| M2 `WriteData` asked as a read | killed (9) |
| M3 system principal denied | killed (2) |
| M4 unknown principal allowed | killed (2, incl. `an_unknown_principal_gets_no_data_access` through the router) |
| M5 a catalog client skips its grants | killed (10) |
| M6 the warehouse re-check ignored (C5) | **killed** (`a_client_of_another_warehouse_gets_no_data_access`) |
| M7 the write flag not passed to the grant rule | killed (8, incl. `a_mixed_role_writes_only_the_table_it_may_write`) |
| M8 non-data table actions denied | killed (26) |
| M10 the `/config` carve-out removed | killed (the subject-scoped config test) |
| M11 the mount keeps the internal actor | killed (8) |
| M12 the collation dropped | killed (1) |
| M13 a question on behalf of another user answered with the caller's grants | killed (`a_data_question_on_behalf_of_another_user_is_refused`) |
| M14 a refused question allowed in the merge | killed (`answers_merge_in_the_order_lakekeeper_asked`) |
| M15 a missing answer allowed in the merge | killed (same) |
| M16 a store error read as "no warehouse" (refusal instead of an error) | killed (`a_grant_store_error_fails_the_decision`) |

## 8. Local gate

On `12ba90c1` (the commits after it are docs only), against the test DB
(`postgres://egret:egret@localhost:5432/egret_test`, `EGRET_ALLOW_DB_SKIP` unset), own
`CARGO_TARGET_DIR`. Other sessions kept the host at a load average of 20–40 throughout.

| Check | Result |
|---|---|
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass |
| `just test-shipping-resolution` | pass |
| `cargo test --workspace --no-fail-fast` | 8387 passed, 9 failed, 0 DB skips. 7 are `egret-workflow` `sql_starrocks_live`, which needs a live StarRocks (`EGRET_TEST_STARROCKS_URL`), as on earlier PRs. 2 are timing bounds under load: `table_hook_db::a_rename_with_nothing_bound_moves_at_once` (2.7 s against its bound) passed 3/3 alone; `view_hold_db::overflow_records_never_hold_more_connections_than_the_cap` ("waited 1.23 s" against 1.2 s) passed 6 of 7 alone, the one failure at a load average of 35. Both passed in this branch's own `-p egret-catalog-engine` run and in F-A's gate. |
| `just test-ranger-gates` | pass |
| Frontend `npm ci`, `npm run gen:api` (no diff), lint, `tsc -b`, build, all 10 `test:bundle*` | pass |
| vitest | 8487/8500 under load (13 tests in 8 files, all timeouts); 4 of the files pass when rerun together and the other 4 each pass alone. This branch changes no frontend file. |

## 9. Live check

Stack `egret-f4creds` (port 18780, free beforehand), images built from the worktree at `12ba90c1`,
seeded with `deploy/demo/seed.sh`, the same five roles and clients as §2 minted through the
control-plane API, throwaway tables made with pyiceberg 0.12.0 as `f4setup`. Probes ran in a
container on the stack network (boto3 against RustFS). Evidence files in the session scratchpad,
prefix `f4creds-live-`.

- **M1 again, the table reader:** load with the header, load with no header and `/credentials`
  all 200 with credentials; the session policy is now
  `"Action":["s3:GetObject","s3:GetObjectVersion"]` on the table prefix. LIST and GET of its own
  files allowed; PUT under its own location **403**; other tables' prefixes and the bucket root 403.
  Its access key differs from the writer's.
- **M2 again, destructive:** OVERWRITE of its own data file **403**, DELETE of a data file **403**,
  DELETE of the metadata file **403**.
- **The reader still reads:** pyiceberg scan of `f4sales.orders` as `f4tread`: 2 snapshots, 6 rows.
  Its pyiceberg append fails with `ACCESS_DENIED during CreateMultipartUpload`.
- **M4 again:** the namespace reader gets the Get-only policy on `f4sales.peer`, PUT 403, and scans
  both tables. The table writer gets the RWD policy and its pyiceberg append succeeds (9 rows); the
  namespace writer appends to `peer` (9 rows).
- **M3 again:** the setup client's view afterwards: `orders` 9 rows, `peer` 9 rows, `f4other.secret`
  6 rows. Nothing broken.
- **Trino 483 provisioned by egret** (`f4trino`, its client `engine-f4trino` holds `catalog`
  `manage`): CREATE TABLE, INSERT (3), UPDATE (1), DELETE (1), read-back, ALTER TABLE RENAME, read
  under the new name, INSERT into the pyiceberg-made `orders` (10 rows), DROP TABLE: all succeeded.
- **#757 classification move:** tag `f4-email` on `f4sales.cls.email`; Trino's
  `ALTER TABLE … RENAME TO cls_r` was held 41 s (#757 waits for the plugins to hold the moved tag) and returned; the
  governance tags API shows the tag on `cls_r.email` and nothing on `cls`; the database row is
  `f4-email|f4sales|cls_r|email`; the table reads back `(1, a@x.io)`.
- **egret's own browse** (system principal): `GET /api/control-plane/warehouses/{wh}/namespaces/f4sales/tables/orders`
  200 with the table detail.

Torn down afterwards: the Trino engine deleted through the API, `docker compose -p egret-f4creds down -v`
(no container, volume or network left), images `egret-backend:f4creds` and `egret-frontend:f4creds`
removed, BuildKit cache kept, port 18780 free.

## 10. Follow-ups (not in this package)

- **Credential lifetime.** lakekeeper's STS validity is 3600 s; a revoked writer's key, and any
  key issued before this fix, stays valid until it expires. Lowering `sts-token-validity-seconds` (e.g.
  900 s) is a separate change, to be measured against long Trino queries first (D4).
- **O2 Shared keys.** Callers with the same permission on the same table share one cached key (C7).
  Revoking one caller cannot revoke the key the others hold.
- **O4** A table grant lists every table and view name in its namespace (metadata only).
- **O5** RustFS allows `ListBuckets` with any vended key (bucket names leak), outside the session
  policy.
- **O6** lakekeeper's `parse_data_access` calls `.to_str().unwrap()` on the delegation header; a
  non-visible-ASCII value panics the handler.
- **Cross-warehouse register (review finding 1).** With two warehouses on one bucket whose storage
  connections write the endpoint differently, a namespace writer in one could register a forged
  metadata file whose location is a table of the other and get RWD on it (C8). Precondition: an admin
  set up overlapping storage. Fix options: refuse overlapping storage connections on egret's side
  (normalising the endpoint), or have the guard refuse a register whose metadata location is not
  under the warehouse's own tables.
- **Grants read twice per load (review finding 2).** The guard and the authorizer each read the
  role's grants (C6); carrying the guard's in the request would halve it.
- **Docs.** The user docs (egret-data/docs, Platform) must say that a `read` grant vends read-only
  storage credentials and that an engine attached with a read-only role can no longer write
  (docs KI-311).

## 11. Review (2026-10-27) and how each finding was handled

An independent read-only review of the four branch commits found no HIGH: no path where a caller
with only a read grant gets write- or delete-capable credentials, or a signed request, through egret's
mount.

1. **LOW/MED: C8 holds only within one warehouse.** Narrowed C8 to what was measured (one
   warehouse, pinned by `a_create_or_register_cannot_take_another_tables_location`) and recorded the
   cross-warehouse register, with its precondition (an admin set up two warehouses on one bucket with
   differently written endpoints; it predates this branch), as a follow-up in §10. No code change.
2. **LOW: the per-load query count.** C6 corrected: 4–6 egret queries per load or `/credentials`
   call (guard and authorizer each), on every load including `client-managed` ones. Reading the
   grants once per request is a follow-up in §10. No optimisation here.
3. **LOW: `ActionOnTable.user` was ignored.** The authorizer now refuses a `ReadData`/`WriteData`
   question asked on behalf of another user (`DataAsk::Refuse`, §4.5 step 0). Not reachable in-process
   (it needs trusted engines or the management API), so a unit test pins it; it failed on the code
   before the fix (red first). Commit `91ac264c`.
4. **LOW: test gaps.** Added: a grant-store error fails the decision (C12, unit); namespace read plus
   write on one table gives RWD on that table and read-only on its sibling; a view load is read-only
   for a namespace writer (C9); through lakekeeper's router with the metadata set directly, an unknown
   principal and a client of another warehouse get no data access (C4, C5; this also kills M6); the
   config test now checks only its own token's principal. Commits `e729ae9f`, `bd80dbe5`.
5. **LOW: commit ids rebased away.** The ids are explained at the top of this spec; the evidence
   applies to the rebased code (`git diff 12ba90c1 4599c6c5 -- crates/` is empty).
6. **INFO: behaviour to document.** A narrowed writer keeps its cached RWD key up to 3600 s, and the
   other writers of the table are served the same key (§10); an engine attached with a read-only role
   can no longer write (D3). The docs update (egret-data/docs, Platform; docs KI-311) is required
   before or with the merge.

**Checks after the review, on `bd80dbe5`:** `cargo fmt --all --check` pass;
`cargo clippy --workspace --all-targets -- -D warnings` pass; `cargo test -p egret-bin --bin egret
catalog_` 58 passed; `cargo test -p egret-catalog-engine --no-fail-fast` 155 passed, 0 failed; no DB
skips (against `egret_test`, `EGRET_ALLOW_DB_SKIP` unset). The full workspace gate (§8) and the live
check (§9) were not repeated: the only change to the vending decision is the refusal of on-behalf
questions, which egret cannot reach today.
