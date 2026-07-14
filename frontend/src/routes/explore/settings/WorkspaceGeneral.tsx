import { Link, useParams } from "react-router-dom";

import { useMyWorkspaces } from "@/api/hooks";
import { useAuth } from "@/auth/useAuth";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";

/**
 * Workspace settings → General.
 *
 * Read-only identity of the current workspace. Renaming or deleting a workspace
 * is a platform-level action (owned by the Control Plane), so this view surfaces
 * the workspace's name/slug/id and points platform admins to where those live —
 * rather than pretending to offer edit controls the workspace API doesn't expose.
 */
export default function WorkspaceGeneral() {
  const { workspace: slug } = useParams();
  const { me } = useAuth();
  const isPlatformAdmin = me?.platform_roles.includes("platform_admin") ?? false;

  const wsQuery = useMyWorkspaces(true);
  const ws = wsQuery.data?.find((w) => w.slug === slug);

  return (
    <div className="space-y-5">
      <div>
        <h2 className="text-lg font-semibold">General</h2>
        <p className="text-sm text-muted-foreground">
          This workspace&rsquo;s identity. Name and lifecycle are managed centrally.
        </p>
      </div>

      <Card>
        <CardHeader>
          <CardTitle className="text-base">Workspace</CardTitle>
          <CardDescription>Read-only — managed in the Control Plane.</CardDescription>
        </CardHeader>
        <CardContent>
          {wsQuery.isLoading ? (
            <ListSkeleton rows={3} cols={2} />
          ) : wsQuery.isError ? (
            <Alert variant="destructive">
              <AlertDescription>
                {(wsQuery.error as Error)?.message ?? "Failed to load workspace details."}
              </AlertDescription>
            </Alert>
          ) : (
            <dl className="grid grid-cols-1 gap-x-8 gap-y-4 sm:grid-cols-[8rem_1fr]">
              <dt className="text-sm text-muted-foreground">Name</dt>
              <dd className="text-sm font-medium">{ws?.name ?? "—"}</dd>

              <dt className="text-sm text-muted-foreground">Slug</dt>
              <dd>
                <Badge variant="secondary" className="font-mono font-normal">
                  {ws?.slug ?? slug ?? "—"}
                </Badge>
              </dd>

              <dt className="text-sm text-muted-foreground">Workspace ID</dt>
              <dd className="font-mono text-xs text-muted-foreground">{ws?.id ?? "—"}</dd>
            </dl>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="text-base">Name &amp; lifecycle</CardTitle>
        </CardHeader>
        <CardContent className="space-y-3 text-sm text-muted-foreground">
          <p>
            Renaming or deleting a workspace is a platform-admin action and lives in the
            Control Plane, not here — so an accidental change from inside a workspace can&rsquo;t
            take it down.
          </p>
          {isPlatformAdmin && (
            <Link
              to="/control-plane/workspaces"
              className="inline-flex items-center text-sm font-medium text-primary underline-offset-2 hover:underline"
            >
              Manage workspaces in the Control Plane →
            </Link>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
