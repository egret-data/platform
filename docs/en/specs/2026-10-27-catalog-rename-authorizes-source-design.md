# The catalog guard authorizes a rename by its source and destination: design

Status: implemented, 2026-10-27. Base: `main` `4807c3e3` (#757). Branch `fix/catalog-rename-authorizes-source`.
Programme: Control Plane & DG, security defects. Origin: the grants-after-rename findings (KI-308, KI-309), item F-A
(and F-C, the same defect on views). Precedent: the guard's own per-request privilege mapping
(`crates/egret-bin/src/main.rs`, `EgretCatalogGuard::check` and `catalog_grant_covers`).

The controller set every choice here (§4, D1–D6); the user may revisit them.

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
- The other root-level writes the guard also authorizes by "any write grant" (§7).

## 6. Tests

`crates/egret-bin/src/catalog_rename_guard_tests.rs`, through `mount_catalog_engine` (the real guard, view hold and
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

- `POST /{prefix}/transactions/commit` (multi-table commit) is also a catalog-root write: any `write` grant commits to
  any table named in its body. By reading only; not measured.
- `POST /{prefix}/v1/aws/s3/sign` (the S3 request signer) is also a catalog-root write by path. By reading only; whether
  lakekeeper's signer scopes the signature to a tabular the caller could load is not checked.
- `POST /{prefix}/namespaces` (create namespace) is allowed for any role with any write grant.
- F-B: case-sensitive grants against a case-insensitive catalog (§3.4).
