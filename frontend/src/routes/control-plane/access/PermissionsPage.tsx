import { useQuery } from "@tanstack/react-query";

import { type PermissionCatalogGroup, listPermissions } from "@/api/access";
import { ApiError } from "@/api/client";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
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

  return (
    <div className="space-y-5">
      <div>
        <h1 className="text-2xl font-semibold leading-tight">Permissions</h1>
        <p className="text-sm text-muted-foreground">
          The full catalog of platform permissions, grouped by area, and the
          roles each one is currently granted to.
        </p>
      </div>

      {permissions.isError ? (
        <Alert variant="destructive">
          <AlertDescription>
            {permissions.error?.message ?? "Failed to load permissions."}
          </AlertDescription>
        </Alert>
      ) : permissions.isLoading ? (
        <p className="text-sm text-muted-foreground">Loading…</p>
      ) : groups.length === 0 ? (
        <p className="text-sm text-muted-foreground">No permissions defined.</p>
      ) : (
        <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
          {groups.map((g) => (
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
