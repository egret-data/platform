import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Loader2, ShieldCheck } from "lucide-react";
import { toast } from "sonner";

import type { ApiError } from "@/api/client";
import type { Engine } from "@/api/controlPlane";
import { useControlPlaneEnginesSecurityUpgrade } from "@/api/generated/control-plane-engines/control-plane-engines";
import type { SecurityUpgradeDto, SecurityUpgradeReason } from "@/api/generated/model";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { StatusBadge } from "@/components/ui/status-badge";
import { WarningCallout, useCan } from "./externalEngine";
import { engineDetailKey, enginesQueryKey } from "./queryKeys";

/** The fields of the engine DTO these warnings read (spec 2026-11-09-starrocks-engine-posture). */
type Posture = Pick<
  Engine,
  | "id"
  | "image"
  | "notGoverned"
  | "securityUpgradeNeeded"
  | "securityUpgradeReasons"
  | "securityUpgradeFeImage"
  | "securityUpgradeBlockedRelease"
>;

export const NOT_GOVERNED_SENTENCE =
  "Not governed: this engine is bound to no policy profile, so egret's grants, masks and row filters do not reach it. Bind it to a StarRocks profile in its settings.";

export const SECURITY_UPGRADE_BADGE_TITLE =
  "This StarRocks engine runs without some of egret's security fixes. Open it and press Upgrade security.";

/** One sentence per reason, in the server's order. */
export const SECURITY_UPGRADE_REASON_SENTENCE: Record<SecurityUpgradeReason, string> = {
  unpatchedImage:
    "Its FE image is not egret-starrocks-fe 4.1.7-egret.4 or later, so StarRocks lets users past this engine's profile (Iceberg metadata tables, prepared statements, the HTTP SQL API, FILES() and more).",
  noRootPassword:
    "Its FE `root` account has no password: anyone who reaches port 9030 can log in as root.",
  notApplied: "The last security upgrade did not finish applying. Run it again.",
};

/**
 * The engine list's posture badges, beside the status: "Not governed" (KI-336)
 * and "Security upgrade needed", each styled like Trino's "Re-apply needed".
 */
export function EnginePostureBadges({ engine }: { engine: Posture }) {
  return (
    <>
      {engine.notGoverned && (
        <StatusBadge tone="highlight" title={NOT_GOVERNED_SENTENCE}>
          Not governed
        </StatusBadge>
      )}
      {engine.securityUpgradeNeeded && (
        <StatusBadge tone="highlight" title={SECURITY_UPGRADE_BADGE_TITLE}>
          Security upgrade needed
        </StatusBadge>
      )}
    </>
  );
}

/** The engine page's "Not governed" warning; the backend decides when it applies. */
export function NotGovernedWarning({ engine }: { engine: Posture }) {
  if (!engine.notGoverned) return null;
  return <WarningCallout testId="starrocks-not-governed">{NOT_GOVERNED_SENTENCE}</WarningCallout>;
}

/** What the toast says changed. */
function upgradeOutcome(res: SecurityUpgradeDto): string {
  const changed = [
    res.imageChanged ? "the FE and CN images were updated" : null,
    res.rootPasswordSet ? "the root password was set" : null,
  ].filter((s): s is string => s !== null);
  if (changed.length === 0) return "Security upgrade applied. The FE and CN pods are restarting.";
  return `Security upgraded: ${changed.join(" and ")}. The FE and CN pods are restarting.`;
}

/**
 * A provisioned StarRocks engine's security-upgrade warning: one sentence per
 * reason, and the **Upgrade security** button (`engine:update`) with its
 * confirmation dialog. A failed apply (502) leaves the engine flagged
 * `notApplied`, so the error is shown, the engine is re-read and the button
 * stays for the repeat that finishes the run.
 */
export function SecurityUpgradeWarning({ engine }: { engine: Posture }) {
  const queryClient = useQueryClient();
  const canUpdate = useCan("engine:update");
  const [open, setOpen] = useState(false);

  const settle = () => {
    void queryClient.invalidateQueries({ queryKey: engineDetailKey(engine.id) });
    void queryClient.invalidateQueries({ queryKey: enginesQueryKey });
  };

  const upgrade = useControlPlaneEnginesSecurityUpgrade<ApiError>({
    mutation: {
      onSuccess: (res) => {
        setOpen(false);
        settle();
        if (res.status !== 200) return;
        toast.success(upgradeOutcome(res.data));
        if (res.data.imageKeptBecause) toast.warning(res.data.imageKeptBecause);
      },
      onError: (e) => {
        setOpen(false);
        settle();
        toast.error(e.message || "The security upgrade failed. Try again.");
      },
    },
  });

  const reasons = engine.securityUpgradeReasons ?? [];
  if (!engine.securityUpgradeNeeded || reasons.length === 0) return null;
  const noRootPassword = reasons.includes("noRootPassword");

  return (
    <WarningCallout testId="starrocks-security-upgrade">
      This engine needs a security upgrade.
      <ul className="mt-1 list-disc space-y-1 pl-5">
        {reasons.map((r) => (
          <li key={r}>{SECURITY_UPGRADE_REASON_SENTENCE[r]}</li>
        ))}
      </ul>
      {canUpdate ? (
        <Button
          type="button"
          size="sm"
          variant="outline"
          className="mt-2"
          onClick={() => setOpen(true)}
          disabled={upgrade.isPending}
        >
          {upgrade.isPending ? (
            <Loader2 className="h-4 w-4 animate-spin" aria-hidden />
          ) : (
            <ShieldCheck className="h-4 w-4" aria-hidden />
          )}
          Upgrade security
        </Button>
      ) : (
        <p className="mt-2">Upgrading it needs the engine:update permission.</p>
      )}

      <AlertDialog open={open} onOpenChange={setOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Upgrade this engine's security?</AlertDialogTitle>
            <AlertDialogDescription asChild>
              <div className="space-y-2">
                <ul className="list-disc space-y-1 pl-5" data-testid="security-upgrade-changes">
                  {engine.securityUpgradeFeImage ? (
                    <>
                      <li className="break-all">
                        FE image: {engine.image ?? "unknown"} → {engine.securityUpgradeFeImage}
                      </li>
                      <li>The CN image moves to the same release.</li>
                    </>
                  ) : null}
                  {engine.securityUpgradeBlockedRelease ? (
                    <li>
                      The image is not changed: no patched image for release{" "}
                      {engine.securityUpgradeBlockedRelease} yet.
                    </li>
                  ) : null}
                  {noRootPassword ? <li>root password will be set</li> : null}
                  {reasons.includes("notApplied") ? <li>The stored changes are applied again.</li> : null}
                </ul>
                <p>
                  The FE and CN pods restart. A client outside egret that logs in as root with no password is
                  refused afterwards.
                </p>
              </div>
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={(e) => {
                e.preventDefault();
                upgrade.mutate({ id: engine.id });
              }}
              disabled={upgrade.isPending}
            >
              {upgrade.isPending && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
              Upgrade security
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </WarningCallout>
  );
}
