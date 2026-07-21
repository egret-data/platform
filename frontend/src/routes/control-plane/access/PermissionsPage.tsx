import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";

import { type PermissionCatalogGroup, listPermissions } from "@/api/access";
import { ApiError } from "@/api/client";
import { FacetSelect, FACET_ALL } from "@/components/explore/FacetSelect";
import { FilterableTable, type Column } from "@/components/explore/FilterableTable";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { Badge } from "@/components/ui/badge";

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
 * column + facet rather than separate grouped tables. Roles are managed on the
 * Roles page — this view just shows which roles each permission is granted to.
 */
export default function PermissionsPage() {
  const permissions = useQuery<PermissionCatalogGroup[], ApiError>({
    queryKey: ["control-plane", "access", "permissions"],
    queryFn: listPermissions,
    retry: false,
  });

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
      key: "roles",
      label: "Granted to",
      value: (r) => r.roles.join(", "),
      // Cap to a few chips + a "+N" overflow (rest on hover), matching Users/
      // Groups — a permission can be granted to dozens of roles, which would
      // otherwise blow up the row height into a wall of badges.
      cell: (r) =>
        r.roles.length === 0 ? (
          <span className="text-muted-foreground">—</span>
        ) : (
          <div className="flex flex-wrap items-center gap-1">
            {r.roles.slice(0, 4).map((role) => (
              <Badge key={role} variant="secondary" className="max-w-[12rem] truncate" title={role}>
                {role}
              </Badge>
            ))}
            {r.roles.length > 4 ? (
              <Badge variant="outline" title={r.roles.slice(4).join(", ")}>
                +{r.roles.length - 4}
              </Badge>
            ) : null}
          </div>
        ),
    },
  ];

  const areaOptions = useMemo(
    () => groups.map((g) => ({ value: g.group, label: g.group })),
    [groups],
  );

  return (
    <div className="space-y-5">
      <div>
        <h1 className="text-2xl font-semibold leading-tight">Permissions</h1>
        <p className="text-sm text-muted-foreground">
          The full catalog of platform permissions and the roles each one is currently
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
    </div>
  );
}
