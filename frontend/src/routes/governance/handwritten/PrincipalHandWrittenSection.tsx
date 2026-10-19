import { useState } from "react";
import { Link } from "react-router-dom";
import { Ban, FileCode2 } from "lucide-react";
import type { ApiError } from "@/api/client";
import type { Bucket, Naming, PrincipalPolicy } from "@/api/generated/model";
import { useControlPlaneGovernancePrincipalHandwritten } from "@/api/generated/control-plane-governance/control-plane-governance";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { usePolicyPermissions } from "@/routes/governance/policyWrites";
import { Section, STACKED_TABLE, TabError } from "@/routes/governance/tabParts";
import { editorUrl, type EditorSubject } from "@/routes/governance/engines/engineUrl";
import {
  BUCKET_WORD,
  DENY_ALL_ELSE,
  NEEDS_POLICY_READ,
  namesHeldSentence,
  namesNobodySentence,
  policyTypeWord,
} from "@/routes/governance/handwritten/handwrittenText";

/** A People page's principal as route B names it: a user by id, a group or role by name. */
export type PrincipalSubject = { kind: "user" | "group" | "role"; id: string };

const RULE_BUCKETS: Bucket[] = ["mask", "row_filter"];
const isRuleType = (t: string) => t === "masking" || t === "rowfilter";

export const NO_USERNAME_SENTENCE =
  "This user has no username, so no hand-written policy item can name them on an engine.";

const byName = <T extends { name: string }>(a: T, b: T) => a.name.localeCompare(b.name);

/** Where a row is edited: the first engine downloading it; for a resource policy with none, its profile's page. */
function editHref(p: PrincipalPolicy): string | null {
  const engine = [...p.engines].sort(byName)[0];
  const subject: EditorSubject | null = engine
    ? { engineId: engine.id }
    : p.inTagService
      ? null
      : { profile: p.service };
  return subject ? `${editorUrl(subject, p.policyType, p.id)}${p.inTagService ? "?scope=tag" : ""}` : null;
}

/**
 * Whether a naming is Direct on this page: an item naming the username on a
 * user's page, and, on a group's or role's page, an item naming that group or
 * role itself (route B reports it as `how: "group"`/`"role"`; live check D3).
 * Only a user can be reached "through" a group or role.
 */
const isDirect = (n: Naming, subject: PrincipalSubject) => n.how === "direct" || subject.kind !== "user";

/** Q1: why a group or role naming reaches nobody, worded for the page it is on. */
function viaSentence(n: Naming, subject: PrincipalSubject): string | null {
  if (n.how === "direct" || !n.via || n.onEngines) return null;
  const kind = n.how;
  return subject.kind === "user" ? namesHeldSentence(kind, n.via) : namesNobodySentence(kind, n.via);
}

/**
 * The hand-written policies that name a People page's principal (spec §6,
 * route B, Q1), in three groups: Direct (an item names the username, or this
 * group or role), Through a group or role (an item names a group or role this
 * user holds, which no engine resolves: it reaches nobody there), and Applies
 * to everyone (`public`, `{USER}`), counted and expandable. A user without a
 * username can be named by no item. Read with `policy:read`; without it the
 * section says so and asks nothing.
 */
export function PrincipalHandWrittenSection({ subject, show }: { subject: PrincipalSubject; show: "access" | "rules" }) {
  const canRead = usePolicyPermissions()("policy:read");
  if (!canRead) {
    return (
      <Section title="Hand-written policies">
        <p className="text-sm text-muted-foreground">{NEEDS_POLICY_READ}</p>
      </Section>
    );
  }
  return <Body subject={subject} show={show} />;
}

function Body({ subject, show }: { subject: PrincipalSubject; show: "access" | "rules" }) {
  const q = useControlPlaneGovernancePrincipalHandwritten(subject.kind, subject.id, { query: { retry: false } });
  if (q.isError) {
    const message = (q.error as unknown as ApiError | null)?.message;
    return (
      <Section title="Hand-written policies">
        <TabError what="hand-written policies" onRetry={() => void q.refetch()} detail={message} />
      </Section>
    );
  }
  if (q.data?.status !== 200) {
    return (
      <Section title="Hand-written policies">
        <ListSkeleton rows={2} cols={3} />
      </Section>
    );
  }
  const d = q.data.data;
  const wanted = (t: string) => (show === "rules" ? isRuleType(t) : !isRuleType(t));
  const policies = d.policies.filter((p) => wanted(p.policyType)).sort((a, b) => a.service.localeCompare(b.service) || byName(a, b));
  const direct = policies.filter((p) => p.naming.some((n) => isDirect(n, subject)));
  const through = policies.filter((p) => p.naming.some((n) => !isDirect(n, subject)));
  const everyoneEntries = d.everyone.policies.filter((e) => RULE_BUCKETS.includes(e.bucket) === (show === "rules"));
  const everyoneIds = [...new Map(everyoneEntries.map((e) => [`${e.service}/${e.id}`, e])).values()].sort(byName);

  return (
    <Section title={d.state === "no_username" ? "Hand-written policies" : `Hand-written policies (${policies.length})`}>
      {d.state === "no_username" ? (
        <p className="text-sm text-muted-foreground">{NO_USERNAME_SENTENCE}</p>
      ) : policies.length === 0 ? (
        <EmptyState icon={FileCode2} title="No hand-written policy names this principal." />
      ) : (
        <>
          {direct.length > 0 ? (
            <Group title="Direct" rows={direct} subject={subject} only="direct" />
          ) : null}
          {through.length > 0 ? (
            <Group title="Through a group or role" rows={through} subject={subject} only="via" />
          ) : null}
        </>
      )}
      {everyoneIds.length > 0 ? <Everyone entries={everyoneIds} /> : null}
    </Section>
  );
}

function Group({
  title,
  rows,
  subject,
  only,
}: {
  title: string;
  rows: PrincipalPolicy[];
  subject: PrincipalSubject;
  only: "direct" | "via";
}) {
  return (
    <Section title={title}>
      <div className="overflow-x-auto rounded-md border">
        <Table aria-label={title} className={STACKED_TABLE}>
          <TableHeader>
            <TableRow>
              <TableHead>Policy</TableHead>
              <TableHead>Names it</TableHead>
              <TableHead>Engines</TableHead>
              <TableHead>State</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {rows.map((p) => {
              const href = editHref(p);
              const namings = p.naming.filter((n) => (only === "direct") === isDirect(n, subject));
              return (
                <TableRow key={`${p.service}/${p.id}`}>
                  <TableCell className="whitespace-normal align-top">
                    <span className="flex flex-wrap items-center gap-1.5">
                      {href ? (
                        <Link to={href} className="break-all font-medium text-primary hover:underline">
                          {p.name}
                        </Link>
                      ) : (
                        <span className="break-all font-medium">{p.name}</span>
                      )}
                      <Badge variant="outline">Hand-written</Badge>
                    </span>
                    <span className="block text-xs text-muted-foreground">
                      {policyTypeWord(p.policyType)} · {p.service}
                    </span>
                    <span className="block break-all font-mono text-xs text-muted-foreground">{p.resourceSummary}</span>
                  </TableCell>
                  <TableCell data-label="Names it" className="whitespace-normal align-top text-sm">
                    {namings.map((n, i) => {
                      const sentence = viaSentence(n, subject);
                      return (
                        <span key={i} className="block">
                          <span className="inline-flex items-center gap-1">
                            {n.bucket === "deny" ? <Ban className="h-3.5 w-3.5 text-destructive" aria-hidden /> : null}
                            {BUCKET_WORD[n.bucket]}
                          </span>
                          {sentence ? <span className="block text-xs text-muted-foreground">{sentence}</span> : null}
                        </span>
                      );
                    })}
                  </TableCell>
                  <TableCell data-label="Engines" className="whitespace-normal align-top text-sm">
                    {p.engines.length > 0 ? [...p.engines].sort(byName).map((e) => e.name).join(", ") : "No engine"}
                  </TableCell>
                  <TableCell data-label="State" className="whitespace-normal align-top text-sm">
                    <Badge variant={p.enabled ? "secondary" : "outline"}>{p.enabled ? "Enabled" : "Disabled"}</Badge>
                    {p.denyAllElse ? <span className="mt-1 block text-xs">{DENY_ALL_ELSE}</span> : null}
                    {p.unmodelled.map((u) => (
                      <span key={u} className="mt-1 block text-xs">
                        {u}
                      </span>
                    ))}
                  </TableCell>
                </TableRow>
              );
            })}
          </TableBody>
        </Table>
      </div>
    </Section>
  );
}

function Everyone({ entries }: { entries: { id: number; name: string; service: string }[] }) {
  const [open, setOpen] = useState(false);
  const n = entries.length;
  return (
    <Section title="Applies to everyone">
      <p className="text-sm text-muted-foreground">
        {`${n} hand-written ${n === 1 ? "policy names" : "policies name"} everyone.`}{" "}
        <Button variant="link" size="sm" className="h-auto p-0" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
          {open ? "Hide them" : "Show them"}
        </Button>
      </p>
      {open ? (
        <ul className="list-disc space-y-0.5 pl-5 text-sm">
          {entries.map((e) => (
            <li key={`${e.service}/${e.id}`}>
              <span className="font-medium">{e.name}</span>
              <span className="text-muted-foreground">{` · ${e.service}`}</span>
            </li>
          ))}
        </ul>
      ) : null}
    </Section>
  );
}
