import { useQuery } from "@tanstack/react-query";
import { Search } from "lucide-react";
import { useState } from "react";

import { type PermissionCatalogGroup, listPermissions } from "@/api/access";
import { ApiError } from "@/api/client";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { ConfigSection } from "@/routes/control-plane/config/ConfigSection";

const COLUMNS = ["Permission", "Description", "Granted to"];

/**
 * Control Plane → Access control → Permissions (read-only).
 *
 * Lists the platform's full permission catalog, grouped by area, and shows
 * which roles each permission is currently granted to. Roles themselves are
 * managed on the Roles page — this is a reference view.
 */
export default function PermissionsPage() {
  const permissions = useQuery<PermissionCatalogGroup[], ApiError>({
    queryKey: ["control-plane", "access", "permissions"],
    queryFn: listPermissions,
    retry: false,
  });

  const groups = permissions.data ?? [];

  const [q, setQ] = useState("");
  const needle = q.trim().toLowerCase();
  const filteredGroups = groups
    .map((g) => ({
      ...g,
      permissions: needle
        ? g.permissions.filter(
            (p) =>
              p.key.toLowerCase().includes(needle) ||
              p.description.toLowerCase().includes(needle),
          )
        : g.permissions,
    }))
    .filter((g) => g.permissions.length > 0);

  return (
    <div className="space-y-5">
      <div>
        <h1 className="text-2xl font-semibold leading-tight">Permissions</h1>
        <p className="text-sm text-muted-foreground">
          The full catalog of platform permissions, grouped by area, and the
          roles each one is currently granted to.
        </p>
      </div>

      {!permissions.isLoading && !permissions.isError && groups.length > 0 && (
        <div className="relative max-w-xs">
          <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
          <Input
            className="pl-8"
            placeholder="Search permissions…"
            value={q}
            onChange={(e) => setQ(e.target.value)}
          />
        </div>
      )}

      {permissions.isError ? (
        <Alert variant="destructive">
          <AlertDescription>
            {permissions.error?.message ?? "Failed to load permissions."}
          </AlertDescription>
        </Alert>
      ) : permissions.isLoading ? (
        <ListSkeleton rows={6} cols={3} />
      ) : groups.length === 0 ? (
        <p className="text-sm text-muted-foreground">No permissions defined.</p>
      ) : filteredGroups.length === 0 ? (
        <p className="text-sm text-muted-foreground">
          No permissions match &ldquo;{q}&rdquo;.
        </p>
      ) : (
        <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
          {filteredGroups.map((g) => (
            <ConfigSection key={g.group} title={g.group}>
              <div className="overflow-x-auto">
                <Table>
                  <TableHeader>
                    <TableRow>
                      {COLUMNS.map((col) => (
                        <TableHead key={col}>{col}</TableHead>
                      ))}
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {g.permissions.map((p) => (
                      <TableRow key={p.key}>
                        <TableCell className="font-mono text-xs">{p.key}</TableCell>
                        <TableCell className="text-muted-foreground">
                          {p.description}
                        </TableCell>
                        <TableCell>
                          {p.roles.length === 0 ? (
                            <span className="text-muted-foreground">—</span>
                          ) : (
                            <div className="flex flex-wrap gap-1">
                              {p.roles.map((role) => (
                                <Badge key={role} variant="secondary">
                                  {role}
                                </Badge>
                              ))}
                            </div>
                          )}
                        </TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </div>
            </ConfigSection>
          ))}
        </div>
      )}
    </div>
  );
}
