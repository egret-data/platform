import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Loader2 } from "lucide-react";
import { toast } from "sonner";
import {
  getPolicyAdminListProfilesQueryKey,
  usePolicyAdminSetMechanism,
} from "@/api/generated/policy-admin/policy-admin";
import { MechanismName, type ProfileMechanism } from "@/api/generated/model";
import type { ApiError } from "@/api/client";
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
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { refusalMessage, usePolicyPermissions } from "@/routes/governance/policyWrites";
import { Section } from "@/routes/governance/tabParts";
import type { FrameProfile } from "@/routes/governance/engines/PageFrame";

export const MECHANISM_RESTARTS =
  "Every engine bound to this profile restarts its coordinator to load the new access control. Queries running on it fail.";
export const MECHANISM_EXTERNAL =
  "External engines are not restarted by egret. Their operator must install the new plugin files, and run Trino 483 or later with plugin/opa.";
export const MECHANISM_NEEDS_UPDATE = "You need policy:update to change this.";

const LABEL: Record<MechanismName, string> = { ranger: "Ranger", opa: "OPA" };

/** What the server wrote on a 502: the switch is committed, some engines were not reconfigured. */
interface PartialSwitch {
  error: string;
  reconfigured: string[];
  unchanged: string[];
  failed: Array<{ engine: string; error: string }>;
  warning: string | null;
}

/** What a 409 carries: every reason the switch is refused, one string each. */
function refusalsOf(e: unknown): string[] | null {
  const body = (e as ApiError | undefined)?.body as { refusals?: unknown } | undefined;
  if (!Array.isArray(body?.refusals)) return null;
  return body.refusals.filter((r): r is string => typeof r === "string");
}

function partialOf(e: unknown): PartialSwitch {
  const err = e as unknown as ApiError;
  const body = (err.body ?? {}) as {
    engines_reconfigured?: string[];
    external_engines_unchanged?: string[];
    engines_failed?: Array<{ engine: string; error: string }>;
    external_engines_warning?: string | null;
  };
  return {
    error: err.message,
    reconfigured: body.engines_reconfigured ?? [],
    unchanged: body.external_engines_unchanged ?? [],
    failed: body.engines_failed ?? [], warning: body.external_engines_warning || null,
  };
}

function doneLines(res: ProfileMechanism): string[] {
  const lines: string[] = [`Switched to ${LABEL[res.mechanism]}.`];
  if (res.engines_reconfigured.length > 0) lines.push(`Reconfigured: ${res.engines_reconfigured.join(", ")}.`);
  if (res.external_engines_unchanged.length > 0) {
    lines.push(`Not restarted (external): ${res.external_engines_unchanged.join(", ")}.`);
  }
  if (res.external_engines_warning) lines.push(res.external_engines_warning);
  return lines;
}

/**
 * Access control (SP2b Task 23): which mechanism the engines bound to a Trino
 * profile are asked, Ranger or OPA (`PATCH /policies/services/{service}`).
 * A switch restarts the bound coordinators, so it is confirmed first; a 409
 * lists every refusal and leaves the selection on the stored mechanism; a 502
 * means the switch is committed but some engines are not reconfigured, and
 * Retry sends the same PATCH again.
 */
export function MechanismSection({ profile }: { profile: FrameProfile }) {
  const can = usePolicyPermissions();
  const qc = useQueryClient();
  const setMechanism = usePolicyAdminSetMechanism();
  const [target, setTarget] = useState<MechanismName | null>(null);
  const [refusals, setRefusals] = useState<string[]>([]);
  const [failure, setFailure] = useState<{ target: MechanismName; partial: PartialSwitch } | null>(null);
  const [otherError, setOtherError] = useState<string | null>(null);
  const [done, setDone] = useState<string[]>([]);

  const dto = profile.dto;
  // The stored value is a plain string on the wire (T5 m6); a value that is neither is shown as Unknown.
  const current: MechanismName | null =
    dto?.mechanism === MechanismName.opa ? MechanismName.opa : dto?.mechanism === MechanismName.ranger ? MechanismName.ranger : null;
  const editable = !!dto && current !== null && can("policy:update");
  const hasExternal = (dto?.engines ?? []).some((e) => e.origin === "external");

  const send = (mechanism: MechanismName) => {
    setRefusals([]);
    setFailure(null);
    setOtherError(null);
    setDone([]);
    setMechanism.mutate(
      { service: profile.name, data: { mechanism } },
      {
        onSuccess: (res) => {
          void qc.invalidateQueries({ queryKey: getPolicyAdminListProfilesQueryKey() });
          toast.success(`Switched to ${LABEL[mechanism]}.`);
          if (res.status === 200) setDone(doneLines(res.data));
        },
        onError: (e) => {
          const err = e as unknown as ApiError;
          if (err.status === 409) {
            setRefusals(refusalsOf(e) ?? [err.message]);
          } else if (err.status === 502) {
            // The switch is committed: the list shows the new mechanism.
            void qc.invalidateQueries({ queryKey: getPolicyAdminListProfilesQueryKey() });
            setFailure({ target: mechanism, partial: partialOf(e) });
          } else {
            setOtherError(refusalMessage(e, "policy:update", "The access control could not be changed."));
          }
        },
      },
    );
  };

  const confirm = () => {
    if (target) send(target);
    setTarget(null);
  };

  return (
    <Section title="Access control">
      <div className="space-y-3">
        <p id="mechanism-label" className="text-sm font-medium">
          How engines are asked
        </p>
        <RadioGroup
          aria-labelledby="mechanism-label"
          value={current ?? ""}
          disabled={!editable || setMechanism.isPending}
          onValueChange={(v) => {
            if (v !== current) setTarget(v as MechanismName);
          }}
        >
          {Object.values(MechanismName).map((m) => (
            <div key={m} className="flex items-center gap-2">
              <RadioGroupItem value={m} id={`mechanism-${m}`} />
              <Label htmlFor={`mechanism-${m}`}>{LABEL[m]}</Label>
            </div>
          ))}
        </RadioGroup>
        {!dto ? <p className="text-sm text-muted-foreground">The profile has not loaded yet.</p> : null}
        {dto && current === null ? (
          <p className="text-sm text-muted-foreground">Current mechanism: Unknown ({dto.mechanism}).</p>
        ) : null}
        {dto && current !== null && !editable ? (
          <p className="text-sm text-muted-foreground">{MECHANISM_NEEDS_UPDATE}</p>
        ) : null}
        {setMechanism.isPending ? <Loader2 className="h-4 w-4 animate-spin" aria-label="Switching" /> : null}
        {/* Rendered only with content: the seed line above it on this tab is the page's one other status. */}
        {done.length > 0 ? (
          <div role="status" className="space-y-1 text-sm">
            {done.map((l) => (
              <p key={l}>{l}</p>
            ))}
          </div>
        ) : null}
        {refusals.length > 0 ? (
          <Alert variant="destructive">
            <AlertDescription className="space-y-1">
              {refusals.map((r) => (
                <p key={r}>{r}</p>
              ))}
            </AlertDescription>
          </Alert>
        ) : null}
        {otherError ? (
          <Alert variant="destructive">
            <AlertDescription>{otherError}</AlertDescription>
          </Alert>
        ) : null}
        {failure ? (
          <Alert variant="destructive">
            <AlertDescription className="space-y-2">
              <p>{failure.partial.error}</p>
              <ul className="list-disc pl-5">
                {failure.partial.failed.map((f) => (
                  <li key={f.engine}>
                    {f.engine}: {f.error}
                  </li>
                ))}
              </ul>
              {failure.partial.reconfigured.length > 0 ? (
                <p>Reconfigured: {failure.partial.reconfigured.join(", ")}.</p>
              ) : null}
              {failure.partial.unchanged.length > 0 ? (
                <p>Not restarted (external): {failure.partial.unchanged.join(", ")}.</p>
              ) : null}
              {failure.partial.warning ? <p>{failure.partial.warning}</p> : null}
              <Button size="sm" variant="outline" disabled={setMechanism.isPending} onClick={() => send(failure.target)}>
                Retry
              </Button>
            </AlertDescription>
          </Alert>
        ) : null}
      </div>
      <AlertDialog open={target !== null} onOpenChange={(o) => !o && setTarget(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Switch to {target ? LABEL[target] : ""}?</AlertDialogTitle>
            <AlertDialogDescription asChild>
              <div className="space-y-2">
                <p>{MECHANISM_RESTARTS}</p>
                {target === MechanismName.opa && hasExternal ? <p>{MECHANISM_EXTERNAL}</p> : null}
              </div>
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction onClick={confirm}>Switch</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Section>
  );
}
