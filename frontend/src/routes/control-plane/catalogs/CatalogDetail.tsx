import { useEffect, useState } from "react";
import { Link, useParams, useSearchParams } from "react-router-dom";
import { ArrowLeft, Boxes, Building2, Clock, Cpu, Library, Settings2 } from "lucide-react";

import { useControlPlaneCatalogRegistryGetRegistryCatalog } from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";
import type { controlPlaneCatalogRegistryGetRegistryCatalogResponse } from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";
import { ApiError } from "@/api/client";
import { useAuth } from "@/auth/useAuth";
import { DetailFact } from "@/components/layout/DetailFact";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { attachmentWarning, isEgretWarehouse, kindLabel } from "./Catalogs";
import { ConfigurationTab } from "./ConfigurationTab";
import { EnginesTab } from "./EnginesTab";
import { WorkspacesTab } from "./WorkspacesTab";

// Re-exported so this page stays the obvious home of its own key, as
// `Catalogs.tsx` does for the list's. It is DEFINED in `../queryKeys` because
// both tab files need it and importing it from here would make each of them
// and this page a module cycle — see there.
export { catalogKey } from "../queryKeys";

/**
 * Control Plane → Catalogs → one catalog (SP3c D1).
 *
 * Configuration, Engines and Workspaces for every catalog. Task 7 fills in
 * Engines; Task 8 adds the six warehouse tabs an iceberg-egret catalog also
 * gets, which is why the tab set is a list and not a fixed three.
 */
export default function CatalogDetail() {
  const { id = "" } = useParams();
  const { me } = useAuth();

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

  const catalog = catalogQ.data?.status === 200 ? catalogQ.data.data : null;
  if (!catalog) {
    return (
      <div className="space-y-5">
        {backLink}
        <ListSkeleton rows={6} cols={2} />
      </div>
    );
  }

  const warning = attachmentWarning(catalog, now);
  const canSeeWarehouse =
    !!me &&
    (me.platform_roles.includes("platform_admin") || me.permissions.includes("warehouse:read"));

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
            // The catalog IS this warehouse; its storage is the warehouse's,
            // and the catalog row does not carry the storage connection id —
            // an egret catalog's `config` is `{ metastore: { type: "egret",
            // warehouse_id } }` and nothing more. The warehouse page is one
            // hop from the storage connection, and it is linked only for an
            // operator who may read warehouses: `catalog_registry:read` does
            // not imply `warehouse:read`, and a link that 403s is worse than
            // an id.
            <DetailFact
              icon={Boxes}
              label="Warehouse"
              value={catalog.warehouseId}
              to={
                canSeeWarehouse
                  ? `/control-plane/warehouses/${catalog.warehouseId}`
                  : undefined
              }
            />
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
        </TabsList>
        <TabsContent value="configuration" className="mt-4">
          <ConfigurationTab catalog={catalog} />
        </TabsContent>
        <TabsContent value="engines" className="mt-4">
          <EnginesTab catalogId={catalog.id} />
        </TabsContent>
        <TabsContent value="workspaces" className="mt-4">
          <WorkspacesTab catalogId={catalog.id} />
        </TabsContent>
      </Tabs>
    </div>
  );
}
