import { useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";

import {
  type AccessRole,
  type PermissionCatalogGroup,
  listPermissions,
  listRoles,
} from "@/api/access";
import { ApiError } from "@/api/client";
import { FacetSelect, FACET_ALL } from "@/components/explore/FacetSelect";
import { FilterableTable, type Column } from "@/components/explore/FilterableTable";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { PageContainer } from "@/components/layout/PageContainer";
import { PageTitle } from "@/components/layout/PageTitle";
import { ENTITY_LINK_CLASS } from "@/components/ui/entity-link";

/** One permission flattened out of its catalog group, carrying its area. */
type PermRow = {
  key: string;
  area: string;
  description: string;
  roles: string[];
};

/**
 * Control Plane → Access → Permissions (read-only reference).
 *
 * The platform's full permission catalog. Standardised onto the shared card-less
 * list scaffold used by Users/Roles/Groups (header → SummaryStrip → bordered
 * FilterableTable + facet): the catalog's areas become a filterable `Area`
 * column + facet rather than separate grouped tables.
 *
 * A permission can be granted to anywhere from zero to hundreds of roles, so the
 * "Granted to" column is a *count* that opens a searchable drill-in dialog
 * (mirroring the Groups → Members pattern) rather than a wall of chips. Roles are
 * managed on the Roles page — this view just shows which roles hold each grant.
 */
export default function PermissionsPage() {
  const navigate = useNavigate();
  const permissions = useQuery<PermissionCatalogGroup[], ApiError>({
    queryKey: ["control-plane", "access", "permissions"],
    queryFn: listPermissions,
    retry: false,
  });

  // Used to turn a role name (all we get on a permission) into a link to its
  // detail page + its scope. Absent names simply render un-linked.
  const roles = useQuery<AccessRole[], ApiError>({
    queryKey: ["control-plane", "access", "roles"],
    queryFn: listRoles,
    retry: false,
  });
  const roleByName = useMemo(
    () => new Map((roles.data ?? []).map((r) => [r.name, r])),
    [roles.data],
  );

  const groups = useMemo(() => permissions.data ?? [], [permissions.data]);

  const rows: PermRow[] = useMemo(
    () =>
      groups.flatMap((g) =>
        g.permissions.map((p) => ({
          key: p.key,
          area: g.group,
          description: p.description,
          roles: p.roles,
        })),
      ),
    [groups],
  );

  const [areaFacet, setAreaFacet] = useState(FACET_ALL);
  const [rolesFor, setRolesFor] = useState<PermRow | null>(null);
  const filtered = rows.filter((r) => areaFacet === FACET_ALL || r.area === areaFacet);
  const grantedCount = rows.filter((r) => r.roles.length > 0).length;

  const columns: Column<PermRow>[] = [
    {
      key: "key",
      label: "Permission",
      sortable: true,
      className: "font-mono text-xs",
      value: (r) => r.key,
      cell: (r) => <code className="font-mono text-xs">{r.key}</code>,
    },
    {
      key: "area",
      label: "Area",
      sortable: true,
      value: (r) => r.area,
      cell: (r) => (
        <Badge variant="outline" className="font-normal">
          {r.area}
        </Badge>
      ),
    },
    {
      key: "description",
      label: "Description",
      value: (r) => r.description,
      cell: (r) => <span className="text-muted-foreground">{r.description || "—"}</span>,
    },
    {
      // Count, not chips — a permission granted to 100+ roles can't be a chip
      // list. The count opens a searchable dialog listing (and linking to) the
      // roles. Sorts by how widely a permission is granted.
      key: "roles",
      label: "Granted to",
      sortable: true,
      className: "text-right tabular-nums",
      value: (r) => r.roles.length,
      cell: (r) =>
        r.roles.length === 0 ? (
          <span className="text-muted-foreground">Not granted</span>
        ) : (
          <Button
            type="button"
            variant="link"
            className="h-auto p-0"
            aria-label={`View the ${r.roles.length} roles granted ${r.key}`}
            onClick={() => setRolesFor(r)}
          >
            {r.roles.length} {r.roles.length === 1 ? "role" : "roles"}
          </Button>
        ),
    },
  ];

  const areaOptions = useMemo(
    () => groups.map((g) => ({ value: g.group, label: g.group })),
    [groups],
  );

  return (
    <PageContainer className="space-y-5">
      <div>
        <PageTitle>Permissions</PageTitle>
        <p className="text-sm text-muted-foreground">
          The full catalog of platform permissions and how many roles each one is currently
          granted to. Change what a role grants on the Roles page.
        </p>
      </div>

      <div className="space-y-4">
        <SummaryStrip
          items={[
            { label: "permissions", value: rows.length },
            { label: "areas", value: groups.length },
            { label: "granted", value: grantedCount },
            { label: "ungranted", value: rows.length - grantedCount },
          ]}
        />
        {permissions.isLoading ? (
          <ListSkeleton rows={6} cols={4} />
        ) : (
          <FilterableTable
            rows={filtered}
            columns={columns}
            getId={(r) => r.key}
            searchPlaceholder="Search permissions…"
            initialSort={{ key: "key", dir: "asc" }}
            pageSize={25}
            bordered
            emptyText={
              permissions.isError
                ? (permissions.error?.message ?? "Failed to load permissions.")
                : "No permissions defined."
            }
            filters={
              <FacetSelect
                label="Area"
                value={areaFacet}
                onChange={setAreaFacet}
                options={areaOptions}
              />
            }
          />
        )}
      </div>

      <PermissionRolesDialog
        perm={rolesFor}
        roleByName={roleByName}
        onOpenChange={(o) => {
          if (!o) setRolesFor(null);
        }}
        onOpenRole={(id) => {
          setRolesFor(null);
          navigate(`/control-plane/roles/${id}`);
        }}
      />
    </PageContainer>
  );
}

/**
 * Read-only drill-in listing every role a permission is granted to, with its
 * own search (the list can be hundreds long). Each resolvable role links to its
 * detail page; a name with no matching role (shouldn't normally happen) renders
 * as plain text.
 */
function PermissionRolesDialog({
  perm,
  roleByName,
  onOpenChange,
  onOpenRole,
}: {
  perm: PermRow | null;
  roleByName: Map<string, AccessRole>;
  onOpenChange: (open: boolean) => void;
  onOpenRole: (roleId: string) => void;
}) {
  const [q, setQ] = useState("");
  const needle = q.trim().toLowerCase();
  const names = perm?.roles ?? [];
  const matches = needle ? names.filter((n) => n.toLowerCase().includes(needle)) : names;

  return (
    <Dialog
      open={perm !== null}
      onOpenChange={(o) => {
        if (!o) setQ("");
        onOpenChange(o);
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle className="flex flex-wrap items-center gap-2">
            Roles granted <code className="font-mono text-sm">{perm?.key}</code>
          </DialogTitle>
          <DialogDescription>
            {names.length} role{names.length === 1 ? "" : "s"} currently hold this permission.
            Manage what a role grants on its detail page.
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-2">
          {names.length > 8 && (
            <Input
              autoFocus
              placeholder="Search roles…"
              value={q}
              onChange={(e) => setQ(e.target.value)}
            />
          )}
          <div className="max-h-80 divide-y overflow-y-auto rounded-md border">
            {matches.length === 0 ? (
              <p className="px-3 py-6 text-center text-sm text-muted-foreground">
                No roles match.
              </p>
            ) : (
              matches.map((name) => {
                const role = roleByName.get(name);
                return (
                  <div
                    key={name}
                    className="flex items-center justify-between gap-2 px-3 py-2"
                  >
                    {role ? (
                      <button
                        type="button"
                        onClick={() => onOpenRole(role.id)}
                        className={`truncate text-left text-sm ${ENTITY_LINK_CLASS}`}
                      >
                        {name}
                      </button>
                    ) : (
                      <span className="truncate text-sm font-medium">{name}</span>
                    )}
                    {role && (
                      <Badge variant="outline" className="shrink-0 font-normal capitalize">
                        {role.scope}
                      </Badge>
                    )}
                  </div>
                );
              })
            )}
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
