import { useEffect, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Library, Plus, Trash2 } from "lucide-react";
import { toast } from "sonner";

import {
  useControlPlaneCatalogRegistryDeleteRegistryCatalog,
  useControlPlaneCatalogRegistryListRegistryCatalogs,
} from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";
import type { RegistryCatalogDto } from "@/api/generated/model";
import { deleteWarehouse } from "@/api/controlPlane";
import { useMyWorkspaces } from "@/api/hooks";
import { ApiError } from "@/api/client";
import { EmptyState } from "@/components/explore/EmptyState";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { FilterableTable, type Column } from "@/components/explore/FilterableTable";
import { ListSkeleton } from "@/components/explore/ListSkeleton";

/**
 * The key the generated list query is cached under, so a delete on this page
 * invalidates the list the user is looking at.
 *
 * This is deliberately the GENERATED builder's value
 * (`getControlPlaneCatalogRegistryListRegistryCatalogsQueryKey()`), not a
 * hand-picked `["control-plane", "catalogs"]` pair: the rows come from the
 * generated hook, so only its own key refreshes them. The `?workspace=`
 * variant appends its params to this same first element, so invalidating on
 * this prefix catches every variant. `Catalogs.test.tsx` pins the literal to
 * the builder's output — a drift there would silently leave a deleted row on
 * screen with every other test still green.
 */
export const catalogsQueryKey = ["/api/control-plane/catalogs"] as const;

/**
 * Is this catalog the egret warehouse it mirrors?
 *
 * `warehouseId` is the WHOLE predicate. Migration `0147_catalog_registry.sql`'s
 * `catalogs_warehouse_mirrors_config` CHECK makes `warehouse_id IS NOT NULL`
 * exactly equivalent to `kind = 'iceberg' AND config->metastore->>type =
 * 'egret'`, so sniffing `config` as well would add a second, weaker copy of a
 * rule the database already enforces.
 */
export function isEgretWarehouse(c: RegistryCatalogDto): boolean {
  return c.warehouseId != null;
}

export type CatalogWarning = { level: "failed" | "stalled"; text: string } | null;

/** 15 minutes: longer than a StarRocks FE takes to come up (SP3b D1 says 5-10). */
const PENDING_GRACE_MS = 15 * 60 * 1000;

/**
 * The list's warning column. `failed` is unambiguous. `pending` is NORMAL for
 * the first minutes of a StarRocks engine's life — SP3b writes it deliberately
 * — so it is only a warning once it has outlived the bootstrap that clears it.
 * `appliedAt` is the proof the attachment ever converged, and `updatedAt` is
 * the last time egret changed the catalog, which is when a re-apply would have
 * restarted the clock.
 */
export function attachmentWarning(c: RegistryCatalogDto, now: number): CatalogWarning {
  const failed = c.engines.filter((e) => e.applyStatus === "failed");
  if (failed.length > 0) {
    return {
      level: "failed",
      text: `Failed to apply on ${failed.map((e) => e.engineName).join(", ")}`,
    };
  }
  const stalled = c.engines.filter(
    (e) =>
      e.applyStatus === "pending" &&
      !e.appliedAt &&
      now - new Date(c.updatedAt).getTime() > PENDING_GRACE_MS,
  );
  if (stalled.length > 0) {
    return {
      level: "stalled",
      text: `Still pending on ${stalled
        .map((e) => e.engineName)
        .join(", ")} — egret has recorded the attachment but has not been able to tell the engine`,
    };
  }
  return null;
}

/** What the Kind column shows: the warehouse it is, or the raw kind. */
function kindLabel(c: RegistryCatalogDto): string {
  return isEgretWarehouse(c) ? "egret warehouse" : c.kind;
}

/**
 * Column set for the catalogs table. As in `warehouseColumns`, `value` (search
 * + sort) is the joined engine/workspace NAMES rather than the count, so a
 * search for an engine finds the catalogs attached to it — the names are drawn
 * beside the count for that reason.
 */
function catalogColumns(
  wsName: (id: string) => string,
  now: number,
  onDelete: (c: RegistryCatalogDto) => void,
): Column<RegistryCatalogDto>[] {
  return [
    {
      key: "name",
      label: "Name",
      sortable: true,
      className: "font-medium",
      value: (c) => c.name,
      cell: (c) => (
        <Link
          to={`/control-plane/catalogs/${c.id}`}
          className="font-medium text-primary hover:underline"
        >
          {c.name}
        </Link>
      ),
    },
    {
      key: "kind",
      label: "Kind",
      sortable: true,
      value: (c) => kindLabel(c),
      cell: (c) => <Badge variant="secondary">{kindLabel(c)}</Badge>,
    },
    {
      key: "engines",
      label: "Engines",
      sortable: true,
      value: (c) => c.engines.map((e) => e.engineName).join(", "),
      cell: (c) => (
        <span className="flex items-baseline gap-2">
          <span>{c.engines.length}</span>
          {c.engines.length > 0 && (
            <span className="truncate text-xs text-muted-foreground">
              {c.engines.map((e) => e.engineName).join(", ")}
            </span>
          )}
        </span>
      ),
    },
    {
      key: "workspaces",
      label: "Workspaces",
      sortable: true,
      value: (c) => c.workspaceIds.map(wsName).join(", "),
      cell: (c) => (
        <span className="flex items-baseline gap-2">
          <span>{c.workspaceIds.length}</span>
          {c.workspaceIds.length > 0 && (
            <span className="truncate text-xs text-muted-foreground">
              {c.workspaceIds.map(wsName).join(", ")}
            </span>
          )}
        </span>
      ),
    },
    {
      key: "status",
      label: "Status",
      sortable: true,
      value: (c) => attachmentWarning(c, now)?.text ?? "",
      cell: (c) => {
        const warning = attachmentWarning(c, now);
        if (!warning) return <span className="text-muted-foreground">—</span>;
        return (
          <Badge variant={warning.level === "failed" ? "destructive" : "warning"}>
            {warning.text}
          </Badge>
        );
      },
    },
    {
      key: "actions",
      label: "",
      className: "text-right w-10",
      cell: (c) => (
        <Button
          variant="ghost"
          size="sm"
          className="text-muted-foreground hover:text-destructive"
          onClick={() => onDelete(c)}
          aria-label={`Delete ${c.name}`}
        >
          <Trash2 className="h-4 w-4" />
          Delete
        </Button>
      ),
    },
  ];
}

/**
 * Control Plane → Infrastructure → Catalogs (SP3c §3).
 *
 * The registry list: name, kind, attached engines, assigned workspaces, and a
 * warning when an attachment is `failed` or has been `pending` longer than a
 * bootstrap takes. Warehouses still has its own section through SP3c-1 (D5);
 * Task 8 turns those routes into redirects here.
 */
export default function Catalogs() {
  // `?workspace=<id>` is the ROUTE's filter here, not a client-side one as on
  // the warehouse list: the list endpoint takes `workspaceId`, and a catalog
  // can be assigned to several workspaces, so filtering the returned rows
  // would have to reimplement the server's own membership test.
  const [searchParams] = useSearchParams();
  const wsFilter = searchParams.get("workspace");
  const catalogsQ = useControlPlaneCatalogRegistryListRegistryCatalogs(
    wsFilter ? { workspaceId: wsFilter } : undefined,
    { query: { retry: false } },
  );
  const workspaces = useMyWorkspaces(true);
  const queryClient = useQueryClient();

  const [deleteTarget, setDeleteTarget] = useState<RegistryCatalogDto | null>(null);

  const invalidate = () => queryClient.invalidateQueries({ queryKey: catalogsQueryKey });

  // The generated `TError` is `ErrorBody | CatalogConflictBody`, but `apiFetch`
  // throws and the runtime value is an `ApiError` carrying the body's `error`
  // string as its `message` (see `api/mutator.ts`). Typing it as what actually
  // arrives is why these toasts say something.
  const deleteRegistry = useControlPlaneCatalogRegistryDeleteRegistryCatalog<ApiError>({
    mutation: {
      onSuccess: invalidate,
      onError: (e) => toast.error(e.message || "Failed to delete the catalog."),
    },
  });
  const deleteWh = useMutation<void, ApiError, string>({
    mutationFn: deleteWarehouse,
    onSuccess: invalidate,
    onError: (e) => toast.error(e.message || "Failed to delete the warehouse."),
  });

  function confirmDelete(c: RegistryCatalogDto) {
    // The registry refuses an egret warehouse catalog unconditionally (409
    // `warehouse_catalog`) — it is deleted with its warehouse, through the
    // endpoint D5 keeps. Sending it to the registry would show the user a
    // conflict they cannot act on.
    if (isEgretWarehouse(c)) deleteWh.mutate(c.warehouseId!);
    else deleteRegistry.mutate({ id: c.id });
  }

  // The clock the stalled rule is judged against. Held in state rather than
  // read in the render body: `Date.now()` there is impure
  // (`react-hooks/purity`), and a page left open would otherwise freeze at its
  // mount time and never cross the grace boundary. The window is 15 minutes,
  // so a minute's resolution is ample — same pattern as `RefreshMenu`.
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 60_000);
    return () => clearInterval(id);
  }, []);

  const wsName = (id: string) => workspaces.data?.find((w) => w.id === id)?.name ?? id.slice(0, 8);
  const rows = catalogsQ.data?.status === 200 ? catalogsQ.data.data : [];

  const total = rows.length;
  const attached = rows.filter((c) => c.engines.length > 0).length;
  const needingAttention = rows.filter((c) => attachmentWarning(c, now) !== null).length;

  const newCatalogButton = (
    <Button asChild className="shrink-0">
      <Link to="/control-plane/catalogs/new">
        <Plus className="h-4 w-4" />
        New catalog
      </Link>
    </Button>
  );

  return (
    <div className="space-y-4" data-testid="catalogs-frame">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="max-w-3xl">
          <h1 className="text-2xl font-semibold leading-tight">Catalogs</h1>
          <p className="text-sm text-muted-foreground">
            Every catalog the platform knows about — egret warehouses and external
            metastores alike. Attach one to an engine to make it queryable, and
            assign it to a workspace to let that workspace see it.
          </p>
        </div>
        {newCatalogButton}
      </div>

      <SummaryStrip
        items={[
          { label: "catalogs", singular: "catalog", value: total },
          { label: "attached to an engine", value: attached },
          { label: "needing attention", value: needingAttention },
        ]}
      />

      {wsFilter && (
        <div className="flex flex-wrap items-center gap-2 rounded-md border bg-muted/40 px-3 py-2 text-sm">
          <span className="text-muted-foreground">
            Filtered to workspace <span className="font-medium text-foreground">{wsName(wsFilter)}</span>
          </span>
          <Link to="/control-plane/catalogs" className="ml-auto text-primary hover:underline">
            Show all workspaces
          </Link>
        </div>
      )}

      {catalogsQ.isError ? (
        <Alert variant="destructive">
          <AlertDescription>Failed to load catalogs. Please try again.</AlertDescription>
        </Alert>
      ) : catalogsQ.isLoading ? (
        <ListSkeleton rows={6} cols={6} />
      ) : (
        <FilterableTable
          rows={rows}
          columns={catalogColumns(wsName, now, setDeleteTarget)}
          getId={(c) => c.id}
          searchPlaceholder="Search catalogs…"
          initialSort={{ key: "name", dir: "asc" }}
          pageSize={25}
          bordered
          emptyText={
            wsFilter ? (
              "No catalogs assigned to this workspace."
            ) : (
              <EmptyState
                icon={Library}
                title="No catalogs yet"
                description="Register an external metastore, or create an egret warehouse over a storage connection."
                action={newCatalogButton}
              />
            )
          }
        />
      )}

      <ConfirmDeleteDialog
        open={deleteTarget !== null}
        onOpenChange={(o) => {
          if (!o) setDeleteTarget(null);
        }}
        title={deleteTarget ? `Delete «${deleteTarget.name}»?` : "Delete catalog?"}
        description={deleteTarget ? deleteDescription(deleteTarget, wsName) : ""}
        confirmText="delete"
        onConfirm={() => {
          if (deleteTarget) confirmDelete(deleteTarget);
        }}
      />
    </div>
  );
}

/**
 * What the confirmation actually does, which differs by branch because the
 * endpoint does. The warehouse sentence is `Warehouses.tsx`'s, kept verbatim so
 * the two sections say the same thing while both exist (D5); the registry one
 * reads the references off the row, because deleting a catalog an engine or
 * workspace still uses is what the server refuses with a 409.
 */
function deleteDescription(c: RegistryCatalogDto, wsName: (id: string) => string): string {
  if (isEgretWarehouse(c)) {
    return "This catalog is an egret warehouse, so it is deleted with the warehouse itself. The egret warehouse record is removed. (The underlying lakekeeper warehouse is not dropped.)";
  }
  const refs: string[] = [];
  if (c.engines.length > 0) {
    refs.push(`${c.engines.length === 1 ? "engine" : "engines"} ${c.engines.map((e) => e.engineName).join(", ")}`);
  }
  if (c.workspaceIds.length > 0) {
    refs.push(
      `${c.workspaceIds.length === 1 ? "workspace" : "workspaces"} ${c.workspaceIds.map(wsName).join(", ")}`,
    );
  }
  return refs.length > 0
    ? `Still referenced by ${refs.join(" and ")}. The server refuses a catalog that is still in use — detach and unassign it first.`
    : "The catalog is removed from the registry. Nothing references it.";
}
