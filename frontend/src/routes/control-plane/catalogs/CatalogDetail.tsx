import { useEffect, useState } from "react";
import { Link, useParams, useSearchParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import {
  ArrowLeft,
  Boxes,
  Building2,
  Cable,
  Clock,
  Cpu,
  Database,
  KeyRound,
  Library,
  Settings2,
  ShieldCheck,
} from "lucide-react";

import { useControlPlaneCatalogRegistryGetRegistryCatalog } from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";
import type { controlPlaneCatalogRegistryGetRegistryCatalogResponse } from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";
import { type Warehouse, getWarehouse } from "@/api/controlPlane";
import { ApiError } from "@/api/client";
import { DetailFact } from "@/components/layout/DetailFact";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { attachmentWarning, isEgretWarehouse, kindLabel } from "./Catalogs";
import { ConfigurationTab } from "./ConfigurationTab";
import { EnginesTab } from "./EnginesTab";
import { WorkspacesTab } from "./WorkspacesTab";
import { warehousesQueryKey } from "../queryKeys";
import { BrowseTab } from "../warehouse-tabs/BrowseTab";
import { ClientsTab } from "../warehouse-tabs/ClientsTab";
import { ConnectTab } from "../warehouse-tabs/ConnectTab";
import { RolesTab } from "../warehouse-tabs/RolesTab";
import { SettingsTab } from "../warehouse-tabs/SettingsTab";

// Re-exported so this page stays the obvious home of its own key, as
// `Catalogs.tsx` does for the list's. It is DEFINED in `../queryKeys` because
// both tab files need it and importing it from here would make each of them
// and this page a module cycle — see there.
export { catalogKey } from "../queryKeys";

/**
 * Control Plane → Catalogs → one catalog (SP3c D1).
 *
 * Configuration, Engines and Workspaces for EVERY catalog. An egret warehouse
 * catalog additionally shows the tabs that were `WarehouseDetail`'s, REUSING
 * those components — they keep their `warehouseId: string` prop, and the
 * registry DTO carries `warehouseId` for exactly this kind of catalog and no
 * other (0147's CHECK constraint). A jdbc catalog simply has fewer tabs.
 *
 * FIVE tabs move, not the six the design's §3 counts. `WarehouseDetail`'s own
 * Engines tab is superseded by the one next door: it drew
 * `engines.filter(e => e.warehouseId === id)` — the engines whose PRIMARY
 * catalog this warehouse is — and every such engine is an `is_primary`
 * attachment in `catalog.engines` since migration 0150 backfilled it from that
 * same column. Two tabs both called "Engines", the second a subset of the
 * first, is worse than one. See `task-8-report.md` for the two columns that
 * are genuinely lost with it.
 */
export default function CatalogDetail() {
  const { id = "" } = useParams();

  // The active tab lives in the URL so the view is deep-linkable and survives
  // a refresh. The default is `configuration` — the one tab every catalog has,
  // whatever its kind and whether or not anything is attached to it.
  const [sp, setSp] = useSearchParams();
  const tab = sp.get("tab") ?? "configuration";
  const setTab = (v: string) =>
    setSp(
      (prev) => {
        const n = new URLSearchParams(prev);
        n.set("tab", v);
        return n;
      },
      { replace: true },
    );

  const catalogQ = useControlPlaneCatalogRegistryGetRegistryCatalog<
    controlPlaneCatalogRegistryGetRegistryCatalogResponse,
    // `apiFetch` throws on every non-2xx, so the error react-query holds is an
    // `ApiError` carrying `.message`. Typed as the generated `ErrorBody` the
    // page would read `.error`, which is `undefined` at runtime — a 404 with a
    // blank screen.
    ApiError
  >(id, { query: { retry: false } });

  // The clock the `stalled` rule is judged against, held in state for the same
  // two reasons as on the list: `Date.now()` in a render body is impure, and a
  // page left open would otherwise never cross the grace boundary.
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 60_000);
    return () => clearInterval(t);
  }, []);

  const catalog = catalogQ.data?.status === 200 ? catalogQ.data.data : null;

  // D1. `null` for every catalog that is not an egret warehouse, which is what
  // switches the five extra tabs off — and what keeps `getWarehouse` from
  // being called for a jdbc catalog that has no warehouse to fetch.
  const wh = catalog && isEgretWarehouse(catalog) ? catalog.warehouseId! : null;

  // `ConnectTab` and `SettingsTab` need the Warehouse ROW — its name, its
  // `lakekeeperWarehouseId`, its `storageConnId` — none of which the registry
  // DTO carries: an egret catalog's `config` is
  // `{ metastore: { type: "egret", warehouse_id } }` and nothing more. Fetched
  // only for this kind, and keyed UNDER `warehousesQueryKey` (not a
  // hand-written triple) so the list invalidations that already exist reach it.
  //
  // It sits ABOVE the early returns, as every hook on this page must: an
  // `isError` render that skipped it would change the hook count between
  // renders of the same component.
  const warehouseQ = useQuery<Warehouse, ApiError>({
    queryKey: [...warehousesQueryKey, wh],
    queryFn: () => getWarehouse(wh!),
    enabled: wh !== null,
    retry: false,
  });

  const backLink = (
    <Link
      to="/control-plane/catalogs"
      className="inline-flex items-center gap-1 text-sm text-muted-foreground transition-colors hover:text-foreground"
    >
      <ArrowLeft className="h-4 w-4" />
      Catalogs
    </Link>
  );

  if (catalogQ.isError) {
    return (
      <div className="space-y-5">
        {backLink}
        <Alert variant="destructive">
          <AlertDescription>
            {catalogQ.error?.message ?? "Couldn't load this catalog."}
          </AlertDescription>
        </Alert>
      </div>
    );
  }

  if (!catalog) {
    return (
      <div className="space-y-5">
        {backLink}
        <ListSkeleton rows={6} cols={2} />
      </div>
    );
  }

  const warning = attachmentWarning(catalog, now);

  return (
    <div className="space-y-5" data-testid="catalog-detail-frame">
      {backLink}

      <div className="min-w-0 space-y-3">
        <div className="flex items-center gap-3">
          <div className="flex h-11 w-11 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary">
            {isEgretWarehouse(catalog) ? (
              <Boxes className="h-6 w-6" />
            ) : (
              <Library className="h-6 w-6" />
            )}
          </div>
          <div className="min-w-0">
            {/* Monospace: the name is the SQL identifier this catalog is
                addressed by on every engine, not a display label. */}
            <h1 className="truncate font-mono text-2xl font-semibold leading-tight">
              {catalog.name}
            </h1>
            <Badge variant="secondary" className="mt-1">
              {kindLabel(catalog)}
            </Badge>
          </div>
        </div>

        <div className="flex flex-wrap items-center gap-x-6 gap-y-2 text-sm">
          <DetailFact
            icon={Cpu}
            label="Engines"
            value={String(catalog.engines.length)}
          />
          <DetailFact
            icon={Building2}
            label="Workspaces"
            value={String(catalog.workspaceIds.length)}
          />
          <DetailFact
            icon={Clock}
            label="Created"
            value={new Date(catalog.createdAt).toLocaleString()}
          />
          <DetailFact
            icon={Clock}
            label="Updated"
            value={new Date(catalog.updatedAt).toLocaleString()}
          />
          {catalog.warehouseId && (
            // The catalog IS this warehouse. It used to link to
            // `/control-plane/warehouses/{id}`; that path now redirects to
            // THIS page, so the link would have pointed at itself. The
            // identifier stays because it is what `catalog.warehouseId` is
            // called everywhere else — in the legacy API paths the tabs below
            // still call, and in `engines.warehouse_id`.
            <DetailFact icon={Boxes} label="Warehouse" value={catalog.warehouseId} />
          )}
        </div>
      </div>

      {/* The alarm an operator clicked on the list, restated where they land.
          Until Task 7's Engines tab exists this is the only place the detail
          page says anything about a failed or stuck attachment. */}
      {warning && (
        <Alert
          variant={warning.level === "failed" ? "destructive" : "default"}
          data-testid="catalog-attachment-warning"
        >
          <AlertDescription>{warning.text}</AlertDescription>
        </Alert>
      )}

      <Tabs value={tab} onValueChange={setTab}>
        <TabsList>
          <TabsTrigger value="configuration">
            <Settings2 className="h-4 w-4" />
            Configuration
          </TabsTrigger>
          <TabsTrigger value="engines">
            <Cpu className="h-4 w-4" />
            Engines
          </TabsTrigger>
          <TabsTrigger value="workspaces">
            <Building2 className="h-4 w-4" />
            Workspaces
          </TabsTrigger>
          {/* D1. Same labels, same icons and — critically — the same `value`s
              as `WarehouseDetail`'s tab strip carried, in its original order
              minus Engines, so a bookmarked `?tab=browse` still lands where it
              did before the redirect sent it here. */}
          {wh !== null && (
            <>
              <TabsTrigger value="roles">
                <ShieldCheck className="h-4 w-4" />
                Roles &amp; grants
              </TabsTrigger>
              <TabsTrigger value="clients">
                <KeyRound className="h-4 w-4" />
                Catalog Clients
              </TabsTrigger>
              <TabsTrigger value="browse">
                <Database className="h-4 w-4" />
                Browse
              </TabsTrigger>
              <TabsTrigger value="connect">
                <Cable className="h-4 w-4" />
                Connect
              </TabsTrigger>
              <TabsTrigger value="settings">
                <Settings2 className="h-4 w-4" />
                Settings
              </TabsTrigger>
            </>
          )}
        </TabsList>
        <TabsContent value="configuration" className="mt-4">
          <ConfigurationTab catalog={catalog} />
        </TabsContent>
        <TabsContent value="engines" className="mt-4">
          <EnginesTab catalog={catalog} />
        </TabsContent>
        <TabsContent value="workspaces" className="mt-4">
          <WorkspacesTab catalogId={catalog.id} />
        </TabsContent>
        {wh !== null && (
          <>
            <TabsContent value="roles" className="mt-4">
              <RolesTab warehouseId={wh} />
            </TabsContent>
            <TabsContent value="clients" className="mt-4">
              <ClientsTab warehouseId={wh} />
            </TabsContent>
            <TabsContent value="browse" className="mt-4">
              <BrowseTab warehouseId={wh} />
            </TabsContent>
            <TabsContent value="connect" className="mt-4">
              <ConnectTab warehouse={warehouseQ.data} />
            </TabsContent>
            <TabsContent value="settings" className="mt-4">
              <SettingsTab warehouseId={wh} storageConnId={warehouseQ.data?.storageConnId} />
            </TabsContent>
          </>
        )}
      </Tabs>
    </div>
  );
}
