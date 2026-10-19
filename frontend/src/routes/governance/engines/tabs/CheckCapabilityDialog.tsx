import { useState } from "react";
import { Link } from "react-router-dom";
import { CircleCheck, CircleHelp, CircleX } from "lucide-react";
import type { ApiError } from "@/api/client";
import type {
  CapabilityView,
  EngineGrantsResponse,
  EngineRef,
  ExplainCapabilityResponse,
  Reach,
  WireUndecidableCause,
} from "@/api/generated/model";
import { useControlPlaneGovernanceListCatalogs } from "@/api/generated/control-plane-governance/control-plane-governance";
import { usePermExplainCapability } from "@/api/generated/perm/perm";
import { causeSentence, SubjectUserField } from "@/components/perm/ExplainResult";
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
import { refusalMessage } from "@/routes/governance/policyWrites";
import { engineUrl } from "@/routes/governance/engines/engineUrl";
import {
  CapabilityValueFields,
  catalogChoices,
  valueProblem,
  valueResources,
} from "@/routes/governance/engines/tabs/capabilityValues";

export const UNDECIDABLE_WORDS = "Cannot be decided ahead of time";
export const NO_PROFILE_TO_ASK = "This engine has no profile to ask.";
export const TAGS_DECLINED = "egret cannot resolve classification tags for this question.";
/** F11 §5.4 (E13): every Trino impersonation question on a delegating profile. */
export const TRINO_IMPERSONATION_SENTENCE =
  "egret cannot decide impersonation on this engine: its baseline lets every user become themself through {USER}, which egret's evaluator does not expand.";

/** The deciding policy's source (spec §6.1). */
export type DecidingSource =
  | { kind: "engine_grant"; grantId: string }
  | { kind: "seeded" }
  | { kind: "compiled" }
  | { kind: "hand_written" };

/**
 * Where the policy that decided came from: an engine grant (attribution),
 * egret-seeded (named in the engine's `managed` list), egret-compiled (any
 * other attribution, or a compiler prefix), else hand-written.
 */
export function decidingSource(
  res: ExplainCapabilityResponse,
  managedNames: ReadonlySet<string>,
): DecidingSource | null {
  const name = res.policyName ?? "";
  // A Denied answer names no policy with "" (Task 1, premise 15).
  if (name === "") return null;
  const a = res.attribution;
  if (a?.kind === "engine_grant") return { kind: "engine_grant", grantId: a.engine_grant_id };
  if (managedNames.has(name)) return { kind: "seeded" };
  if (a || name.startsWith("perm-")) return { kind: "compiled" };
  return { kind: "hand_written" };
}

function isUserMarker(c: WireUndecidableCause): boolean {
  const construct = c.kind === "policy_construct" || c.kind === "envelope_construct" ? c.construct : null;
  return construct?.kind === "resource_value_expression_marker" && construct.value.includes("{USER}");
}

/**
 * Check (spec §6.1): asks `POST /perm/explain/capability` whether a user holds
 * a capability on this engine, and shows the answer in words and an icon,
 * never colour alone (FE-A11Y-4).
 */
export function CheckCapabilityDialog({
  open,
  onOpenChange,
  engine,
  reach,
  grants,
  capabilities,
  onCloseAutoFocus,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  engine: EngineRef;
  reach: Reach | null;
  grants: EngineGrantsResponse;
  capabilities: CapabilityView[];
  /** Where focus goes when the dialog closes (it has no trigger of its own). */
  onCloseAutoFocus?: (event: Event) => void;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[85vh] max-w-2xl overflow-y-auto" onCloseAutoFocus={onCloseAutoFocus}>
        {open ? (
          <CheckForm
            engine={engine}
            reach={reach}
            grants={grants}
            capabilities={capabilities}
            close={() => onOpenChange(false)}
          />
        ) : null}
      </DialogContent>
    </Dialog>
  );
}

function CheckForm({
  engine,
  reach,
  grants,
  capabilities,
  close,
}: {
  engine: EngineRef;
  reach: Reach | null;
  grants: EngineGrantsResponse;
  capabilities: CapabilityView[];
  close: () => void;
}) {
  const [user, setUser] = useState("");
  const [capabilityId, setCapabilityId] = useState("");
  const capability = capabilities.find((c) => c.id === capabilityId);
  const [values, setValues] = useState<Record<string, string>>({});
  const [submitted, setSubmitted] = useState(false);
  const catalogsQ = useControlPlaneGovernanceListCatalogs({ query: { retry: false } });
  const catalogs = catalogChoices(catalogsQ, reach);
  const explain = usePermExplainCapability<ApiError>();

  const resources = valueResources(capability);
  const errors: Record<string, string | undefined> = {};
  for (const r of resources) {
    const v = values[r.key] ?? "";
    const problem = valueProblem(v);
    if (problem && (submitted || v !== "")) errors[r.key] = problem;
  }
  const userProblem = submitted && user.trim() === "" ? "Enter a username." : null;

  function ask() {
    setSubmitted(true);
    if (!capability || user.trim() === "") return;
    if (resources.some((r) => valueProblem(values[r.key] ?? "") !== null)) return;
    explain.mutate({
      data: {
        subject: { user: user.trim() },
        engineId: engine.id,
        capability: capability.id,
        values: resources.map((r) => values[r.key]),
      },
    });
  }

  const result = explain.data?.status === 200 ? explain.data.data : null;
  return (
    <>
      <DialogHeader>
        <DialogTitle>Check an engine permission</DialogTitle>
        <DialogDescription>{`Asks what ${engine.name}'s policy set answers for one user. Nothing is changed.`}</DialogDescription>
      </DialogHeader>
      <form
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault();
          ask();
        }}
      >
        <div className="space-y-1">
          <SubjectUserField
            id="check-user"
            value={user}
            onChange={setUser}
            invalid={Boolean(userProblem)}
            describedBy={userProblem ? "check-user-error" : undefined}
          />
          {userProblem ? (
            <p id="check-user-error" className="text-xs text-destructive">
              {userProblem}
            </p>
          ) : null}
        </div>
        <div className="space-y-1.5">
          <Label htmlFor="check-capability">Capability</Label>
          <Select
            value={capabilityId}
            onValueChange={(v) => {
              setCapabilityId(v);
              setValues({});
            }}
          >
            <SelectTrigger
              id="check-capability"
              aria-invalid={submitted && !capability ? true : undefined}
              aria-describedby={submitted && !capability ? "check-capability-error" : undefined}
            >
              <SelectValue placeholder="Choose a capability" />
            </SelectTrigger>
            <SelectContent>
              {capabilities.map((c) => (
                <SelectItem key={c.id} value={c.id}>
                  {c.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          {submitted && !capability ? (
            <p id="check-capability-error" className="text-xs text-destructive">
              Choose a capability.
            </p>
          ) : null}
        </div>
        {capability ? (
          <CapabilityValueFields
            idPrefix="check-value"
            capability={capability}
            values={values}
            onChange={(key, v) => setValues((prev) => ({ ...prev, [key]: v }))}
            errors={errors}
            catalogs={catalogs}
          />
        ) : null}

        {explain.isError ? (
          <Alert variant="destructive">
            <AlertDescription>
              {refusalMessage(explain.error, "policy:read", "The question could not be asked.")}
            </AlertDescription>
          </Alert>
        ) : null}
        {result ? <CheckAnswer result={result} engine={engine} grants={grants} capabilityId={capabilityId} /> : null}

        <DialogFooter>
          <Button type="button" variant="outline" onClick={close}>
            Close
          </Button>
          <Button type="submit" disabled={explain.isPending}>
            {explain.isPending ? "Checking…" : "Check"}
          </Button>
        </DialogFooter>
      </form>
    </>
  );
}

function CheckAnswer({
  result,
  engine,
  grants,
  capabilityId,
}: {
  result: ExplainCapabilityResponse;
  engine: EngineRef;
  grants: EngineGrantsResponse;
  capabilityId: string;
}) {
  const body = (() => {
    if (result.outcome === "no_profile" || !result.decision) return <p className="text-sm">{NO_PROFILE_TO_ASK}</p>;
    const d = result.decision;
    const source = decidingSource(result, new Set(grants.managed.map((m) => m.policyName)));
    const verdict =
      d.kind === "allowed" ? (
        <>
          <CircleCheck className="h-4 w-4 text-success" aria-hidden />
          <span>Allowed</span>
        </>
      ) : d.kind === "denied" ? (
        <>
          <CircleX className="h-4 w-4 text-destructive" aria-hidden />
          <span>Denied</span>
        </>
      ) : (
        <>
          <CircleHelp className="h-4 w-4 text-muted-foreground" aria-hidden />
          <span>{UNDECIDABLE_WORDS}</span>
        </>
      );
    return (
      <div className="space-y-2 text-sm">
        <p className="flex items-center gap-1.5 font-medium">{verdict}</p>
        {source ? (
          <p>
            {"Decided by "}
            <span className="font-mono text-xs">{result.policyName}</span>
            {" ("}
            {source.kind === "engine_grant" ? (
              <Link
                to={engineUrl(engine.id, undefined, { grant: source.grantId })}
                className="text-primary underline focus-visible:ring-2 focus-visible:ring-ring"
              >
                egret engine grant
              </Link>
            ) : (
              <span>
                {source.kind === "seeded" ? "egret-seeded" : source.kind === "compiled" ? "egret-compiled" : "hand-written"}
              </span>
            )}
            {")."}
          </p>
        ) : d.kind === "denied" ? (
          <p className="text-muted-foreground">No policy grants it.</p>
        ) : null}
        {d.kind === "undecidable" ? (
          <>
            {engine.type === "trino" && capabilityId === "trino.impersonate_user" && d.why.some(isUserMarker) ? (
              <p>{TRINO_IMPERSONATION_SENTENCE}</p>
            ) : null}
            {result.tagResolution === "declined" ? <p>{TAGS_DECLINED}</p> : null}
            <ul role="list" className="list-disc space-y-0.5 pl-5 text-muted-foreground">
              {d.why.map((c, i) => (
                // The same cause can repeat (one per policy item), so the index is part of the key.
                <li key={`${c.kind}-${i}`}>{causeSentence(c)}</li>
              ))}
            </ul>
          </>
        ) : result.tagResolution === "declined" ? (
          <p>{TAGS_DECLINED}</p>
        ) : null}
      </div>
    );
  })();
  return (
    <section aria-label="Answer" className="rounded-md border border-border p-3">
      {body}
    </section>
  );
}
