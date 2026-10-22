import { useQueryClient } from "@tanstack/react-query";

import { useLogout } from "@/api/hooks";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";

/**
 * Landing for a signed-in user who belongs to no workspace and has no
 * Governance or Control Plane access (KI-185). It renders outside the app
 * shell, so it carries its own way out: the address an admin adds them by, a
 * re-check, and sign-out.
 */
export function NoWorkspaceYet({ email }: { email: string }) {
  const qc = useQueryClient();
  const logout = useLogout();
  return (
    <div className="flex min-h-screen items-center justify-center bg-background p-4">
      <Card className="w-full max-w-md space-y-4 p-6">
        <h1 className="text-xl font-semibold">No workspaces yet</h1>
        <p className="text-sm text-muted-foreground">
          You&apos;re signed in as <span className="font-medium text-foreground">{email}</span>, but
          you&apos;re not a member of any workspace. Ask a workspace admin to add you with this
          email address. An admin can also add it before your first sign-in.
        </p>
        <div className="flex flex-wrap gap-2">
          <Button onClick={() => qc.invalidateQueries({ queryKey: ["my-workspaces"] })}>
            Check again
          </Button>
          <Button variant="outline" onClick={() => logout.mutate()} disabled={logout.isPending}>
            Sign out
          </Button>
        </div>
      </Card>
    </div>
  );
}
