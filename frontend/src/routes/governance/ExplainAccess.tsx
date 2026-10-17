import { useState } from "react";
import { Loader2, Search } from "lucide-react";

import { usePermExplain } from "@/api/generated/perm/perm";
import type { Action, ExplainResponse, Scope } from "@/api/generated/model";
import { ApiError } from "@/api/client";
import { ExplainResultView, SubjectUserField } from "@/components/perm/ExplainResult";
import { ScopePicker } from "@/components/perm/ScopePicker";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

const EMPTY_SCOPE: Scope = { catalog_id: "" };


/**
 * Governance → Explain access (D9; rerouted onto the served artifact by
 * SP4d-2a).
 *
 * Admin-facing: subject + action + target in; one verdict per POLICY PROFILE
 * out, three-valued, over the policy set each engine actually downloads. There
 * is deliberately no aggregate badge: no single word is honest over
 * `{allowed, refused, undecidable}` × N profiles, and a summary reading
 * "Allowed" above a profile that refuses would be the most misleading thing on
 * the page.
 *
 * The subject is a user and nothing else. `workspaces` (SP4a), `roles` (SP4c)
 * and `groups` (SP4d-2a) each left `ExplainSubject` for one version of one
 * reason — a caller-supplied value cannot stand in for a fact only egret can
 * verify — and roles and groups reach the evaluator EMPTY because no engine
 * egret provisions can match either.
 *
 * Gated `policy:read` at the route (`App.tsx`), not `policy:update` — this is
 * the one route on this surface a read-only user can call, because `explain`
 * opens no transaction and writes nothing.
 */
export default function ExplainAccess() {
  const [user, setUser] = useState("");
  const [action, setAction] = useState<Action>("read");
  const [target, setTarget] = useState<Scope>(EMPTY_SCOPE);

  const [result, setResult] = useState<ExplainResponse | null>(null);
  const [explainError, setExplainError] = useState<ApiError | null>(null);

  const explainMutation = usePermExplain<ApiError>();

  const canSubmit = user.trim().length > 0 && target.catalog_id.trim().length > 0;
  /**
   * Why `Explain` is disabled, in words (SP4b-2 finding 2). A disabled button
   * with no explanation is the failure this finding is about: with
   * `policy:read` alone the target picker once held zero options (it read the
   * registry list, `catalog_registry:read`); it reads the Governance list now
   * and says so itself when even that is refused — but the button must not
   * simply sit there inert either way.
   */
  const missing = [
    user.trim().length === 0 ? "a user" : null,
    target.catalog_id.trim().length === 0 ? "a target catalog" : null,
  ]
    .filter(Boolean)
    .join(" and ");

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!canSubmit) return;
    setExplainError(null);
    try {
      const res = await explainMutation.mutateAsync({
        data: {
          // A user and nothing else — see the component doc.
          subject: { user: user.trim() },
          action,
          target,
        },
      });
      setResult(res.status === 200 ? res.data : null);
    } catch (err) {
      setResult(null);
      setExplainError(err as ApiError);
    }
  }

  const forbidden = explainError?.status === 403;

  return (
    <div className="space-y-5">
      <div>
        <h1 className="text-2xl font-semibold leading-tight">Explain access</h1>
        <p className="text-sm text-muted-foreground">
          Why a subject can or cannot reach a target — answered per policy profile, over
          the policy set each engine actually downloads.
        </p>
      </div>

      <form
        onSubmit={handleSubmit}
        className="grid gap-6 rounded-lg border border-border p-4 md:grid-cols-2 md:p-6"
      >
        <div className="space-y-4">
          <h2 className="text-sm font-semibold">Subject</h2>
          <SubjectUserField value={user} onChange={setUser} />

          <div className="space-y-1.5">
            <Label htmlFor="explain-action">Action</Label>
            <Select value={action} onValueChange={(v) => setAction(v as Action)}>
              <SelectTrigger id="explain-action" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="read">read</SelectItem>
                <SelectItem value="write">write</SelectItem>
              </SelectContent>
            </Select>
            <p className="text-xs text-muted-foreground">
              Ranger has no <span className="font-mono">write</span>: it is evaluated as
              insert and delete on Trino, and insert, update and delete on StarRocks, each
              answered separately below.
            </p>
          </div>
        </div>

        <div className="space-y-4">
          <h2 className="text-sm font-semibold">Target</h2>
          <ScopePicker value={target} onChange={setTarget} />
          <Button type="submit" disabled={!canSubmit || explainMutation.isPending}>
            {explainMutation.isPending ? (
              <Loader2 className="h-4 w-4 animate-spin" />
            ) : (
              <Search className="h-4 w-4" />
            )}
            Explain
          </Button>
          {!canSubmit && (
            <p className="text-xs text-muted-foreground">Explain needs {missing}.</p>
          )}
        </div>
      </form>

      {explainError && (
        <Alert variant="destructive">
          <AlertDescription>
            {forbidden ? (
              <>
                You need the <code>policy:read</code> permission.
              </>
            ) : (
              explainError.message || "Failed to explain access."
            )}
          </AlertDescription>
        </Alert>
      )}

      {result && <ExplainResultView result={result} />}
    </div>
  );
}
