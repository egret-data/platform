/**
 * The two list caches that describe the same rows, while D5 keeps both sections
 * live.
 *
 * Through SP3c-1 a user can reach an egret warehouse from Control Plane →
 * Catalogs **and** from Control Plane → Warehouses, and either page can delete
 * it: the Catalogs list routes an egret-warehouse row to `deleteWarehouse`,
 * which is the very endpoint the Warehouses list calls. A page that invalidates
 * only its own key therefore leaves the OTHER page showing a row that no longer
 * exists — for the 30 seconds of `queryClient`'s `staleTime`, which is long
 * enough to click the sidebar. Both pages invalidate both keys.
 *
 * The keys live here rather than in the two pages because each page needs the
 * other's: exporting them from the pages themselves would make
 * `Warehouses.tsx` and `catalogs/Catalogs.tsx` import each other, and a
 * module cycle that happens to work only because both reads sit inside function
 * bodies is not a thing to leave for the next reader. Both pages re-export
 * their own key, so every existing import path still resolves.
 *
 * D5 keeps the warehouse half alive through SP3c-1: only the warehouse
 * DETAIL route became a redirect, so the Warehouses list is still a writer of
 * both keys and SP3c-2 is what retires it. `warehousesQueryKey` gained a third
 * reader in Task 8 — `CatalogDetail` keys its single-warehouse fetch UNDER it
 * (`[...warehousesQueryKey, id]`), so the invalidations that already exist
 * reach that row too.
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
