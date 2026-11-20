import { useQueryClient } from "@tanstack/react-query";

import { useLogout } from "@/api/hooks";
import { Button } from "@/components/ui/button";
import { StandaloneStatePage } from "@/components/layout/StatePage";

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
    <StandaloneStatePage
      title="No workspaces yet"
      actions={
        <>
          <Button onClick={() => qc.invalidateQueries({ queryKey: ["my-workspaces"] })}>
            Check again
          </Button>
          <Button variant="outline" onClick={() => logout.mutate()} disabled={logout.isPending}>
            Sign out
          </Button>
        </>
      }
    >
      <p>
        You&apos;re signed in as <span className="font-medium text-foreground">{email}</span>, but
        you&apos;re not a member of any workspace. Ask a workspace admin to add you with this email
        address. An admin can also add it before your first sign-in.
      </p>
    </StandaloneStatePage>
  );
}
