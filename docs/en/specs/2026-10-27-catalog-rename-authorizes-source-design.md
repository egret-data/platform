# The catalog guard authorizes a rename by its source and destination: design

Status: implemented, 2026-10-27. Base: `main` `4807c3e3` (#757). Branch `fix/catalog-rename-authorizes-source`.
Programme: Control Plane & DG, security defects. Origin: the grants-after-rename findings (KI-308, KI-309), item F-A
(and F-C, the same defect on views). Precedent: the guard's own per-request privilege mapping
(`crates/egret-bin/src/main.rs`, `EgretCatalogGuard::check` and `catalog_grant_covers`).

The controller set every choice here (§4, D1–D6; §8.4, D7–D9); the user may revisit them. §8 extends the package,
at the user's request (2026-10-27), to the multi-table commit, and §9 to the S3 signer, the sibling holes of §7.

## 1. Problem

The embedded catalog's guard decides every Iceberg request from its method and path. It reads a target namespace only
when the path's second segment is `namespaces` (E2). A rename is `POST /{prefix}/tables/rename` (or `/views/rename`),
with the source and destination in the JSON body (E3). The guard never reads that body, so it authorizes the rename as
a **catalog-root write**: any role holding **any** `write` or `manage` grant anywhere in the warehouse may rename any
table or view (E2).

Catalog grants are keyed by name (E4). A role can therefore rename a table it cannot read onto a name it holds a grant
on, and then read it:

- a `table` grant with `write` on `sales.loot` (a name that does not exist yet) renames `sales.secret` onto
  `sales.loot`, after which the same grant reads it;
- a `namespace` grant with `write` on `stage` moves `sales.secret` into `stage`, after which the grant reads it.

Measured at `4807c3e3` through lakekeeper's real router and the real guard (§6, the two premise tests): the rename
answered `204`, then the load under the new name answered `200`, for a role whose load of the source answered `403`.

## 2. Evidence (at `4807c3e3`)

`MAIN` = `crates/egret-bin/src/main.rs`, `MOUNT` = `crates/egret-catalog-engine/src/mount.rs`, `LK/` = the git-pinned
lakekeeper checkout (rev `791650571d`), under `crates/lakekeeper/src/api/iceberg/v1/`.

| id | fact | where |
|---|---|---|
| E1 | The guard sees `method`, `path`, `query` and the bearer only; `guard_then_inject_metadata` passes nothing else | `MOUNT:245-255`, `MOUNT:326-375` |
| E2 | `target_ns` is set only when `segs[1] == "namespaces"`; with no target, a write is allowed when any grant has `write`/`manage` | `MAIN:614-640` |
| E3 | lakekeeper's rename routes are `/{prefix}/tables/rename` and `/{prefix}/views/rename`; the body is `{source, destination}`, each `{namespace: [..], name}` | `LK/tables.rs:564`, `LK/views.rs:298`; `rename_of` in `crates/egret-catalog-engine/src/view_hold.rs` |
| E4 | `catalog.catalog_grants` stores `securable_type`, `namespace text[]`, `table_name`, `privilege`; matching is exact, case-sensitive, with no wildcards; a `table` grant also covers `views/{name}` | `MAIN:666-697` (`catalog_grant_covers`) |
| E5 | The per-operation mapping: a drop (`DELETE …/tables/{t}`) needs a `write`/`manage` grant covering `(ns, t)`, i.e. `catalog`, `namespace = ns` or `table = (ns, t)`; a create (`POST …/namespaces/{ns}/tables`) needs a `write`/`manage` grant covering the namespace with no table, i.e. `catalog` or `namespace = ns`; a `table` grant never creates | `MAIN:666-697` |
| E6 | An engine's catalog client is given a role with one grant: `catalog`, `manage` | `crates/egret-engine-mgmt/src/engines.rs:2775-2790` |
| E7 | Behind the guard, the view hold and the table hook (#757) each buffer the body (16 MiB cap) and act on renames | `crates/egret-catalog-engine/src/view_hold.rs:2120-2180`; `crates/egret-catalog-engine/src/table_hook.rs` (rename arm) |
| E8 | Every catalog token is minted for a catalog client; there is no platform-admin bypass in the guard, and the catalog admin API (`crates/egret-catalog-engine/src/warehouse_admin/`) has no rename route | `MAIN:512-645` |

## 3. Design

### 3.1 The mount reads the body for a rename

`CatalogGuard::check` gains a `body: Option<&[u8]>` argument. `guard_then_inject_metadata` buffers the body only for
`POST /{prefix}/tables/rename` and `POST /{prefix}/views/rename` (`is_rename_request`), passes it to the guard, and puts
the same bytes back on the request, so the view hold, the table hook and lakekeeper read exactly what the guard read.
Every other request streams through untouched, with `None`. The cap is 1 MiB: a rename body is a few hundred bytes. A
body over the cap or that fails to read is a 400.

### 3.2 The privilege model

The guard parses the body and checks two things, both before anything is forwarded:

- **Source:** the privilege a **drop** of the source needs (E5): a `write`/`manage` grant covering `(source namespace,
  source name)`, so `catalog`, `namespace` on the source namespace, or `table` on the source name.
- **Destination:** the privilege a **create** in the destination namespace needs (E5): a `write`/`manage` grant
  covering the destination namespace with no table, so `catalog` or `namespace` on the destination namespace.

Why: a rename is, to every observer, a drop of the source name and a create of the destination name in one step.
Requiring less than drop on the source lets a role remove a name it could not remove; requiring less than create on
the destination lets a role create a table where it cannot create one, which is exactly the escalation (a name-keyed
grant pointing at the new table). `write` includes read, so no separate read check is needed.

### 3.3 Views

`/views/rename` gets the same treatment, with the same mapping: a view drop needs a grant covering `(ns, view)` and a
view create needs a namespace-level grant (the existing mapping already treats `views/{name}` like `tables/{name}`, E4).

### 3.4 Case sensitivity (F-B)

The check matches grants exactly as every other guard decision does: namespace arrays element by element, names byte
for byte. Lakekeeper resolves names case-insensitively; egret's grants do not. The consequence is fail-closed: a grant
on `Sales.Orders` does not authorize renaming `sales.orders`, as it does not authorize loading it. Folding case here
alone would make a rename more permissive than a load of the same table; folding belongs in `catalog_grant_covers`
for every route, as one change (left open, §7).

### 3.5 Multi-level namespaces

The body carries the namespace as a JSON array; the guard compares it to the grant's `namespace text[]` level by
level. A grant on `corp` covers neither `corp.fin` nor anything in it, as on every other route. No `\u{1f}` joining is
involved.

### 3.6 Answers

- 403 when the source is not covered: `role '<r>' may not rename <ns>.<name>: renaming needs write on the source, as
  dropping it would`.
- 403 when the destination is not covered: `role '<r>' may not rename into <ns>.<name>: renaming needs write on
  namespace <ns>, as creating a table there would`.
- 400 for a body that is not a rename: `malformed rename request: <what>` (not JSON; no `source` or `destination`; a
  `namespace` that is not a non-empty list of strings; a `name` that is not a non-empty string).

Namespaces print with their levels joined by `.`. The message says nothing about whether the source exists: the check
runs before lakekeeper, on names only.

### 3.7 What keeps working

- An engine's catalog client (`catalog`, `manage`, E6) renames anything, tables and views.
- A role with `write`/`manage` on both namespaces (or `table` on the source plus `namespace` on the destination's)
  renames.
- #757's table hook and the view hold run behind the guard on the re-injected body; their rename handling is
  unchanged.
- There is no platform-admin path through this guard, and no admin rename route elsewhere (E8).

## 4. Controller defaults

| id | choice | reason |
|---|---|---|
| D1 | Source needs drop's privilege (write on the table) | a rename removes the source name; mirroring drop keeps one rule per effect |
| D2 | Destination needs create's privilege (namespace-level write); a `table` grant on the destination name is **not** enough | a `table` grant cannot create a table today; allowing it here would reopen the escalation for any role that holds write on some table and a grant on a future name |
| D3 | The mount buffers only the two rename routes | no cost or behaviour change for any other request; a large commit body still streams |
| D4 | Exact, case-sensitive matching (F-B unchanged) | consistent with every other route; fails closed |
| D5 | Malformed body: 400 from the guard, before lakekeeper | the guard cannot authorize what it cannot read; refusing early keeps the hooks from seeing it |
| D6 | Body cap 1 MiB | a rename body is tiny; the cap bounds what the guard buffers |

## 5. Not in scope

- Moving catalog grants, `perm.grants` and rules with a rename (KI-309; the grants-after-rename package).
- The other root-level writes the guard authorizes by "any write grant" (§7), except the multi-table commit (§8).

## 6. Tests

`crates/egret-bin/src/catalog_guard_body_tests.rs`, through `mount_catalog_engine` (the real guard, view hold and
table hook in front of lakekeeper's router, on an in-memory warehouse):

- premise: `a_table_grant_on_the_destination_does_not_rename_an_unreadable_table`,
  `a_write_grant_elsewhere_does_not_move_an_unreadable_table_into_it`;
- source and destination: `a_read_grant_on_the_source_is_not_enough`, `an_unauthorized_destination_is_refused`,
  `a_table_grant_on_the_destination_name_does_not_create_it`;
- views: `a_view_rename_is_authorized_by_its_source_and_destination`;
- multi-level: `a_multi_level_namespace_matches_exactly`;
- malformed: `a_malformed_rename_body_is_a_400_from_the_guard`;
- still working: `an_authorized_rename_still_renames`, `an_engine_client_with_warehouse_manage_still_renames`.

Each premise test failed at `4807c3e3` (rename `204`, then the load `200`), as did every refusal test; the two
still-working tests passed before and after. Mutants killed by these tests: the source check removed, the destination
check removed, a destination table grant accepted, a read grant accepted on the source, the body not put back, the
rename not recognised (the old root-op path), an empty namespace accepted.

`table_hook_db`'s and `view_hold_db`'s stub guards now sit in front of their renames, so #757's classification move and
the view hold's rename run on the body the mount re-injected.

## 7. Related defects found, not fixed

- `POST /{prefix}/transactions/commit` (multi-table commit) was also a catalog-root write. Fixed in §8.
- `POST /{prefix}/v1/aws/s3/sign` (the S3 request signer) is also a catalog-root write by path. Not exploitable on an
  egret warehouse, because remote signing is off; pinned, not changed (§9).
- `POST /{prefix}/namespaces` (create namespace) is allowed for any role with any write grant, because its path has no
  namespace segment and the namespace is named only in the body. Creating one grants the creator nothing: grants are
  exact, so a new namespace is reachable only through a `catalog` grant or a grant an admin adds on it. The harm is
  squatting a name and clutter. A narrow fix is obvious and small: hand the guard the body (`{"namespace": [..]}`) as
  for a rename and require a `write`/`manage` `catalog` grant for a top-level namespace, or a `namespace` grant on the
  parent for a nested one. One choice is open: whether a grant on the parent may create children (the parent grant
  already "covers everything in it" for tables).
- F-B: catalog grants are case-sensitive while lakekeeper's names are not (§3.4). It fails closed everywhere: a grant on
  `Sales.Orders` neither loads nor renames nor commits to `sales.orders`. The fix is one change in
  `catalog_grant_covers` (fold both sides, as lakekeeper does), for every route at once, with a migration that flags
  grants differing only in case. Not done here.

## 8. The multi-table commit

### 8.1 Problem and evidence

`POST /{prefix}/transactions/commit` names the tables it changes only in its body (`table-changes[].identifier`,
`LK/tables.rs:581`). Its path has no `namespaces` segment, so the guard authorized it as a catalog-root write, exactly
like the rename (E2): any role with any `write`/`manage` grant could commit any change to any table.

Measured at `7b965597` through the real mount: a role whose only grant is `table` `write` on `sales.mine` committed
`set-properties` to `sales.theirs`. The commit answered **200**, and lakekeeper then held the new property on
`sales.theirs` (`a_multi_table_commit_to_an_unwritable_table_is_refused`). A set-properties update is the harmless
probe; the same request carries `add-snapshot`, `set-location`, `remove-snapshots` and schema changes.

### 8.2 Privilege model

For **every** table in `table-changes`, the guard requires what a single-table update (`POST
…/namespaces/{ns}/tables/{t}`) of that table requires: a `write`/`manage` grant covering `(ns, t)`, so `catalog`,
`namespace` on `ns`, or `table` on `(ns, t)` (E5). The same `catalog_grant_covers` call decides both routes, so they
cannot drift. Namespaces match exactly and level by level (§3.4, §3.5).

### 8.3 Body, answers, hooks

- The mount hands the guard the body (`GuardedBody::MultiTableCommit`) and puts the same bytes back, as for a rename
  (§3.1). #757's table hook reads this body too; its multi-table tests run behind a stub guard that reads it.
- The first table not covered → 403, before anything is forwarded: `role '<r>' may not commit to <ns>.<name>: a
  multi-table commit needs write on every table it changes, as updating it alone would; nothing was committed`.
  The commit is atomic in lakekeeper, and the guard refuses it whole, so a covered table in the same commit is not
  changed either.
- 400, `malformed multi-table commit: <what>`: not JSON; `table-changes` missing, not a list, or empty; an entry
  with no `identifier`; an identifier whose `namespace` is not a non-empty list of strings or whose `name` is not a
  non-empty string.

### 8.4 Controller defaults

| id | choice | reason |
|---|---|---|
| D7 | Every table needs single-table update's privilege; one miss refuses the whole commit | the commit is all-or-nothing in lakekeeper; forwarding a part would change its meaning |
| D8 | Body cap 16 MiB | a commit carries whole schemas and snapshot lists; the table hook behind the guard already buffers the same body to 16 MiB, so this adds no new memory bound and refuses nothing the hook accepts |
| D9 | An empty `table-changes`, or an entry with no `identifier`, is a 400 | an entry with no identifier names no table the guard can authorize; an empty commit authorizes nothing |

### 8.5 Tests

In `crates/egret-bin/src/catalog_guard_body_tests.rs`:

- red at `7b965597`, green now: `a_multi_table_commit_to_an_unwritable_table_is_refused` (the premise),
  `one_unwritable_table_refuses_the_whole_commit`, `a_multi_table_commit_matches_namespaces_exactly`,
  `a_malformed_multi_table_commit_is_a_400_from_the_guard`;
- green before and after: `an_authorized_multi_table_commit_still_commits` (table and namespace grants, an engine with
  warehouse manage, a multi-level namespace).

## 9. The S3 signer

### 9.1 What lakekeeper checks (pinned lakekeeper, `LK/` = `crates/lakekeeper/src/` at `791650571d`)

| id | fact | where |
|---|---|---|
| E9 | Three routes sign: `/aws/s3/sign`, `/{prefix}/v1/aws/s3/sign`, `/signer/{prefix}/tabular-id/{id}/v1/aws/s3/sign`; the `s3-signer` feature is in lakekeeper's defaults, which egret takes | `LK/api/iceberg/v1/s3_signer.rs:34-75`; `crates/lakekeeper/Cargo.toml:19-22` |
| E10 | The guard refuses the first and the third (their first segment is not the warehouse prefix) and lets the second through as a catalog-root write: any role with any `write`/`manage` grant | `MAIN` guard prefix check; E2 |
| E11 | The signer refuses a warehouse whose S3 profile has `remote_signing_enabled = false` with 403 `RemoteSigningDisabled`, and a non-S3 profile with 400, **before** it parses the location or looks up a table | `LK/server/s3_signer/sign.rs:89-104` |
| E12 | With signing on, it maps the signed location to a table (`get_table_info_by_location`), then asks the authorizer for `ReadData` (GET/HEAD) or `WriteData` (PUT/POST/DELETE) on that table, then checks the URI lies under the table's location | `LK/server/s3_signer/sign.rs:131-231`, `:447` |
| E13 | egret runs lakekeeper on `AllowAllAuthorizer`, so E12's table check allows every caller | `crates/egret-catalog-engine/src/mount.rs:74` (`EngineState`) |
| E14 | egret creates every S3 warehouse with `remote-signing-enabled: false`, and no egret route changes a storage profile (the admin API changes only the credential) | `crates/egret-catalog-engine/src/warehouse.rs` (`s3_storage_profile`); `crates/egret-catalog-engine/src/admin.rs:188` |

### 9.2 Measured

Through the real mount, with a role whose only grant is `namespace` `write` on `stage`, against a table `sales.secret`
(a one-off probe, not committed; lakekeeper's warehouse cache turned off for the run with
`LAKEKEEPER__CACHE__WAREHOUSE__ENABLED=false` so a profile written to its table took effect):

- the in-memory profile: GET and PUT both 400 `UnsupportedStorageType`;
- egret's S3 profile (signing off): GET and PUT both 403 `RemoteSigningDisabled`;
- the same profile with signing **on**, and the table's location moved to `s3://…`: lakekeeper accepted the role for
  `sales.secret`, matched the URI to the table's location, and stopped only at 412 `SignWithoutCredentials` because the
  test warehouse has no S3 key. With a key it would have signed: lakekeeper does not tie the signature to anything the
  caller may read (E12, E13).

### 9.3 Decision (controller default D10)

**No guard change.** The signer is limited on every egret warehouse: signing is off (E14), and lakekeeper refuses
before any table lookup (E11). What is fixed is that the limit is now explicit and pinned:

- `s3_storage_profile` (extracted from `create_lakekeeper_warehouse`, same JSON) documents that signing off is
  load-bearing, and a unit test, `egret_s3_warehouses_have_remote_signing_off`, fails if it is turned on;
- `the_s3_signer_gives_no_signature_on_an_egret_warehouse` drives the real mount on a warehouse with egret's S3 profile:
  a role with write elsewhere and an engine with warehouse manage both get 403 `RemoteSigningDisabled` for a GET and a
  PUT, and the table-specific signer route is refused by the guard.

If remote signing is ever wanted, the guard must first authorize the signed location itself: map it to a table
through lakekeeper's tabular locations (the lookup E12 already does, one indexed query per signed request) and require
read for GET/HEAD and write for PUT/POST/DELETE on that table, with an unmappable location refused. That check is not
built now, because no egret warehouse can reach it.

Both tests passed before this change too: they pin the present behaviour. Mutation: turning signing on in
`s3_storage_profile` fails both.

## 10. Review findings (2026-10-27)

An independent review of this branch found the guard holes below, mostly on routes next to the ones §1–§9 fixed. The
user's decision: close the whole class here. Each subsection says what was measured first (a red test through the real
mount, seen failing on the code before the fix), what was decided, and how it is tested. F4 is recorded only (§10.7).

### 10.1 F8: test stubs, duplicate keys, `+` and `%` in names

- The stub guards in `table_hook_db` and `view_hold_db` decided which requests carry a body with
  `path.ends_with("/tables/rename")`, which drifts from the mount (a single-table commit to a table named `rename` ends
  with that suffix). They now ask `GuardedBody::of`, the function the mount uses.
- A duplicate key cannot make the guard decide on one copy while lakekeeper runs the other: lakekeeper deserializes
  rename, commit and identifier bodies with plain `serde` derives, which refuse a duplicate field.
  `a_duplicate_key_body_renames_nothing` pins it both ways round (the unauthorized copy first, then last): nothing is
  renamed. It passed before this change too.
- Names containing `+` or `%`: lakekeeper URL-decodes path segments and maps `+` to a space; the guard compared raw
  segments. That fails closed (a grant on `a b` is not reached by `a+b`), except for a grant whose own name contains
  `+` or `%xx`. §10.4 makes the guard decode path segments as lakekeeper does, so this note is closed there.

### 10.2 F2: a guarded body was read before the token was verified (introduced by §3.1 and §8.3)

- **Problem.** The mount read the body of a rename (1 MiB) or a multi-table commit (16 MiB) before `check` ran, so
  before the token was verified. Anyone who could reach `/catalog/v1` could make it buffer 16 MiB per request, with no
  token and any prefix, and hold the connection while trickling the body.
- **Measured** at `b1d85fee`: an unauthenticated `POST /{prefix}/transactions/commit` whose body yields 64 KiB and
  then never ends got no answer within 5 s (`an_unauthenticated_body_is_not_read`); the same with a valid token and a
  declared `Content-Length` over the cap also hung (`a_declared_length_over_the_cap_is_a_413_unread`); a streamed body
  over the cap got 400, not 413 (`a_streamed_body_over_the_cap_is_a_413`).
- **Decision.**
  - `CatalogGuard` gains `authenticate(bearer)` (token, expiry, audience). The mount calls it before it reads a
    guarded body; `check` still decides the request in full afterwards (the token is verified twice on these routes:
    one key read and one signature check).
  - A declared `Content-Length` over the cap is a **413** before the body is read. A streamed body that crosses the
    cap is a **413** as soon as it does (`http_body_util::LengthLimitError`). Any other read failure is a 400.
  - `authenticate` is a required trait method, so a new guard cannot forget it.
- **Tests:** the three above, red before, green now. The unauthenticated test asserts that the server pulled **0
  bytes** from the body stream (a counting stream), which is how "refused without being buffered" is observed.
