import { useState } from "react";
import { Pencil, Plus, Trash2, UserCheck } from "lucide-react";
import { toast } from "sonner";

import {
  getPermListGrantsQueryKey,
  usePermDeleteGrant,
  usePermListGrants,
} from "@/api/generated/perm/perm";
import { useControlPlaneCatalogRegistryListRegistryCatalogs } from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";
import type { Action, GrantView } from "@/api/generated/model";
import { ApiError } from "@/api/client";
import { useMyWorkspaces } from "@/api/hooks";
import { useAuth } from "@/auth/useAuth";
import { EmptyState } from "@/components/explore/EmptyState";
import { FACET_ALL, FacetSelect } from "@/components/explore/FacetSelect";
import { FilterableTable, type Column } from "@/components/explore/FilterableTable";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { useQueryClient } from "@tanstack/react-query";
import { AssignmentBadge, GrantDialog } from "./GrantDialog";
import { grantSentence } from "./permFormat";

/**
 * Governance → Data permissions (SP4b-2 Task 3).
 *
 * Steward-facing: "who can do what" as a sentence built from `perm.grants`
 * (SP4a). NOT the RBAC permission *catalog* at Control Plane → Access →
 * Permissions — that page lists the fixed set of fine-grained permissions
 * egret understands; this one lists grants over data. Deliberately kept
 * distinct in name (see the design spec's naming hazard) so the two are never
 * confused in conversation or in a bug report.
 *
 * D5: **the sentence is the primary column**, not a resource map — the
 * structural form (Ranger policies) lives on the Policies page instead. A
 * grant whose `origin` is `workspace_assignment` was written by the
 * assignment sync, not a human (D3 of SP4a): it is marked, its source is
 * named, and it offers no Edit/Delete here.
 */
export default function DataPermissions() {
  const { me } = useAuth();
  const isAdmin = !!me && me.platform_roles.includes("platform_admin");
  const canWrite = isAdmin || !!me?.permissions.includes("policy:update");

  const grantsQ = usePermListGrants({ query: { retry: false } });
  const rows: GrantView[] = grantsQ.data?.status === 200 ? grantsQ.data.data : [];

  // ONLY for `GrantDialog`'s own preview line, which a writer opens and which
  // needs the registry anyway to pick a catalog. The SENTENCE below is
  // rendered from `GrantView.catalog_name` on the payload — SP4b-2 finding 1:
  // this page gates on `policy:read`, this endpoint gates on
  // `catalog_registry:read`, and resolving the primary column here made D5's
  // sentence read as a raw uuid for exactly the persona the page is for.
  const catalogsQ = useControlPlaneCatalogRegistryListRegistryCatalogs(undefined, {
    query: { retry: false },
  });
  const catalogs = catalogsQ.data?.status === 200 ? catalogsQ.data.data : [];
  const catalogName = (id: string) => catalogs.find((c) => c.id === id)?.name ?? id;

  const workspaces = useMyWorkspaces(true);
  const workspaceLabel = (id: string) => workspaces.data?.find((w) => w.id === id)?.name ?? id;

  const [effectFilter, setEffectFilter] = useState(FACET_ALL);
  const [actionFilter, setActionFilter] = useState(FACET_ALL);
  const filtered = rows.filter(
    (g) =>
      (effectFilter === FACET_ALL || g.effect === effectFilter) &&
      (actionFilter === FACET_ALL || g.actions.includes(actionFilter as Action)),
  );

  const [dialogOpen, setDialogOpen] = useState(false);
  const [editing, setEditing] = useState<GrantView | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<GrantView | null>(null);

  const queryClient = useQueryClient();
  const deleteGrant = usePermDeleteGrant<ApiError>({
    mutation: {
      onSuccess: () => void queryClient.invalidateQueries({ queryKey: getPermListGrantsQueryKey() }),
      onError: (e) => toast.error(e.message || "Failed to delete the grant."),
    },
  });

  function openCreate() {
    setEditing(null);
    setDialogOpen(true);
  }
  function openEdit(g: GrantView) {
    setEditing(g);
    setDialogOpen(true);
  }

  const columns: Column<GrantView>[] = [
    {
      key: "sentence",
      label: "Grant",
      sortable: true,
      value: (g) => grantSentence(g),
      cell: (g) => (
        <div className="space-y-1 py-0.5">
          <p className="text-sm">{grantSentence(g)}</p>
          {g.origin.type === "workspace_assignment" && (
            <AssignmentBadge workspaceLabel={workspaceLabel(g.origin.workspace_id)} />
          )}
        </div>
      ),
    },
    {
      key: "actions",
      label: "",
      className: "w-24 text-right",
      cell: (g) =>
        g.origin.type === "explicit" && canWrite ? (
          <div className="flex justify-end gap-1">
            <Button
              variant="ghost"
              size="icon"
              aria-label={`Edit ${grantSentence(g)}`}
              onClick={() => openEdit(g)}
            >
              <Pencil className="h-4 w-4" />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              className="text-muted-foreground hover:text-destructive"
              aria-label={`Delete ${grantSentence(g)}`}
              onClick={() => setDeleteTarget(g)}
            >
              <Trash2 className="h-4 w-4" />
            </Button>
          </div>
        ) : null,
    },
  ];

  const allowCount = rows.filter((g) => g.effect === "allow").length;
  const denyCount = rows.filter((g) => g.effect === "deny").length;
  const assignmentCount = rows.filter((g) => g.origin.type === "workspace_assignment").length;

  const forbidden = grantsQ.error instanceof ApiError && grantsQ.error.status === 403;

  const newGrantButton = canWrite ? (
    <Button size="sm" onClick={openCreate}>
      <Plus className="mr-1 h-4 w-4" />
      New grant
    </Button>
  ) : null;

  return (
    <div className="space-y-5">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h1 className="text-2xl font-semibold leading-tight">Data permissions</h1>
          <p className="text-sm text-muted-foreground">
            Who can do what with data, read as a sentence — e.g. "analyst can read
            wh.sales.* except group contractors".
          </p>
        </div>
        {newGrantButton}
      </div>

      <div className="space-y-4">
        {grantsQ.isLoading ? (
          <ListSkeleton rows={6} cols={2} />
        ) : forbidden ? (
          <Alert variant="destructive">
            <AlertDescription>
              You need the <code>policy:read</code> permission.
            </AlertDescription>
          </Alert>
        ) : grantsQ.isError ? (
          <Alert variant="destructive">
            <AlertDescription>Failed to load grants. Please try again.</AlertDescription>
          </Alert>
        ) : (
          <>
            <SummaryStrip
              items={[
                { label: "grants", singular: "grant", value: rows.length },
                { label: "allow", value: allowCount },
                { label: "deny", value: denyCount },
                { label: "from a workspace assignment", value: assignmentCount },
              ]}
            />
            <FilterableTable
              rows={filtered}
              columns={columns}
              getId={(g) => g.id}
              searchPlaceholder="Search grants…"
              initialSort={{ key: "sentence", dir: "asc" }}
              pageSize={25}
              bordered
              filters={
                <>
                  <FacetSelect
                    label="Effect"
                    value={effectFilter}
                    onChange={setEffectFilter}
                    options={[
                      { value: "allow", label: "Allow" },
                      { value: "deny", label: "Deny" },
                    ]}
                  />
                  <FacetSelect
                    label="Action"
                    value={actionFilter}
                    onChange={setActionFilter}
                    options={[
                      { value: "read", label: "Read" },
                      { value: "write", label: "Write" },
                    ]}
                  />
                </>
              }
              emptyText={
                <EmptyState
                  icon={UserCheck}
                  variant="dashed"
                  title="No grants yet"
                  description={
                    canWrite
                      ? "Create a grant to say who can read or write which data."
                      : "Creating a grant needs the policy:update permission, which this account does not have."
                  }
                  action={newGrantButton ?? undefined}
                />
              }
            />
          </>
        )}
      </div>

      <GrantDialog
        open={dialogOpen}
        onOpenChange={setDialogOpen}
        existing={editing}
        catalogName={catalogName}
      />

      <ConfirmDeleteDialog
        open={deleteTarget !== null}
        onOpenChange={(o) => {
          if (!o) setDeleteTarget(null);
        }}
        title={
          deleteTarget
            ? `Delete "${grantSentence(deleteTarget)}"?`
            : "Delete grant?"
        }
        description="This grant is permanently removed and every profile it reaches recompiles."
        confirmText="delete"
        onConfirm={() => {
          if (deleteTarget) deleteGrant.mutate({ id: deleteTarget.id });
          setDeleteTarget(null);
        }}
      />
    </div>
  );
}
