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
for every catalog; an iceberg-egret catalog additionally shows today's warehouse tabs, reusing those
components rather than reimplementing them. A jdbc catalog simply has fewer tabs.

**D2 — Primary becomes a flag on the attachment, in SP3c-1.** `registry.engine_catalog_attachments`
gains `is_primary boolean`, unique per engine, backfilled from `engines.warehouse_id`, and
`CreateEngineBody.warehouse_id` becomes optional so the wizard can attach several catalogs and mark
one primary. The column itself is dropped in SP3c-2, once nothing reads it — and **SP3c-2 also owns letting a
non-warehouse catalog lead**. In SP3c-1 the primary must still be an egret warehouse, because
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
nobody loses it. `catalog_registry:read` joins `CONTROL_PLANE_PERMS` in SP3c-1, alongside the section it gates, and
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
  the six existing warehouse tabs when the catalog is iceberg-egret (D1).
- **`/control-plane/warehouses/*` redirects** to the matching catalog. Three places link to that path
  by literal string — `WorkspaceDetail.tsx`, `StorageDetail.tsx`, `Sidebar.tsx` — and move with it.
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

1. `engine_mgmt.engine_catalogs`: no production writer remains. Move `egret-policy`'s legacy read
   onto the registry, then drop the table and the five dead store methods.
2. The legacy `/engines/{id}/catalogs*` endpoints, once the new UI is the only caller.
3. `engines.warehouse_id`, whose `is_primary` replacement landed in SP3c-1.
4. `warehouse:*`, after D3's grant migration and the `warehouse_admin` re-gating.

Each step is independently revertible, and each lands with its own tests.

## 5. Testing

- Vitest for the new pages, including the kind-conditional tabs, the attach picker, the wizard's
  multi-select, and a redirect test per moved path.
- A permission test that a user holding only `catalog_registry:read` can enter Control Plane and see
  Catalogs — the lock-out this design exists to avoid.
- A migration test that every role holding `warehouse:*` ends with the matching `catalog_registry:*`
  and no more.
- `provisioning-ui-kind.spec.ts` is **already stale** — it expects a dialog that became a full-page
  wizard — so SP3c-1 rewrites it rather than inheriting a broken baseline.
- Live on kind, reported as manual: create a catalog in the UI, attach it to an engine, watch the
  status settle, assign it to a workspace, and confirm the workspace sees it.

## 6. Out of scope

Per-catalog Ranger policy generation and the egret permission model (SP4). Egret-enforced engines
(SP5). Notebook selection by assignment, which needs SP4's per-workspace notebook role. The
`SetNotebookWarehouse` endpoint appears to have no UI caller at all; that is noted for SP4 rather
than removed here on a guess.
