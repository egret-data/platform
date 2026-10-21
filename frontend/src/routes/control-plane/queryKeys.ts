/**
 * Two keys for one underlying row, invalidated together by both places that
 * create or delete an egret-warehouse catalog.
 *
 * SP3c-2 (Task 8) deleted the Warehouses LIST page, so the original reason for
 * this pair — two separate full-list pages of the same rows, either of which
 * could delete one and leave the other stale — no longer applies. The pair
 * stays because each key still has its own live reader that is NOT the other
 * key's writer: `catalogsQueryKey` is `catalogs/Catalogs.tsx`'s own list, and
 * `warehousesQueryKey` is read by `EngineCreate.tsx` (the attach-a-warehouse
 * list, and the "provisioned but not registered" check built from it) and by
 * `CatalogDetail.tsx`'s single-warehouse fetch, keyed UNDER it
 * (`[...warehousesQueryKey, id]`) so this invalidation reaches that row too.
 * Dropping either half would leave one of those readers stale for the 30
 * seconds of `queryClient`'s `staleTime` after a create or delete elsewhere —
 * the same bug this pair always existed to prevent, just with a narrower cast
 * of readers now that `Warehouses.tsx` is gone.
 *
 * The keys live here, not in the two writers (`catalogs/CatalogCreate.tsx`'s
 * warehouse branch and `catalogs/Catalogs.tsx`'s delete), because
 * `catalogs/Catalogs.tsx` needs `warehousesQueryKey` and `CatalogCreate.tsx`
 * needs both — importing across those files would risk a module cycle that
 * happens to work only because every read sits inside a function body.
 * `catalogs/Catalogs.tsx` re-exports `catalogsQueryKey` so existing import
 * paths still resolve.
 */
import { getControlPlaneCatalogRegistryGetRegistryCatalogQueryKey } from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";

/** The hand-written `listWarehouses` query (`api/controlPlane.ts`). */
export const warehousesQueryKey = ["control-plane", "warehouses"] as const;

/**
 * The GENERATED registry list query's key prefix — the value of
 * `getControlPlaneCatalogRegistryListRegistryCatalogsQueryKey()`, not a
 * hand-picked `["control-plane", "catalogs"]` pair.
 *
 * The catalog rows come from the generated hook, so only its own key refreshes
 * them; the `?workspace=` variant appends its params to this same first
 * element, so invalidating on this prefix catches every variant.
 * `catalogs/__tests__/Catalogs.test.tsx` pins the literal to the builder's
 * output through `vi.importActual` — a drift here would silently leave a
 * deleted row on screen with every other test still green.
 */
export const catalogsQueryKey = ["/api/control-plane/catalogs"] as const;

/**
 * One catalog's detail query, for everything on `CatalogDetail` that changes
 * it — the Configuration save and the Workspaces save, each in its own file.
 *
 * It DELEGATES to the generated builder instead of restating its value, so no
 * drift is representable. The key is the request URL,
 * `["/api/control-plane/catalogs/<id>"]`; a hand-written
 * `["control-plane","catalogs",id]` triple — which is what this repo's older
 * hand-rolled queries look like — matches no cache entry at all, so a saved
 * catalog would stay on screen exactly as it was with every test still green.
 * That is the defect Task 4's review found on the list page.
 *
 * It lives HERE rather than in `CatalogDetail.tsx` for the same reason the two
 * list keys do: `ConfigurationTab` and `WorkspacesTab` both need it, and
 * importing it from the page that renders them is a module cycle that happens
 * to work only because every read sits inside a function body.
 * `CatalogDetail.tsx` re-exports it.
 */
export const catalogKey = (id: string) =>
  getControlPlaneCatalogRegistryGetRegistryCatalogQueryKey(id);

/**
 * One engine's detail query (`getEngine`, read by `EngineDetail.tsx`), for the
 * writers outside that page that change the row. The value is the hand-rolled
 * `["cp", "engine", id]` triple the page has always used; it is named here so a
 * new writer cannot misspell it.
 */
export const engineDetailKey = (id: string) => ["cp", "engine", id] as const;
