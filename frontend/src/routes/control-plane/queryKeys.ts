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
 * Task 8 deletes the warehouse half of this when the section becomes a
 * redirect, and this module goes with it.
 */

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
