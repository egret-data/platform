# SP3c — the Catalogs UI, and retiring the warehouse-shaped world

**Status:** designed
**Date:** 2026-09-22
**Extends:** `2026-09-19-catalog-registry-design.md` §5 (SP3a merged in #587, SP3b in #589)
**Delivered as two PRs:** SP3c-1 the UI, SP3c-2 the removal.

SP3a made `registry.catalogs` the source of truth; SP3b taught StarRocks to read it. Both kept
today's screens working by reimplementing the old endpoints over the registry. SP3c is where the
screens catch up, and then where the old shapes go.

## 1. What the code said that the design did not

Three things were found by reading the tree, and each changes the plan:

- **`WarehouseDetail.tsx` is 2379 lines and its tabs are not what §5 assumed.** They are Roles &
  grants, Catalog Clients, Engines, Browse, Connect and Settings — the only UI anywhere for catalog
  roles, grants, clients, namespace browsing and warehouse configuration. Deleting the page without
  relocating them deletes a management surface, not a screen.
- **`engines.warehouse_id` is load-bearing.** It decides which attachment is an engine's *primary*
  catalog (`list_engine_attachments(engine.id, Some(engine.warehouse_id))`) and seeds the engine's
  Ranger role. It is not legacy cruft to drop; "primary" has to be defined somewhere else first.
- **`warehouse:*` gates ~1550 lines of `warehouse_admin`** — roles, grants, clients, browse, config,
  protection, format policy — plus the classification resource-tag endpoints. `catalog_registry:*`
  covers none of it. And `warehouse:read` is one of eight keys in `CONTROL_PLANE_PERMS`, which
  decides whether a user may enter Control Plane **at all**: removing it carelessly locks people out
  of far more than a page.

## 2. Decisions

**D1 — One catalog detail page, with kind-conditional tabs.** Configuration, Engines and Workspaces
for every catalog; an iceberg-egret catalog additionally shows today's five warehouse tabs — the sixth, Engines, is superseded by the registry's own and is deleted, reusing those
components rather than reimplementing them. A jdbc catalog simply has fewer tabs.

**D2 — Primary becomes a flag on the attachment, in SP3c-1.** `registry.engine_catalog_attachments`
gains `is_primary boolean`, unique per engine, backfilled from `engines.warehouse_id`, and
`CreateEngineBody.warehouse_id` becomes optional so the wizard can attach several catalogs and mark
one primary. The column itself is dropped in SP3c-2, once nothing reads it. ~~SP3c-2 also owns letting a
non-warehouse catalog lead.~~ **Withdrawn 2026-09-23 (user decision): the primary attachment must name a
catalog that has a warehouse, in SP3c-2 as today** — see §4 step 3 for why that is a feature and not a
rewrite. In SP3c-1 the primary must still be an egret warehouse, because
`engines.warehouse_id` is `NOT NULL` and ten production sites read it, including `ensure_engine_role`
seeding the engine's Ranger role from the warehouse and the StarRocks path deriving its storage.
Rewriting those ten belongs in the PR that deletes the column, not in the one that rebuilds the UI
(user decision, 2026-09-22). The wizard disables that choice inline and names the reason. (An earlier draft put
all of this in SP3c-2; the wizard cannot honestly "attach catalogs" while the API demands exactly
one warehouse, so the schema half moves up — user decision, 2026-09-22.) Primary is a property of the *relationship* between an engine and a catalog, which is what it
always was — storing it on the engine is what forced one warehouse per engine.

**D3 — One permission family.** `warehouse_admin` and the resource-tag endpoints move onto
`catalog_registry:*`, and a migration grants `catalog_registry:{read,create,update,delete}` to every
role that holds the matching `warehouse:*` before the old keys are dropped. Nobody gains access,
nobody loses it. **That grant must be re-run by SP3c-2's own migration, not assumed done** (amended
2026-09-23): migration `0148` performed it once, in the past, so every role created since holds
`warehouse:*` with no `catalog_registry:*` counterpart — `warehouseRedirect.test.tsx:140-173` is an
entire describe block about exactly that population. Grant then delete, in one transaction, in that
order. **Re-gate without re-tagging**: the routes keep their paths and their OpenAPI tag; only the
`require()` string and the 403 descriptions change (§4 step 4).
`catalog_registry:read` joins `CONTROL_PLANE_PERMS` in SP3c-1, alongside the section it gates, and
`warehouse:read` is removed from it only in SP3c-2, with the permission itself. Both keys sit in the
list in between, which is the safe order: a user holding either can always enter Control Plane, and
no window exists in which neither key admits them.

**D4 — Test says what it tested.** With an attached engine, `SHOW SCHEMAS FROM <name>` through that
engine. Without one, a TCP reachability check of the configured host and port — labelled as exactly
that in the UI, because a green tick that means "the port answers" must not read as "the credentials
work". Driver-level checks arrive with SP5.

The probe does not exist yet and SP3c-1 builds it (user decision, 2026-09-22). It is a connect-only
check with a short timeout, and it takes its host and port **from the stored catalog configuration**,
never from request input — egret must not become a port scanner with a login page. Today's test
route reaches only Trino (it goes through `monitor::coordinator_call`, whose fallback service is
`{name}-trino`), so a StarRocks attachment falls back to the probe until SP4.

**D5 — The UI ships before anything is removed.** SP3c-1 leaves every old endpoint in place; SP3c-2
deletes them once nothing calls them. A user who upgrades to 3c-1 alone has a working system with
two ways to reach catalogs; a user who upgrades to both has one.

## 3. SP3c-1 — the UI

- **Control Plane → Infrastructure → Catalogs.** List: name, kind, attached engines, assigned
  workspaces, and a warning when any attachment is `failed` or stuck `pending`.
- **Create**: choose a kind, fill the typed form, optionally an Advanced tab per engine. Built from
  the existing config primitives (`ConfigSection`, `FieldLabel`, the secret picker, the
  storage-connection picker).
- **Detail**: Configuration, Engines (attach, detach, apply status, Retry), Workspaces (assign), plus
  the five existing warehouse tabs when the catalog is iceberg-egret (D1).
- **The assignment rule the UI must state.** An egret-warehouse catalog with **no** assignments is
  still visible to the workspace that owns its warehouse, and the **first** assignment revokes that
  (`egret-query/src/registry/repo.rs:529-537`). So assigning such a catalog elsewhere silently
  removes it from the owner's engine view, and unassigning everything does not hide it. Nobody wrote
  this down before SP3c-1's review found it; both halves surprise people, so the Workspaces tab says
  them where the operator is deciding.
- **`/control-plane/warehouses/:id` redirects** to the matching catalog, which is where its five tabs
  now live. The **list** route, `Warehouses.tsx` and the sidebar entry stay in SP3c-1 and retire in
  SP3c-2 with the rest of the section — D5 forbids removals here, and an earlier draft of this line
  (a glob over `warehouses/*`, with `Sidebar.tsx` named outright) contradicted it. Links by literal
  string move with the redirect; enumerate them by grep rather than from a list, because every
  written list of them so far has been wrong, including one that would have pointed the catalog page
  at itself.
- **Engine detail → Catalogs** replaces the inline `CatalogForm` with a picker over the registry,
  showing each attachment's status and offering Retry.
- **Create-engine wizard** attaches catalogs instead of choosing one warehouse.
- **Permissions**: `catalog_registry:read` gates the new section, in the route guard, the sidebar and
  `appAccess.ts`.
- **Backend, four pieces**: a registry-tagged detach route — `DELETE /catalogs/{id}/engines/{engine_id}`,
  symmetric with `/catalogs/{id}/workspaces`, because re-tagging the existing engine route would
  rename a generated function and D5 forbids removals here; `is_primary` with its backfill (D2);
  `CreateEngineBody.warehouse_id` made optional; and the TCP probe (D4).

## 4. SP3c-2 — the removal

Amended 2026-09-23, after a survey of the tree at `14de835a` (SP3a, SP3b and SP3c-1 all merged).
The survey corrected the premise of step 1, added a precondition to step 2, and turned step 3 into a
decision rather than a rewrite. It ships as **one PR** with the URLs unchanged (user decision,
2026-09-23); the steps below are its commit boundaries, not its release boundaries.

**1. `engine_mgmt.engine_catalogs` — and the boot backfill, together.** The table has **four**
production readers, not one: `egret-policy/src/store.rs:266-284`, the backfill's two scan queries
(`backfill.rs:122-127`, `:193-200`), and `unmigrated_legacy_catalog_count`
(`catalog-registry/src/store.rs:951-962`), whose non-zero count 409s every Trino apply
(`engines.rs:641-653`). `run_backfill` runs unconditionally at every boot
(`egret-bin/src/main.rs:1263`), so the table cannot be dropped while the backfill lives.

Ruling (user decision, 2026-09-23): **remove both, and make SP3a/3b/3c-1 a required upgrade hop.**
After SP3c-2 no code migrates a pre-SP3a database, so the migration must **fail loudly rather than
drop data** — it raises if `engine_mgmt.engine_catalogs` still holds a row
`registry.legacy_engine_catalogs` has not mapped, naming the hop. An operator who meets that error
upgrades through SP3c-1 and boots once.

This also settles the policy reader without adding anything to the registry. Its legacy `UNION` arm
exists for two populations (`egret-policy/src/store.rs:220-232`): engines whose backfill failed, and
the rename window. The first ceases to exist when the backfill does. The second is **already stale** —
`registry.catalogs.name` is `UNIQUE` and immutable (`0147:11-12`; `CreateRegistryCatalogBody.name`
documented "immutable"; `UpdateRegistryCatalogBody` carries no name; `RegistryStore::update_catalog`
says "kind and name never change"), and the only renamer left is the backfill's own suffix pass. So
both legacy arms — `tag_assignments_for_service` and `bump_bound_tag_versions`
(`tag_version.rs:61-64`) — delete outright. Dead with them: `legacy.rs` (664 lines, whose last
consumer is the backfill), `map_legacy_row_on`, `registry.legacy_engine_catalogs`, the apply guard,
the five dead store methods, and eleven DB test files.

**2. The legacy `/engines/{id}/catalogs*` endpoints — after `configError` moves.** Five of the six
routes already have no caller anywhere; only `GET` has one, and only as the `configError` carrier, so
the stated order is right. Two corrections:

- **The move must degrade, never propagate.** `to_dto` (`catalogs.rs:237-269`) sets `config` from the
  raw column and parses nothing. Adding a `row.def()` call per row makes one stored row the tightened
  validator refuses able to fail the **whole list** — precisely the defect SP3c-1's review N1 fixed.
  Record the `Err` in a new `config_error` field; never `?`.
- **A carve-out loses its channel.** `catalog_apply.rs:1265-1272` deliberately does *not* write
  `apply_status = failed` for `ApplyError::Build`, on the written ground that the reason "is reported
  independently as `configError` on `GET /engines/{id}/catalogs`" — one of the routes being deleted.
  Move the field first and the channel survives; delete first and a Trino build failure is invisible
  on **both** surfaces. This was not in the design and is now part of the precondition.

Deleting the routes also retires `CatalogTestDto`'s undiscriminated `{ ok, message }`, which is what
makes D4 true everywhere rather than on one route. Note that `POST /engines/{id}/catalogs/{cid}` and
`…/apply` live under the same URL prefix but are **registry-era and stay**.

**3. `engines.warehouse_id` — and what "primary" may now mean.** One writer (`store.rs:203-227`),
eleven production readers; four of them die with step 1. `EngineDto.warehouse_id` leaves the wire,
which deletes two frontend echo lines (`useEngineConfigForm.ts:1291`,
`useStarRocksConfigForm.ts:506`) the server already ignores on `PUT`.

Two readers need an answer rather than a rewrite. StarRocks derives its builtin storage volume from
the warehouse (`engines.rs:626-635`, `:4977-4983`), and `ensure_engine_role` / `create_client` seed
the engine's Ranger role and Catalog Client from it (`engines.rs:2899-2905`, `:3073`, `:3800`). A
`jdbc` primary has `warehouse_id IS NULL` and answers neither.

Ruling (user decision, 2026-09-23): **the primary attachment must name a catalog that has a
warehouse.** The backend refuses `is_primary` on a catalog without one, and says why. That is today's
behaviour preserved, which keeps SP3c-2 a removal; letting a non-warehouse catalog lead is a feature,
and a PR that deletes some 2 500 lines is the worst place to introduce the one thing that breaks a
live provision. It becomes its own item after SP4.

Two consequences to write down, because nobody had:

- `EngineBinding.warehouse_id` (`egret-bin/src/main.rs:286`) feeds the Ranger policy-editor
  autocomplete (`egret-policy/src/admin/mod.rs:372-396`). An engine now has *N* warehouses — one per
  attached egret catalog — so it becomes `warehouse_ids: Vec<Uuid>`, and the consumer already dedupes
  into a `Vec`. This **widens** the picker: an engine attaching three egret catalogs will
  autocomplete all three warehouses' schemas. That is the right answer, and it is a behaviour change.
- Migration `0150`'s partial unique index enforces **at most one** primary, not exactly one. Dropping
  `engines.warehouse_id` removes the last `NOT NULL` anchor for "an engine has a primary", leaving
  `resolve_primary_catalog` as the sole enforcement point.

**4. `warehouse:*` — re-gate without re-tagging.** Thirty `require()` sites: 27 in
`catalog-engine/src/warehouse_admin/mod.rs`, 3 in `egret-bi/src/catalog_tags.rs`. The migration
re-runs D3's grant and then deletes the four keys in one transaction, and the four `perm!` entries
leave `egret-core/src/rbac.rs:82-104`.

The URLs do not move (user decision, 2026-09-23): `/control-plane/warehouses/{id}/…` keeps its paths
**and its OpenAPI tag**. Only the `require()` string and the 403 descriptions change. Re-tagging would
rename some forty generated functions across `api/controlPlane.ts:209-480` and break three e2e specs
plus the hand-written URLs in `api/catalogTags.ts`, for nothing a user can see. The path parameter
stays a warehouse id, which the frontend still holds as `RegistryCatalogDto.warehouseId`.

**5. The Warehouses UI section.** Two traps a grep for `/control-plane/warehouses` does not find:

- `CatalogCreate.tsx:28` imports `warehousesQueryKey` **from `"../Warehouses"`**, not from
  `"../queryKeys"` where it is defined. Deleting the page is a compile break until that import moves.
- `AddWarehouseDialog` (`Warehouses.tsx:258-401`) is the **only** create-warehouse UI in the tree.
  The section may not be deleted until `CatalogCreate` can create an egret-warehouse catalog end to
  end — verified in a browser, not inferred.

`WarehouseRedirect` **stays**, re-gated, as an unlisted compatibility route: it already answers the
warehouse-id → catalog-id lookup that `WorkspaceDetail.tsx:409` and `StorageDetail.tsx:546` would
otherwise each have to grow, and it keeps a bookmarked URL working. `WorkspaceDetail.tsx:377`'s
`?workspace=` link re-points at `Catalogs.tsx`, which already supports that filter server-side. The
list route, the sidebar entry, `Warehouses.tsx` and its two tests go.

**Ordering and risk.** Steps 1, 2, 4 and 5 are mechanical once their preconditions hold. Step 3 is
not: it travels as its own commit with its own live check, and the PR's final gate **provisions both
a Trino and a StarRocks engine on kind**, because the storage derivation and the Ranger role seeding
are where a mistake becomes a broken cluster rather than a failing test.

## 5. Testing

- Vitest for the new pages, including the kind-conditional tabs, the attach picker, the wizard's
  multi-select, and a redirect test per moved path.
- A permission test that a user holding only `catalog_registry:read` can enter Control Plane and see
  Catalogs — the lock-out this design exists to avoid.
- A migration test that every role holding `warehouse:*` ends with the matching `catalog_registry:*`
  and no more — seeded with a role created **after** `0148`, since that is the population the re-run
  exists for (§4 step 4). No such test exists today.
- A migration test that SP3c-2 **refuses** a database with an unmapped `engine_mgmt.engine_catalogs`
  row, rather than dropping it (§4 step 1). A migration that silently succeeds on data it cannot
  carry forward is the failure mode this guard exists to make impossible.
- A test that `GET /catalogs` still lists every row when one row's stored config no longer parses,
  and reports that row's `configError` — the rewritten home of
  `one_unparseable_catalog_does_not_take_an_engines_whole_list_with_it`, which moves rather than dies.
- A backend test that attaching `is_primary` to a catalog with no warehouse is refused, and says why.
- Live on kind, reported as manual and **provisioning both engine types**: a Trino engine and a
  StarRocks engine, each provisioned after the column is gone, each reaching `HEALTHY`, with the
  StarRocks storage volume and the engine's Ranger role both verified against the primary
  attachment's catalog rather than against the dropped column.
- `provisioning-ui-kind.spec.ts` is **already stale** — it expects a dialog that became a full-page
  wizard — so SP3c-1 rewrites it rather than inheriting a broken baseline.
- Live on kind, reported as manual: create a catalog in the UI, attach it to an engine, watch the
  status settle, assign it to a workspace, and confirm the workspace sees it.

## 6. Out of scope

Per-catalog Ranger policy generation and the egret permission model (SP4). Egret-enforced engines
(SP5). Notebook selection by assignment, which needs SP4's per-workspace notebook role. The
`SetNotebookWarehouse` endpoint appears to have no UI caller at all; that is noted for SP4 rather
than removed here on a guess.
