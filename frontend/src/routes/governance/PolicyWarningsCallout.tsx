import { Link } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { TriangleAlert } from "lucide-react";

import { usePermServicePolicyWarnings } from "@/api/generated/perm/perm";
import type { PolicyWarning, WarnedPolicy } from "@/api/generated/model";
import { ApiError } from "@/api/client";
import { listPolicies, type PolicySummary } from "@/api/policies";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";

/**
 * The stored policies of one profile whose resource levels Ranger reads
 * differently from how the editor shows them (SP4d-4 B1): a policy that stops
 * above `column` and allows `select`, or one that names a level under an
 * absent one. A stored row is flagged here and never rewritten; nothing is
 * shown when the profile has none.
 */
export function PolicyWarningsCallout({
  service,
  serviceId,
}: {
  service: string;
  /** The profile's id; the route is keyed by it. Undefined while the profile loads. */
  serviceId: number | undefined;
}) {
  const q = usePermServicePolicyWarnings(serviceId ?? 0, {
    query: {
      enabled: serviceId !== undefined,
      retry: false,
      // Under the `policies` prefix so every policy write that invalidates the
      // lists refreshes this too.
      queryKey: ["control-plane", "policies", "warnings", serviceId],
    },
  });
  // The editor link needs each policy's type; the warnings carry only id and name.
  const all = useQuery<PolicySummary[], ApiError>({
    queryKey: ["control-plane", "policies", service, "all"],
    queryFn: () => listPolicies(service),
    enabled: !!service && (q.data?.status === 200 ? q.data.data.warningCount > 0 : false),
    retry: false,
  });

  if (serviceId === undefined || q.isLoading) return null;
  if (q.isError) {
    // The page itself says what a 403 needs; only a real failure is news here.
    if (q.error instanceof ApiError && q.error.status === 403) return null;
    return (
      <Alert variant="destructive">
        <AlertDescription>
          Could not check this profile&apos;s stored policies for level warnings. Reload the page to
          try again.
        </AlertDescription>
      </Alert>
    );
  }
  const body = q.data?.status === 200 ? q.data.data : null;
  if (!body || body.warningCount === 0) return null;

  const typeOf = (id: number) => all.data?.find((p) => p.id === id)?.policy_type ?? "access";
  const kinds = new Set(body.policies.flatMap((p) => p.warnings.map((w) => w.kind)));
  const n = body.warningCount;
  const m = body.policies.length;

  return (
    <Alert aria-labelledby="policy-warnings-title">
      <TriangleAlert className="h-4 w-4" />
      <div>
        <AlertTitle id="policy-warnings-title">
          {n === 1 ? "1 warning" : `${n} warnings`} on {m === 1 ? "1 stored policy" : `${m} stored policies`}
        </AlertTitle>
        <AlertDescription className="space-y-3">
          <p>
            Ranger reads these stored policies differently from how the editor shows them. Nothing
            was changed: the rows are flagged, not rewritten.
          </p>
          {kinds.has("select_stops_above_column") && (
            <p>
              A policy that stops at catalog, schema or table does not grant column SELECT on Trino
              470 or StarRocks 4.1.4 (measured against the real Ranger plugins): it allows only
              table-level checks such as <code>count(*)</code>. To grant column reads, open the
              policy and set the column level (use * for every column). Saving it then stores the
              broader policy.
            </p>
          )}
          {kinds.has("level_gap") && (
            <p>
              A level is named below one that is not set, so Ranger cannot match the policy as
              written. Open it and set the missing level (use * for every value), or clear the
              level below it.
            </p>
          )}
          <ul className="space-y-2">
            {body.policies.map((p: WarnedPolicy) => (
              <li key={p.id}>
                <Link
                  className="font-medium text-primary hover:underline"
                  to={`/governance/policies/${encodeURIComponent(service)}/${typeOf(p.id)}/${p.id}`}
                >
                  {p.name}
                </Link>
                <ul>
                  {p.warnings.map((w: PolicyWarning) => (
                    <li key={`${w.kind}-${w.level ?? w.missing ?? ""}`}>{w.sentence}</li>
                  ))}
                </ul>
              </li>
            ))}
          </ul>
        </AlertDescription>
      </div>
    </Alert>
  );
}
