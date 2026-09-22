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
import { catalogsQueryKey, warehousesQueryKey } from "../queryKeys";

// Re-exported so this page stays the obvious home of its own key (Task 5's
// `CatalogCreate.tsx` and this file's tests both import it from here). It is
// DEFINED in `../queryKeys` because `Warehouses.tsx` needs it too — see there.
export { catalogsQueryKey } from "../queryKeys";

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
 *
 * The clock is the ATTACHMENT's `createdAt`, which is the moment the wait
 * began: `pending` is written only by the INSERT that creates the row (engine
 * create and attach), and nothing flips a settled attachment back to it. The
 * catalog's `updatedAt` cannot stand in — attaching an EXISTING catalog never
 * writes `registry.catalogs.updated_at`, so that clock is already arbitrarily
 * old at the instant a fresh StarRocks attachment starts waiting, and the row
 * would cry "still pending" throughout exactly the bootstrap this grace exists
 * to cover. `appliedAt` is the independent proof the attachment ever
 * converged, kept as a second guard.
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
      now - new Date(e.createdAt).getTime() > PENDING_GRACE_MS,
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

/**
 * A catalog's workspace assignments split into the ones this operator can name
 * and a count of the ones they cannot — see `wsName` for why some cannot be.
 */
function namedWorkspaces(
  c: RegistryCatalogDto,
  wsName: (id: string) => string | null,
): { named: string[]; hidden: number } {
  const named: string[] = [];
  let hidden = 0;
  for (const id of c.workspaceIds) {
    const name = wsName(id);
    if (name) named.push(name);
    else hidden += 1;
  }
  return { named, hidden };
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
  wsName: (id: string) => string | null,
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
      // Only the names the operator can actually see are searchable, because
      // only those are drawn; the hidden ones contribute a count, not a token.
      value: (c) => namedWorkspaces(c, wsName).named.join(", "),
      cell: (c) => {
        const { named, hidden } = namedWorkspaces(c, wsName);
        const parts = [...named];
        if (hidden > 0) parts.push(`${hidden} not visible to you`);
        return (
          <span className="flex items-baseline gap-2">
            <span>{c.workspaceIds.length}</span>
            {parts.length > 0 && (
              <span className="truncate text-xs text-muted-foreground">{parts.join(", ")}</span>
            )}
          </span>
        );
      },
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

  // BOTH keys, both branches. An egret-warehouse row is deleted through the
  // very endpoint the Warehouses list calls, so refreshing only this page's
  // cache leaves the deleted warehouse listed on the other one for the 30
  // seconds of `staleTime` — reachable exactly because D5 keeps both sections
  // live. `Warehouses.tsx` does the mirror of this.
  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey: catalogsQueryKey });
    void queryClient.invalidateQueries({ queryKey: warehousesQueryKey });
  };

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

  // Best-effort, and it says so when it cannot.
  //
  // `useMyWorkspaces` is `GET /me/workspaces` — the CALLER's workspaces, which
  // is the only workspace list this page may assume it can read. The
  // all-workspaces endpoint (`GET /api/admin/workspaces`) requires
  // `workspace:read` and this page is gated on `catalog_registry:read`, which
  // does not imply it, so fetching it would 403 exactly the persona the
  // section was built for. A catalog assigned to a workspace the operator is
  // not in therefore cannot be named here — and the UI says "not visible to
  // you" rather than printing eight characters of a UUID, which reads like
  // data and is not.
  const wsName = (id: string): string | null =>
    workspaces.data?.find((w) => w.id === id)?.name ?? null;
  const rows = catalogsQ.data?.status === 200 ? catalogsQ.data.data : [];
  const plan = deleteTarget ? deletePlan(deleteTarget, wsName) : null;

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
            {wsName(wsFilter) ? (
              <>
                Filtered to workspace{" "}
                <span className="font-medium text-foreground">{wsName(wsFilter)}</span>
              </>
            ) : (
              // The id is the only identifier available for a workspace the
              // operator is not a member of, so it is shown AS an id rather
              // than in the slot a name would occupy.
              <>
                Filtered to workspace id{" "}
                <span className="font-mono text-xs text-foreground">{wsFilter}</span>
              </>
            )}
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
        description={plan?.description ?? ""}
        confirmText="delete"
        confirmDisabled={plan?.blocked ?? false}
        onConfirm={() => {
          if (deleteTarget) confirmDelete(deleteTarget);
        }}
      >
        {plan && plan.references.length > 0 ? (
          <ul className="list-disc space-y-0.5 pl-5">
            {plan.references.map((r) => (
              <li key={r}>{r}</li>
            ))}
          </ul>
        ) : null}
      </ConfirmDeleteDialog>
    </div>
  );
}

/**
 * What the confirm dialog says, what it lists, and whether it may proceed.
 *
 * The branch matters because the endpoint does. The warehouse sentence is
 * `Warehouses.tsx`'s, kept verbatim so the two sections say the same thing
 * while both exist (D5). The registry branch names the references off the row
 * — the same engines and workspaces the server would send back in the 409's
 * `catalog_in_use` body — and, when there are any, WITHHOLDS the confirm
 * button: a dialog that explains the refusal and then invites the user to type
 * "delete" and collect the error it just described is not a warning, it is a
 * trap.
 */
type DeletePlan = { description: string; references: string[]; blocked: boolean };

function deletePlan(c: RegistryCatalogDto, wsName: (id: string) => string | null): DeletePlan {
  if (isEgretWarehouse(c)) {
    return {
      description:
        "This catalog is an egret warehouse, so it is deleted with the warehouse itself. The egret warehouse record is removed. (The underlying lakekeeper warehouse is not dropped.)",
      references: [],
      blocked: false,
    };
  }
  const references = c.engines.map((e) => `Attached to engine ${e.engineName}`);
  const { named, hidden } = namedWorkspaces(c, wsName);
  for (const n of named) references.push(`Assigned to workspace ${n}`);
  if (hidden > 0) {
    references.push(
      `Assigned to ${hidden} workspace${hidden === 1 ? "" : "s"} not visible to you`,
    );
  }
  if (references.length === 0) {
    return {
      description: "The catalog is removed from the registry. Nothing references it.",
      references,
      blocked: false,
    };
  }
  return {
    description:
      "The server refuses a catalog that anything still uses, so this delete cannot go ahead yet. Detach it from every engine and unassign it from every workspace first. It still has:",
    references,
    blocked: true,
  };
}
