import { useEffect, useState } from "react";
import { toast } from "sonner";
import { useQueryClient } from "@tanstack/react-query";
import { Button } from "@/components/ui/button";
import { useExitImpersonation, useMe } from "@/api/hooks";

/** `mm:ss` remaining until `expiresAt`, floored at `00:00`. */
function formatRemaining(expiresAt: string, now: number): string {
  const totalSeconds = Math.max(0, Math.floor((new Date(expiresAt).getTime() - now) / 1000));
  const mm = Math.floor(totalSeconds / 60);
  const ss = totalSeconds % 60;
  return `${String(mm).padStart(2, "0")}:${String(ss).padStart(2, "0")}`;
}

/**
 * Persistent, unmissable bar shown on every authed page while an admin is
 * acting as another user (see `Me.impersonating`). Mounted once in
 * `AppShell` so it can never be scrolled past or forgotten.
 */
export function ImpersonationBanner() {
  const { data: me } = useMe();
  const impersonating = me?.impersonating;
  const exitImpersonation = useExitImpersonation();
  const qc = useQueryClient();

  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    if (!impersonating) return;
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, [impersonating]);

  // Once the countdown hits zero the backend has already reverted the
  // session on its next request — refresh `["me"]` so the banner clears
  // without requiring the admin to navigate or take any action.
  const expired = impersonating ? new Date(impersonating.expires_at).getTime() - now <= 0 : false;
  useEffect(() => {
    if (expired) qc.invalidateQueries({ queryKey: ["me"] });
  }, [expired, qc]);

  if (!impersonating) return null;

  function handleExit() {
    exitImpersonation.mutate(undefined, {
      onSuccess: () => toast.success("Returned to your account"),
      onError: (err) => toast.error(err instanceof Error ? err.message : "Failed to exit impersonation"),
    });
  }

  return (
    <div
      role="status"
      className="flex shrink-0 items-center justify-center gap-3 bg-highlight px-4 py-2 text-sm font-bold text-highlight-foreground"
    >
      <span>
        Impersonating <span className="underline">{impersonating.target.email}</span>
        {" · "}
        {expired ? "expired" : `${formatRemaining(impersonating.expires_at, now)} left`}
      </span>
      <Button
        variant="outline"
        size="sm"
        className="h-6 border-highlight-foreground/40 bg-transparent px-2 text-highlight-foreground hover:bg-highlight-foreground/10"
        onClick={handleExit}
        disabled={exitImpersonation.isPending}
      >
        Exit
      </Button>
    </div>
  );
}
