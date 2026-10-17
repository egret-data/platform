import { Fragment, useId, useState } from "react";
import { ChevronDown, ChevronRight, Users } from "lucide-react";
import type { ExplainPrincipalResponse, ExplainResponse, MemberVerdict } from "@/api/generated/model";
import { EmptyState } from "@/components/explore/EmptyState";
import { decisionLabel, ExplainResultView } from "@/components/perm/ExplainResult";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";

/** Above the counts: why there is no single verdict (P5). */
export const NO_SINGLE_VERDICT =
  "Each member's own answer, per policy profile. A group's or role's access is its members' access, and members can differ, so there is no single verdict.";

/** Every access in every profile allowed; at least one profile answered. */
function allowedEverywhere(r: ExplainResponse): boolean {
  if (r.outcome !== "per_profile" || r.profiles.length === 0) return false;
  return r.profiles.every((p) => p.accesses.length > 0 && p.accesses.every((a) => a.decision.kind === "allowed"));
}

function anyDecision(r: ExplainResponse, kind: "denied" | "undecidable"): boolean {
  return r.profiles.some((p) => p.accesses.some((a) => a.decision.kind === kind));
}

/** `Evaluated 200 of 201 members; the others were not asked.` */
export function truncatedText(evaluated: number, total: number): string {
  return `Evaluated ${evaluated} of ${total} members; the others were not asked.`;
}

/**
 * Explain for a group or a role (spec §4.3): each member's own answer, from
 * `POST /perm/explain/principal`. Three counts — allowed by every profile,
 * refused by at least one, undecidable on at least one — a row per evaluated
 * member with each profile's verdict per access, a "Refused only" filter, and
 * the member's full `ExplainResultView` on expand. There is no overall verdict
 * and nothing here makes one up: one member allowed and another refused is
 * two answers, not a third. A member with no username is listed under "Not
 * asked" with the reason, never dropped; when the server stopped at its limit
 * the view says how many were evaluated of how many.
 *
 * The filter and the expanded rows are local state: they shape an answer that
 * is itself not in the URL (it is the result of a POST), so a shared link
 * could not carry them.
 */
export function MembersExplainView({ response, kind }: { response: ExplainPrincipalResponse; kind: "group" | "role" }) {
  const [refusedOnly, setRefusedOnly] = useState(false);
  const [open, setOpen] = useState<ReadonlySet<string>>(new Set());
  const switchId = useId();
  const { evaluated, notEvaluated, membersTotal, truncated } = response;

  if (membersTotal === 0) {
    return (
      <EmptyState
        icon={Users}
        title={kind === "group" ? "This group has no members, so nobody was asked." : "No user holds this role, so nobody was asked."}
      />
    );
  }

  const allowed = evaluated.filter((m) => allowedEverywhere(m.result)).length;
  const refused = evaluated.filter((m) => anyDecision(m.result, "denied")).length;
  const undecidable = evaluated.filter((m) => anyDecision(m.result, "undecidable")).length;
  const shown = refusedOnly ? evaluated.filter((m) => anyDecision(m.result, "denied")) : evaluated;

  const toggle = (id: string) =>
    setOpen((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  return (
    <div data-testid="members-explain" className="space-y-4 rounded-lg border border-border p-4 md:p-6">
      <p className="text-sm text-muted-foreground">{NO_SINGLE_VERDICT}</p>
      <dl className="flex flex-wrap gap-x-6 gap-y-2 text-sm">
        {[
          ["Allowed by every profile", allowed],
          ["Refused by at least one", refused],
          ["Undecidable on at least one", undecidable],
        ].map(([label, n]) => (
          <div key={label} className="flex items-baseline gap-2">
            <dt className="text-muted-foreground">{label}</dt>
            <dd className="font-semibold tabular-nums">{n}</dd>
          </div>
        ))}
      </dl>
      {truncated ? <p className="text-sm">{truncatedText(evaluated.length, membersTotal)}</p> : null}

      {evaluated.length > 0 ? (
        <div className="space-y-2">
          <div className="flex items-center gap-2">
            <Switch id={switchId} checked={refusedOnly} onCheckedChange={setRefusedOnly} />
            <Label htmlFor={switchId}>Refused only</Label>
          </div>
          {shown.length === 0 ? (
            <EmptyState
              title="No evaluated member is refused."
              action={
                <Button variant="outline" size="sm" onClick={() => setRefusedOnly(false)}>
                  Show every member
                </Button>
              }
            />
          ) : (
            <Table aria-label="Members">
              <TableHeader>
                <TableRow>
                  <TableHead>Member</TableHead>
                  <TableHead>Verdicts</TableHead>
                  <TableHead className="w-12">
                    <span className="sr-only">Full answer</span>
                  </TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {shown.map((m) => (
                  <MemberRows key={m.userId} member={m} expanded={open.has(m.userId)} onToggle={() => toggle(m.userId)} />
                ))}
              </TableBody>
            </Table>
          )}
        </div>
      ) : null}

      {notEvaluated.length > 0 ? (
        <section className="space-y-1" aria-label="Not asked">
          <h3 className="text-sm font-semibold">Not asked</h3>
          <ul role="list" className="space-y-0.5 text-sm">
            {notEvaluated.map((m) => (
              <li key={m.userId}>
                {`${m.label}: `}
                <span className="text-muted-foreground">No username yet, so no engine knows this user.</span>
              </li>
            ))}
          </ul>
        </section>
      ) : null}
    </div>
  );
}

function MemberRows({ member, expanded, onToggle }: { member: MemberVerdict; expanded: boolean; onToggle: () => void }) {
  const r = member.result;
  return (
    <Fragment>
      <TableRow>
        <TableCell className="whitespace-normal">
          <p className="text-sm">{member.label}</p>
          <p className="font-mono text-xs text-muted-foreground break-all">{member.username}</p>
        </TableCell>
        <TableCell className="whitespace-normal">
          {r.outcome === "no_profile_serves_the_catalog" ? (
            <p className="text-sm">Nothing is enforcing: no policy profile serves this catalog.</p>
          ) : (
            <ul role="list" className="space-y-0.5 text-sm">
              {r.profiles.map((p) => (
                <li key={p.service_id}>
                  <span className="font-medium">{p.service_name}</span>
                  {": "}
                  {p.accesses.map((a) => `${a.access_type} ${decisionLabel(a.decision)}`).join(", ") || "no access asked"}
                </li>
              ))}
            </ul>
          )}
        </TableCell>
        <TableCell>
          <Button
            variant="ghost"
            size="icon"
            aria-expanded={expanded}
            aria-label={`${expanded ? "Hide" : "Show"} the full answer for ${member.label}`}
            onClick={onToggle}
          >
            {expanded ? <ChevronDown aria-hidden className="size-4" /> : <ChevronRight aria-hidden className="size-4" />}
          </Button>
        </TableCell>
      </TableRow>
      {expanded ? (
        <TableRow>
          <TableCell colSpan={3} className="whitespace-normal">
            <ExplainResultView result={r} />
          </TableCell>
        </TableRow>
      ) : null}
    </Fragment>
  );
}
