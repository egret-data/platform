import { useState } from "react";

import { ApiError } from "@/api/client";
import { useStartPlatformImpersonation, useStartWsImpersonation } from "@/api/impersonation";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";

export interface ImpersonateDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  targetUserId: string;
  targetEmail: string;
  /** Workspace slug/id: when set, starts a workspace-scoped impersonation
   * (`POST /api/ws/{workspace}/impersonate`) instead of the platform one. */
  workspaceScope?: string;
  /** Called after the overlay is successfully created, right before the dialog closes. */
  onStarted?: () => void;
}

/**
 * Start-impersonation dialog (Task 11). Collects a required reason and an
 * optional step-up password, then calls the platform or workspace start
 * endpoint depending on `workspaceScope`.
 *
 * `Me` doesn't expose whether the current admin authenticates locally or via
 * OIDC (see `MeResponse` — no provider field), so the password field is always
 * shown but is optional: a local admin fills it in for step-up; an OIDC admin
 * leaves it blank and the backend responds `409 oidc_step_up`, which
 * `useStartPlatformImpersonation`/`useStartWsImpersonation` turn into a
 * redirect to the IdP. The password is simply unused in that case.
 */
export function ImpersonateDialog({
  open,
  onOpenChange,
  targetUserId,
  targetEmail,
  workspaceScope,
  onStarted,
}: ImpersonateDialogProps) {
  const [reason, setReason] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);

  // Rules of hooks: both start hooks are constructed unconditionally; only the
  // one matching `workspaceScope` is ever invoked below.
  const platformImpersonate = useStartPlatformImpersonation();
  const wsImpersonate = useStartWsImpersonation(workspaceScope ?? "");
  const start = workspaceScope ? wsImpersonate : platformImpersonate;

  function reset() {
    setReason("");
    setPassword("");
    setError(null);
    start.reset();
  }

  function handleOpenChange(next: boolean) {
    if (!next) reset();
    onOpenChange(next);
  }

  const canSubmit = reason.trim().length > 0 && !start.isPending;

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!canSubmit) return;
    setError(null);
    try {
      await start.mutateAsync({
        target_user_id: targetUserId,
        reason: reason.trim(),
        password: password.length > 0 ? password : undefined,
      });
      onStarted?.();
      handleOpenChange(false);
    } catch (err) {
      // A 409 oidc_step_up redirects the whole browser away (handled inside
      // the start hook's onError) — there's nothing left to show in the dialog.
      if (err instanceof ApiError && err.status === 409 && err.code === "oidc_step_up") return;
      setError(err instanceof Error ? err.message : "Failed to start impersonation.");
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>Impersonate user</DialogTitle>
            <DialogDescription>
              You&apos;ll act as this user until the session expires or you exit. This is
              logged and requires a reason.
            </DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            {error && (
              <Alert variant="destructive">
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            )}

            <div className="space-y-1.5">
              <Label htmlFor="impersonate-target">Target user</Label>
              <Input id="impersonate-target" value={targetEmail} readOnly disabled />
            </div>

            <div className="space-y-1.5">
              <div className="flex items-center gap-0.5">
                <Label htmlFor="impersonate-reason">Reason</Label>
                <span className="text-destructive" aria-hidden>
                  *
                </span>
              </div>
              <Textarea
                id="impersonate-reason"
                value={reason}
                onChange={(e) => setReason(e.target.value)}
                placeholder="Why are you impersonating this user?"
                required
                autoFocus
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="impersonate-password">Password</Label>
              <Input
                id="impersonate-password"
                type="password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                autoComplete="current-password"
              />
              <p className="text-xs text-muted-foreground">
                Confirm with your password if you sign in locally. Signing in with SSO?
                Leave this blank — you&apos;ll be redirected to re-authenticate.
              </p>
            </div>
          </div>

          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => handleOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={!canSubmit}>
              {start.isPending ? "Starting…" : "Impersonate"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
