import { useEffect, useState } from "react";
import { useParams, useSearchParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import {
  Boxes,
  Building2,
  Cable,
  Clock,
  Cpu,
  Database,
  KeyRound,
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
import { PageContainer } from "@/components/layout/PageContainer";
import { BackLink, DetailHeader } from "@/components/layout/DetailHeader";

// Re-exported so this page stays the obvious home of its own key, as
// `Catalogs.tsx` does for the list's. It is DEFINED in `../queryKeys` because
// both tab files need it and importing it from here would make each of them
// and this page a module cycle — see there.
export { catalogKey } from "../queryKeys";

/** The three tabs every catalog has, whatever its kind. */
const ALWAYS_TABS = ["configuration", "engines", "workspaces"];
/**
 * The five an egret warehouse catalog adds (D1) — but only for an account that
 * may read the warehouse endpoints behind them. Same `value`s `WarehouseDetail`
 * carried, which is what keeps a bookmarked `?tab=browse` working.
 */
const WAREHOUSE_TABS = ["roles", "clients", "browse", "connect", "settings"];

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
  //
  // RAW, because which tabs EXIST depends on the catalog and on this account
  // (see `tab` below `warehouseTabs`).
  const [sp, setSp] = useSearchParams();
  const rawTab = sp.get("tab") ?? "configuration";
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

  /**
   * All five moved tabs are served by the warehouse endpoints
   * (`egret-catalog-engine/src/warehouse_admin/mod.rs` — roles, clients,
   * namespaces, tabulars, config, tasks, and the warehouse row itself), gated
   * `catalog_registry:read` since SP3c-2 (Task 7) retired the router's own
   * `warehouse:read` (migration 0153 re-ran 0148's grant and deleted the old
   * keys). So `wh !== null` is the only other condition these tabs need —
   * `catalog_registry:read` already gates the route this whole page sits
   * behind (`App.tsx`'s `catalogs/:id`), which is why `warehouseTabs` below
   * reduces to the presence of a warehouse id.
   */
  const warehouseTabs = wh !== null;

  /**
   * `?tab=` NARROWED TO A TAB THAT EXISTS (SP3c-1 Task 11, found in a browser).
   *
   * Radix renders no panel for a `value` no trigger carries, so a `?tab=` the
   * strip does not offer produced a page with the header, the tab strip, NO
   * selected tab and an empty body. It is reachable from the redirect this
   * branch ships: `/control-plane/warehouses/{id}?tab=browse` is a URL an
   * operator may have bookmarked when the warehouse page owned those five
   * tabs, and it now lands here — where, for a catalog with no warehouse
   * (`wh === null`), `browse` is not among the triggers. `?tab=nonsense`
   * does the same for everyone.
   *
   * Falling back to `configuration` is the same rule `TrinoConfigEditor`
   * applies to its own `?section=` ("normalize unknown/stale values … so a bad
   * deep-link can't render a blank pane"); it was simply not applied here.
   * The URL is left as it is — rewriting it would destroy the operator's
   * bookmark for the account that CAN see the tab.
   */
  const tab =
    ALWAYS_TABS.includes(rawTab) || (warehouseTabs && WAREHOUSE_TABS.includes(rawTab))
      ? rawTab
      : "configuration";

  // `ConnectTab` and `SettingsTab` need the Warehouse ROW — its name, its
  // `lakekeeperWarehouseId`, its `storageConnId` — none of which the registry
  // DTO carries: an egret catalog's `config` is
  // `{ metastore: { type: "egret", warehouse_id } }` and nothing more. Keyed
  // UNDER `warehousesQueryKey` (not a hand-written triple) so the list
  // invalidations that already exist reach it.
  //
  // Fetched only when a tab that CONSUMES it is the one open. Radix unmounts
  // the inactive panels, so firing this on every view of an egret catalog
  // spent a request nothing was reading — and, for a user whose
  // `catalog_registry:read` has not yet reached every PDP replica, spent a
  // request that could only 403.
  //
  // It sits ABOVE the early returns, as every hook on this page must: an
  // `isError` render that skipped it would change the hook count between
  // renders of the same component.
  const warehouseQ = useQuery<Warehouse, ApiError>({
    queryKey: [...warehousesQueryKey, wh],
    queryFn: () => getWarehouse(wh!),
    enabled: warehouseTabs && (tab === "connect" || tab === "settings"),
    retry: false,
  });

  /**
   * What Connect and Settings render INSTEAD of themselves when the warehouse
   * row is not in hand. `null` means "render the tab".
   *
   * `ConnectTab` reads `warehouse?.lakekeeperWarehouseId` and, finding nothing,
   * says *"This warehouse isn't provisioned yet"*. That sentence is a claim
   * about the WAREHOUSE, and a failed or still-running request is not evidence
   * for it — a 403 on `getWarehouse` made the page assert, in plain words, a
   * fact it had never learned. Neither branch below reaches the tab, so the
   * sentence can only be said about a row egret actually read.
   */
  const warehouseRowGate = () => {
    if (warehouseQ.isLoading) return <ListSkeleton rows={4} cols={2} />;
    if (!warehouseQ.isError) return null;
    return (
      <Alert variant="destructive" data-testid="warehouse-row-error">
        <AlertDescription>
          {warehouseQ.error?.status === 403
            ? "egret refused to read this warehouse's record: that read needs the catalog_registry:read permission. Connect and Settings are both built from that record, so neither can be shown. This is an answer about your permissions and not about the warehouse — nothing here says it is unprovisioned, empty or broken."
            : `Couldn't read this warehouse's record: ${
                warehouseQ.error?.message ?? "the request failed"
              }. Connect and Settings are both built from it, so neither can be shown.`}
        </AlertDescription>
      </Alert>
    );
  };

  const backLink = <BackLink to="/control-plane/catalogs">Catalogs</BackLink>;

  if (catalogQ.isError) {
    return (
      <PageContainer className="space-y-5">
        {backLink}
        <Alert variant="destructive" data-testid="catalog-read-error">
          <AlertDescription>
            {/* A 403 here reached a page whose ROUTE is gated on the same key,
                so `me.permissions` and the PDP disagree: Task 11 §5.10
                reproduced exactly that — a fresh grant had not reached every
                PDP replica, and the first read after landing 403'd. Task 8
                carried this as unreachable and it is not. `ApiError.message`
                for it is `Error::Forbidden`'s Display, the bare word
                "forbidden", which names no permission and offers no route
                forward — the same shape Task 7's F3 removed from the attach
                picker one tab over. */}
            {catalogQ.error?.status === 403
              ? "egret refused to read this catalog: that read needs the catalog_registry:read permission. This is an answer about your permissions and not about the catalog — nothing here says it is missing or broken. If the permission was granted moments ago, try again shortly; otherwise a platform administrator can grant it."
              : (catalogQ.error?.message ?? "Couldn't load this catalog.")}
          </AlertDescription>
        </Alert>
      </PageContainer>
    );
  }

  if (!catalog) {
    return (
      <PageContainer className="space-y-5">
        {backLink}
        <ListSkeleton rows={6} cols={2} />
      </PageContainer>
    );
  }

  const warning = attachmentWarning(catalog, now);

  return (
    <PageContainer>
    <div className="space-y-5" data-testid="catalog-detail-frame">
      <DetailHeader
        className=""
        back={{ to: "/control-plane/catalogs", label: "Catalogs" }}
        // Monospace: the name is the SQL identifier this catalog is addressed
        // by on every engine, not a display label.
        title={catalog.name}
        titleClassName="min-w-0 truncate font-mono"
        status={<Badge variant="secondary">{kindLabel(catalog)}</Badge>}
        meta={
          <>
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
          </>
        }
      />

      {/* The alarm an operator clicked on the list, restated where they land.
          The Engines tab now says the same thing per attachment, and in more
          detail; this stays because it is visible from whichever tab is open,
          including the five a warehouse catalog adds. */}
      {warning && (
        <Alert
          variant={warning.level === "failed" ? "destructive" : "default"}
          data-testid="catalog-attachment-warning"
        >
          <AlertDescription>{warning.text}</AlertDescription>
        </Alert>
      )}

      {/* SP3c-1 through this task used to say ONCE, above the strip, why the
          five warehouse tabs were withheld: `warehouse:read` did not follow
          from `catalog_registry:read`, the permission that gates this whole
          page. SP3c-2 (Task 7) retired `warehouse:read` and re-gated those
          five tabs' endpoints onto `catalog_registry:read` too, so holding
          the permission this page already required is now sufficient — the
          five tabs render whenever `wh !== null` (see `warehouseTabs` above),
          and there is nothing left for a banner here to explain. */}

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
          {warehouseTabs && (
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
        {warehouseTabs && wh !== null && (
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
              {warehouseRowGate() ?? <ConnectTab warehouse={warehouseQ.data} />}
            </TabsContent>
            <TabsContent value="settings" className="mt-4">
              {warehouseRowGate() ?? (
                <SettingsTab warehouseId={wh} storageConnId={warehouseQ.data?.storageConnId} />
              )}
            </TabsContent>
          </>
        )}
      </Tabs>
    </div>
    </PageContainer>
  );
}
