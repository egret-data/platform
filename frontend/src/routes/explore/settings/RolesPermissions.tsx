import { useMemo } from "react";
import { useParams } from "react-router-dom";

import { useWorkspaceRoleCatalog, type WsPerm, type WsRole } from "@/api/workspaceAccess";
import { FilterableTable, type Column } from "@/components/explore/FilterableTable";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { errorStatus } from "@/lib/httpStatus";

/** Built-in role names, seeded on install — mirrors RolesPage's own copy. */
const SEEDED_ROLE_NAMES = new Set(["ws_admin", "ws_editor", "ws_viewer"]);

function isSeeded(role: WsRole): boolean {
  return SEEDED_ROLE_NAMES.has(role.name);
}

const permColumns: Column<WsPerm>[] = [
  {
    key: "key",
    label: "Permission",
    sortable: true,
    value: (p) => p.key,
    cell: (p) => <span className="font-mono text-xs">{p.key}</span>,
  },
  {
    key: "description",
    label: "Description",
    value: (p) => p.description,
    cell: (p) => <span className="text-muted-foreground">{p.description || "—"}</span>,
  },
  {
    key: "group",
    label: "Group",
    sortable: true,
    value: (p) => p.group,
    cell: (p) =>
      p.group ? (
        <Badge variant="outline">{p.group}</Badge>
      ) : (
        <span className="text-muted-foreground">—</span>
      ),
  },
];

/**
 * Read-only view of workspace roles and the permissions each one grants.
 *
 * Roles themselves are defined platform-wide (Control Plane → Access
 * control) and merely apply per-workspace, so there is nothing to edit
 * here — this page exists to answer "what can each role do?" for a
 * workspace admin without sending them to the control plane.
 */
export default function RolesPermissions() {
  const ws = useParams().workspace!;
  const q = useWorkspaceRoleCatalog(ws);

  const roles = q.data ?? [];
  const forbidden = errorStatus(q.error) === 403;
  const nameById = useMemo(() => new Map(roles.map((r) => [r.id, r.name])), [roles]);

  return (
    <div className="space-y-5">
      <header className="space-y-3">
        <div>
          <h2 className="text-lg font-semibold">Roles & permissions</h2>
          <p className="text-sm text-muted-foreground">
            What each workspace role can do in <span className="font-medium">{ws}</span>.
          </p>
        </div>
        <div
          className="rounded-md border border-highlight/40 bg-highlight/5 px-3 py-2 text-xs text-muted-foreground"
          role="note"
        >
          Roles are defined platform-wide in{" "}
          <span className="font-medium">Control Plane → Access control</span> and apply to
          every workspace. This is a read-only view.
        </div>
      </header>

      {q.isLoading ? (
        <ListSkeleton rows={4} cols={3} />
      ) : forbidden ? (
        <p
          className="rounded-md border bg-muted/40 px-4 py-6 text-sm text-muted-foreground"
          role="alert"
        >
          Only workspace admins can view roles.
        </p>
      ) : q.isError ? (
        <p className="py-6 text-sm text-destructive" role="alert">
          Couldn&apos;t load roles. {String(q.error)}
        </p>
      ) : roles.length === 0 ? (
        <p className="rounded-md border bg-muted/40 px-4 py-6 text-sm text-muted-foreground">
          No workspace roles defined.
        </p>
      ) : (
        roles.map((role) => (
          <Card key={role.id}>
            <CardHeader className="space-y-1.5">
              <div className="flex flex-wrap items-center gap-2">
                <span className="font-medium">{role.name}</span>
                {isSeeded(role) && (
                  <Badge variant="secondary" className="font-normal">
                    built-in
                  </Badge>
                )}
                {role.parent_role_id && nameById.get(role.parent_role_id) && (
                  <Badge variant="outline" className="font-normal">
                    Inherits from {nameById.get(role.parent_role_id)}
                  </Badge>
                )}
              </div>
              {role.description && (
                <p className="text-sm text-muted-foreground">{role.description}</p>
              )}
            </CardHeader>
            <CardContent>
              <FilterableTable
                rows={role.permissions}
                columns={permColumns}
                getId={(p) => p.key}
                searchPlaceholder="Search permissions…"
                initialSort={{ key: "key", dir: "asc" }}
                emptyText="No permissions."
              />
            </CardContent>
          </Card>
        ))
      )}
    </div>
  );
}
