import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Loader2 } from "lucide-react";
import { toast } from "sonner";

import {
  getControlPlaneCatalogRegistryListCatalogWorkspacesQueryKey,
  useControlPlaneCatalogRegistryListCatalogWorkspaces,
  useControlPlaneCatalogRegistrySetCatalogWorkspaces,
} from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";
import type { controlPlaneCatalogRegistryListCatalogWorkspacesResponse } from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";
import { useMyWorkspaces } from "@/api/hooks";
import { ApiError } from "@/api/client";
import { useAuth } from "@/auth/useAuth";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Label } from "@/components/ui/label";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { catalogKey, catalogsQueryKey } from "../queryKeys";

/** One row of the checkbox list: a workspace this catalog could be assigned to. */
type Row = {
  id: string;
  name: string;
  slug: string;
  /** When it was assigned, for a row that already is. */
  assignedAt: string | null;
  /** True when the row exists only because the catalog is assigned to it. */
  foreignToMe: boolean;
};

/**
 * Control Plane → Catalogs → detail → Workspaces.
 *
 * `PUT /catalogs/{id}/workspaces` REPLACES the whole set, so this list must be
 * able to represent every workspace the catalog is currently assigned to —
 * including one the operator is not a member of. `useMyWorkspaces` is
 * `GET /me/workspaces`, deliberately caller-scoped, and there is no
 * all-workspaces list this page may read (`GET /api/admin/workspaces` needs
 * `workspace:read`, which `catalog_registry:read` does not imply). Building
 * the rows from the caller's list ALONE would therefore drop such a workspace
 * out of the array on the next save — a silent unassign, with nothing on
 * screen to have warned anyone. The rows are the union: the caller's
 * workspaces plus every assignment, whose name and slug the assignment list
 * itself carries.
 */
export function WorkspacesTab({ catalogId }: { catalogId: string }) {
  const { me } = useAuth();
  const queryClient = useQueryClient();

  const canUpdate =
    !!me &&
    (me.platform_roles.includes("platform_admin") ||
      me.permissions.includes("catalog_registry:update"));

  // `<..., ApiError>`: `apiFetch` throws, so the value on the error path is an
  // `ApiError` with `.message`. The generated `TError = ErrorBody` would make
  // `.error` type-check and be `undefined` at runtime.
  const assignedQ = useControlPlaneCatalogRegistryListCatalogWorkspaces<
    controlPlaneCatalogRegistryListCatalogWorkspacesResponse,
    ApiError
  >(catalogId, { query: { retry: false } });

  const mine = useMyWorkspaces(true);

  const assigned = assignedQ.data?.status === 200 ? assignedQ.data.data : [];
  const assignedIds = assigned.map((w) => w.workspaceId);

  // `null` = the operator has not touched the list yet, so it keeps following
  // the server's answer as that arrives and refetches. A `useState(new
  // Set(assignedIds))` reads its argument once — at a first render where
  // `assigned` is still empty — and the boxes would never tick themselves.
  const [draft, setDraft] = useState<string[] | null>(null);
  const selected = draft ?? assignedIds;

  const setMut = useControlPlaneCatalogRegistrySetCatalogWorkspaces<ApiError>({
    mutation: {
      onSuccess: () => {
        toast.success("Workspace assignments saved");
        void queryClient.invalidateQueries({
          queryKey: getControlPlaneCatalogRegistryListCatalogWorkspacesQueryKey(catalogId),
        });
        // The catalog DTO carries `workspaceIds`, and the list page draws a
        // workspace count from it — both go stale otherwise.
        void queryClient.invalidateQueries({ queryKey: catalogKey(catalogId) });
        void queryClient.invalidateQueries({ queryKey: catalogsQueryKey });
        setDraft(null);
      },
      onError: (e) => toast.error(e.message || "Failed to save the assignments."),
    },
  });

  const rows: Row[] = [];
  const seen = new Set<string>();
  for (const w of mine.data ?? []) {
    seen.add(w.id);
    rows.push({
      id: w.id,
      name: w.name,
      slug: w.slug,
      assignedAt: assigned.find((a) => a.workspaceId === w.id)?.assignedAt ?? null,
      foreignToMe: false,
    });
  }
  for (const a of assigned) {
    if (seen.has(a.workspaceId)) continue;
    rows.push({
      id: a.workspaceId,
      name: a.name,
      slug: a.slug,
      assignedAt: a.assignedAt,
      foreignToMe: true,
    });
  }
  rows.sort((a, b) => a.name.localeCompare(b.name));

  const dirty =
    draft !== null &&
    (draft.length !== assignedIds.length || draft.some((id) => !assignedIds.includes(id)));

  function toggle(id: string, on: boolean) {
    const next = new Set(selected);
    if (on) next.add(id);
    else next.delete(id);
    setDraft([...next]);
  }

  return (
    <div className="space-y-4">
      <Alert>
        <AlertDescription>
          An assignment decides which catalogs a workspace sees in its engine view. It does not
          restrict what an engine will answer — a query naming this catalog directly is still
          served — and notebook warehouse selection ignores assignment entirely, staying with the
          workspace that owns the warehouse.
        </AlertDescription>
      </Alert>

      {assignedQ.isError ? (
        <Alert variant="destructive">
          <AlertDescription>
            {assignedQ.error?.message ?? "Couldn't load this catalog's workspaces."}
          </AlertDescription>
        </Alert>
      ) : assignedQ.isLoading ? (
        <ListSkeleton rows={4} cols={2} />
      ) : rows.length === 0 ? (
        <p className="text-sm text-muted-foreground">There are no workspaces to assign.</p>
      ) : (
        <ul className="divide-y rounded-lg border">
          {rows.map((r) => (
            <li key={r.id} className="flex flex-wrap items-center gap-3 p-3">
              <Checkbox
                id={`cd-ws-${r.id}`}
                checked={selected.includes(r.id)}
                disabled={!canUpdate}
                onCheckedChange={(v) => toggle(r.id, v === true)}
              />
              <Label htmlFor={`cd-ws-${r.id}`} className="cursor-pointer font-medium">
                {r.name}
              </Label>
              <span className="font-mono text-xs text-muted-foreground">{r.slug}</span>
              {r.foreignToMe && (
                <span className="text-xs text-muted-foreground">
                  you are not a member of this workspace
                </span>
              )}
              {r.assignedAt && (
                <span className="ml-auto text-xs text-muted-foreground">
                  assigned {new Date(r.assignedAt).toLocaleString()}
                </span>
              )}
            </li>
          ))}
        </ul>
      )}

      {canUpdate && (
        <div className="flex flex-wrap items-center justify-end gap-3 border-t pt-4">
          {dirty && (
            <p role="status" className="mr-auto text-xs text-muted-foreground">
              Unsaved changes. Saving replaces the whole set of assignments.
            </p>
          )}
          <Button
            variant="outline"
            onClick={() => setDraft(null)}
            disabled={!dirty || setMut.isPending}
          >
            Reset
          </Button>
          <Button
            onClick={() =>
              // The WHOLE set, every time. This endpoint is a replace, so
              // posting only the additions unassigns everything else.
              setMut.mutate({ id: catalogId, data: { workspaceIds: [...selected] } })
            }
            disabled={!dirty || setMut.isPending}
          >
            {setMut.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
            Save assignments
          </Button>
        </div>
      )}
    </div>
  );
}
