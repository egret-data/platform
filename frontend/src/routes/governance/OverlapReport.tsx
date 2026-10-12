import { Link } from "react-router-dom";
import { TriangleAlert } from "lucide-react";

import { usePermListOverlaps } from "@/api/generated/perm/perm";
import type { StoredMember, StoredOverlap } from "@/api/generated/model";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";

/**
 * Where an operator reads the mask and row-filter pairs that are ALREADY
 * stored and may decide one request two ways (SP4d-4 Task 4).
 *
 * A write refuses only the overlaps it creates, so a pair stored earlier
 * blocks nothing and would otherwise stay invisible: this is its report, shown
 * on Mask & filter rules (every profile) and on a profile's own page
 * (`serviceName`, so a hand-written pair on a manual profile is visible where
 * it is edited). Nothing is shown when there is none.
 */
export function OverlapReport({ serviceName }: { serviceName?: string }) {
  const q = usePermListOverlaps({ query: { retry: false } });

  if (q.isLoading) return null;
  if (q.isError) {
    return (
      <Alert variant="destructive">
        <AlertDescription>
          Could not check for overlapping rules. Reload the page to try again.
        </AlertDescription>
      </Alert>
    );
  }
  const all: StoredOverlap[] = q.data?.status === 200 ? q.data.data : [];
  const pairs = serviceName ? all.filter((o) => o.serviceName === serviceName) : all;
  if (pairs.length === 0) return null;

  return (
    <Alert aria-labelledby="overlap-report-title">
      <TriangleAlert className="h-4 w-4" />
      <div>
        <AlertTitle id="overlap-report-title">
          {pairs.length === 1
            ? "1 pair of rules overlaps"
            : `${pairs.length} pairs of rules overlap`}
        </AlertTitle>
        <AlertDescription className="space-y-3">
          <p>
            Each pair can decide one request two different ways. They were stored before overlaps
            were refused, so they block nothing, but the engine&apos;s choice between them is not
            something either one says. Break each pair to fix it.
          </p>
          <ul className="space-y-3">
            {pairs.map((o) => (
              <li
                key={`${o.serviceId}|${o.rules.map((r) => r.policyId ?? r.id ?? r.name).join("|")}`}
                className="space-y-1"
              >
                <div>
                  <span className="font-medium">
                    {o.kind === "mask" ? "Mask" : "Row filter"}
                  </span>{" "}
                  on <code>{o.coordinate}</code>
                  {!serviceName && (
                    <>
                      , profile{" "}
                      <Link
                        className="text-primary hover:underline"
                        to={`/governance/policies/${encodeURIComponent(o.serviceName)}`}
                      >
                        {o.serviceName}
                      </Link>
                    </>
                  )}
                </div>
                <div>
                  {o.rules.map((m, i) => (
                    <span key={`${m.policyId ?? m.id ?? m.name}`}>
                      {i > 0 && " and "}
                      <MemberLink member={m} overlap={o} />
                    </span>
                  ))}
                  {o.servedToday && <> — today the engine serves {o.servedToday.name}.</>}
                </div>
                <details>
                  <summary className="cursor-pointer">How to fix</summary>
                  <p className="mt-1">{o.howToFix}</p>
                  <p className="mt-1 text-muted-foreground">{o.sentence}</p>
                </details>
              </li>
            ))}
          </ul>
        </AlertDescription>
      </div>
    </Alert>
  );
}

/** A compiled member opens its rule; a hand-written one opens its policy in the editor. */
function MemberLink({ member, overlap }: { member: StoredMember; overlap: StoredOverlap }) {
  const label = member.id ? `rule ${member.name}` : `policy ${member.name}`;
  const to = member.id
    ? `/governance/mask-filter-rules?rule=${encodeURIComponent(member.id)}`
    : member.policyId != null
      ? `/governance/policies/${encodeURIComponent(overlap.serviceName)}/${
          overlap.kind === "mask" ? "masking" : "rowfilter"
        }/${member.policyId}`
      : null;
  return to ? (
    <Link className="font-medium text-primary hover:underline" to={to}>
      {label}
    </Link>
  ) : (
    <span className="font-medium">{label}</span>
  );
}
