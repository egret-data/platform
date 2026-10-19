import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import type { ApiError } from "@/api/client";
import type {
  CapabilityView,
  EngineGrantView,
  EngineGrantWriteResponse,
  EngineRef,
  NewEngineGrant,
  PermPrincipal,
  Reach,
} from "@/api/generated/model";
import { useControlPlaneGovernanceListCatalogs } from "@/api/generated/control-plane-governance/control-plane-governance";
import {
  getPermListAllEngineGrantsQueryKey,
  getPermListEngineGrantsQueryKey,
  usePermCreateEngineGrant,
  usePermPatchEngineGrant,
} from "@/api/generated/perm/perm";
import { getPolicyAdminListPoliciesQueryKey } from "@/api/generated/policy-admin/policy-admin";
import { PrincipalPicker } from "@/components/perm/PrincipalPicker";
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
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { useUnsavedChangesWarning } from "@/lib/useUnsavedChangesWarning";
import { refusalMessage } from "@/routes/governance/policyWrites";
import { EXTERNAL_SENTENCE } from "@/routes/governance/engines/PageFrame";
import {
  CapabilityValueFields,
  reachedCatalogs,
  valueProblem,
  valueResources,
} from "@/routes/governance/engines/tabs/capabilityValues";

export const NO_PRINCIPAL = "Add at least one user, group or role.";

/** The success sentence of Q4's "compiled" outcome, with the other engines the grant reaches. */
export function compiledSentence(profile: string, engineId: string, reach: Reach, external: boolean): string {
  const others = reach.engines.filter((e) => e.id !== engineId).map((e) => e.name);
  let s = `Saved and compiled into ${profile}. The engine applies it at its next plugin download.`;
  if (others.length > 0) s += ` It reaches ${others.join(", ")}.`;
  if (external) s += ` ${EXTERNAL_SENTENCE}`;
  return s;
}

/** Q4's "saved but not compiled": the compile skipped this grant. `null` when it compiled. */
export function skippedReason(res: EngineGrantWriteResponse): string | null {
  const skip = res.compile.skipped.find((s) => s.source_kind === "engine_grant" && s.id === res.id);
  return skip ? skip.reason : null;
}

/** Values by resource key, from a stored grant (values are in key order, `fixed` keys skipped). */
function valuesOf(grant: EngineGrantView | null | undefined, cap: CapabilityView | undefined): Record<string, string> {
  const out: Record<string, string> = {};
  if (!grant || !cap) return out;
  valueResources(cap).forEach((r, i) => {
    out[r.key] = grant.values[i] ?? "";
  });
  return out;
}

/**
 * Create or edit an engine grant (spec §6.1, EP4). The form mounts only while
 * open, so each open starts from `existing` (or blank). Outcomes per Q4: any
 * refusal is "not saved" and keeps the dialog open with every input; a skip
 * of this grant is reported by `onSkipped` on the tab; otherwise a toast says
 * it compiled.
 */
export function EngineGrantDialog({
  open,
  onOpenChange,
  engine,
  profile,
  reach,
  capabilities,
  existing,
  onSkipped,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  engine: EngineRef;
  profile: string;
  reach: Reach | null;
  /** The engine type's capabilities, in the catalogue's order. */
  capabilities: CapabilityView[];
  existing?: EngineGrantView | null;
  onSkipped: (reason: string | null) => void;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[85vh] max-w-2xl overflow-y-auto">
        {open ? (
          <GrantForm
            engine={engine}
            profile={profile}
            reach={reach}
            capabilities={capabilities}
            existing={existing ?? null}
            onSkipped={onSkipped}
            close={() => onOpenChange(false)}
          />
        ) : null}
      </DialogContent>
    </Dialog>
  );
}

function GrantForm({
  engine,
  profile,
  reach,
  capabilities,
  existing,
  onSkipped,
  close,
}: {
  engine: EngineRef;
  profile: string;
  reach: Reach | null;
  capabilities: CapabilityView[];
  existing: EngineGrantView | null;
  onSkipped: (reason: string | null) => void;
  close: () => void;
}) {
  const [capabilityId, setCapabilityId] = useState(existing?.capability ?? "");
  const capability = capabilities.find((c) => c.id === capabilityId);
  const [values, setValues] = useState<Record<string, string>>(() => valuesOf(existing, capability));
  const [principals, setPrincipals] = useState<PermPrincipal[]>(existing?.principals ?? []);
  const [submitted, setSubmitted] = useState(false);
  const [serverError, setServerError] = useState<string | null>(null);
  const snapshot = JSON.stringify({ capabilityId, values, principals });
  const [initial] = useState(snapshot);
  useUnsavedChangesWarning(snapshot !== initial);

  const catalogsQ = useControlPlaneGovernanceListCatalogs({ query: { retry: false } });
  const catalogs = reachedCatalogs(catalogsQ.data?.status === 200 ? catalogsQ.data.data : [], reach);

  const queryClient = useQueryClient();
  const onSuccess = (res: { status: number; data: unknown }) => {
    if (res.status !== 200) return;
    const body = res.data as EngineGrantWriteResponse;
    void queryClient.invalidateQueries({ queryKey: getPermListEngineGrantsQueryKey(engine.id) });
    void queryClient.invalidateQueries({ queryKey: getPermListAllEngineGrantsQueryKey() });
    void queryClient.invalidateQueries({ queryKey: getPolicyAdminListPoliciesQueryKey({ service: profile }) });
    const skipped = skippedReason(body);
    onSkipped(skipped);
    if (skipped === null) {
      toast.success(compiledSentence(profile, engine.id, body.reach, engine.origin === "external"));
    }
    close();
  };
  const onError = (e: ApiError) =>
    setServerError(`Not saved: ${refusalMessage(e, "policy:update", "The engine permission could not be saved.")}`);
  const create = usePermCreateEngineGrant<ApiError>({ mutation: { onSuccess, onError } });
  const patch = usePermPatchEngineGrant<ApiError>({ mutation: { onSuccess, onError } });
  const saving = create.isPending || patch.isPending;

  const resources = valueResources(capability);
  const errors: Record<string, string | undefined> = {};
  for (const r of resources) {
    const v = values[r.key] ?? "";
    // An empty field is flagged only once Grant was pressed; a malformed one at once.
    const problem = valueProblem(v);
    if (problem && (submitted || v !== "")) errors[r.key] = problem;
  }
  const principalProblem = submitted && principals.length === 0 ? NO_PRINCIPAL : null;
  // A refusal of `public` belongs beside the principals (spec §6.1); every other one under the form.
  const principalRefusal = serverError && /\bpublic\b/i.test(serverError) ? serverError : null;
  const formRefusal = serverError && !principalRefusal ? serverError : null;

  function save() {
    setSubmitted(true);
    setServerError(null);
    if (!capability) return;
    const blocked = resources.some((r) => valueProblem(values[r.key] ?? "") !== null) || principals.length === 0;
    if (blocked) return;
    const body: NewEngineGrant = {
      capability: capability.id,
      values: resources.map((r) => values[r.key]),
      principals,
    };
    if (existing) patch.mutate({ id: existing.id, data: body });
    else create.mutate({ engineId: engine.id, data: body });
  }

  const title = existing ? "Edit engine permission" : "Grant engine permission";
  return (
    <>
      <DialogHeader>
        <DialogTitle>{title}</DialogTitle>
        <DialogDescription>
          {`On ${engine.name}, through profile ${profile}. egret compiles it into the profile; the engine applies it at its next plugin download.`}
        </DialogDescription>
      </DialogHeader>
      <form
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault();
          save();
        }}
      >
        <div className="space-y-1.5">
          <Label htmlFor="engine-grant-capability">Capability</Label>
          <Select
            value={capabilityId}
            onValueChange={(v) => {
              setCapabilityId(v);
              setValues({});
              setServerError(null);
            }}
          >
            <SelectTrigger
              id="engine-grant-capability"
              // FE-A11Y-6: the dialog the user opened focuses its first field.
              autoFocus
              aria-invalid={submitted && !capability ? true : undefined}
              aria-describedby={submitted && !capability ? "engine-grant-capability-error" : undefined}
            >
              <SelectValue placeholder="Choose a capability" />
            </SelectTrigger>
            <SelectContent>
              {capabilities.map((c) => (
                <SelectItem key={c.id} value={c.id}>
                  <span className="flex flex-col items-start">
                    <span>{c.label}</span>
                    <span className="text-xs text-muted-foreground">{c.description}</span>
                  </span>
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          {submitted && !capability ? (
            <p id="engine-grant-capability-error" className="text-xs text-destructive">
              Choose a capability.
            </p>
          ) : null}
        </div>

        {capability ? (
          <CapabilityValueFields
            idPrefix="engine-grant-value"
            capability={capability}
            values={values}
            onChange={(key, v) => {
              setValues((prev) => ({ ...prev, [key]: v }));
              setServerError(null);
            }}
            errors={errors}
            catalogs={catalogs}
          />
        ) : null}

        <fieldset className="space-y-1.5" aria-describedby="engine-grant-principals-help">
          <legend className="text-sm font-medium">Granted to</legend>
          <p id="engine-grant-principals-help" className="text-xs text-muted-foreground">
            Users, groups or roles. A group is expanded to its current members.
          </p>
          <PrincipalPicker
            includeOnly
            value={{ include: principals, exclude: [] }}
            onChange={(next) => {
              setPrincipals(next.include);
              setServerError(null);
            }}
          />
          {principalProblem ? <p className="text-xs text-destructive">{principalProblem}</p> : null}
          {principalRefusal ? (
            <p role="alert" className="text-xs text-destructive">
              {principalRefusal}
            </p>
          ) : null}
        </fieldset>

        {formRefusal ? (
          <Alert variant="destructive">
            <AlertDescription>{formRefusal}</AlertDescription>
          </Alert>
        ) : null}

        <DialogFooter>
          <Button type="button" variant="outline" onClick={close}>
            Cancel
          </Button>
          <Button type="submit" disabled={saving}>
            {saving ? "Saving…" : existing ? "Save changes" : "Grant"}
          </Button>
        </DialogFooter>
      </form>
    </>
  );
}
