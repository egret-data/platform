import { Link } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { ShieldCheck } from "lucide-react";
import { type Profile, listProfiles } from "@/api/policies";
import { ApiError } from "@/api/client";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

/**
 * Control Plane → Policies (drill-down L1: Profiles landing).
 *
 * Lists Ranger policy profiles. Each row links into the per-profile detail
 * page (`ProfileDetailPage`) where access / masking / row-filter policies
 * are managed. The per-policy table, type tabs, enable toggle, delete, and
 * editor navigation now live there — this page is just the index.
 */
export default function Policies() {
  const profiles = useQuery<Profile[], ApiError>({
    queryKey: ["control-plane", "profiles"],
    queryFn: listProfiles,
    retry: false,
  });
  const rows = profiles.data ?? [];

  return (
    <div className="p-6">
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2 text-lg">
            <ShieldCheck className="h-5 w-5" />
            Policy Profiles
          </CardTitle>
          <CardDescription>
            Ranger policy sets. Each engine enforces one profile. Open a
            profile to manage its access, masking, and row-filter policies.
          </CardDescription>
        </CardHeader>
        <CardContent>
          {profiles.isError ? (
            <Alert variant="destructive">
              <AlertDescription>
                {profiles.error?.message ?? "Failed to load profiles."}
              </AlertDescription>
            </Alert>
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Name</TableHead>
                  <TableHead>Type</TableHead>
                  <TableHead>Engines</TableHead>
                  <TableHead>Version</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {rows.map((p) => (
                  <TableRow key={p.id}>
                    <TableCell>
                      <Link
                        to={`/control-plane/policies/${encodeURIComponent(p.name)}`}
                        className="font-medium text-primary hover:underline"
                      >
                        {p.name}
                      </Link>
                    </TableCell>
                    <TableCell>
                      <Badge variant="secondary">{p.type}</Badge>
                    </TableCell>
                    <TableCell>
                      {p.engines.length ? (
                        p.engines.map((e) => e.name).join(", ")
                      ) : (
                        <span className="text-muted-foreground">—</span>
                      )}
                    </TableCell>
                    <TableCell className="text-muted-foreground">
                      v{p.policy_version}
                    </TableCell>
                  </TableRow>
                ))}
                {rows.length === 0 && (
                  <TableRow className="hover:bg-transparent">
                    <TableCell
                      colSpan={4}
                      className="text-center text-muted-foreground"
                    >
                      {profiles.isLoading ? "Loading…" : "No profiles yet."}
                    </TableCell>
                  </TableRow>
                )}
              </TableBody>
            </Table>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
