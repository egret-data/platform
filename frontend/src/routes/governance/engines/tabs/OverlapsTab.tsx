import { useId } from "react";
import { Link } from "react-router-dom";
import { Layers } from "lucide-react";
import type { StoredMember, StoredOverlap } from "@/api/generated/model";
import { usePermListOverlaps } from "@/api/generated/perm/perm";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { TabError } from "@/routes/governance/tabParts";
import { editorUrl, type EditorSubject } from "@/routes/governance/engines/engineUrl";
import type { TabContext } from "@/routes/governance/engines/PageFrame";

export const NO_OVERLAPS = "No overlapping rules on this profile.";

/**
 * A profile's stored mask and row-filter pairs that can decide one request two
 * ways (`GET /perm/overlaps`, SP4d-4; spec §6.3, EP6). The route reports every
 * profile; a pair is this profile's when its `serviceName` is the profile (the
 * route names the resource profile whose download serves it, and a tag service
 * only when no profile names it). The API may report one pair more than once,
 * so pairs are counted distinct.
 */
export function profileOverlaps(all: StoredOverlap[], profile: string): StoredOverlap[] {
  const seen = new Set<string>();
  const out: StoredOverlap[] = [];
  for (const o of all) {
    if (o.serviceName !== profile) continue;
    const key = [o.kind, o.coordinate, ...o.rules.map((r) => r.id ?? r.policyId ?? r.name)].join("|");
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(o);
  }
  return out;
}

/** The pairs of `profile`, or null while unknown; the page feeds the tab's count badge from it. */
export function useProfileOverlaps(profile: string | null) {
  const q = usePermListOverlaps({ query: { enabled: !!profile, retry: false } });
  const pairs = profile && q.data?.status === 200 ? profileOverlaps(q.data.data, profile) : null;
  return { q, pairs };
}

const editorKind = (kind: string) => (kind === "mask" ? "masking" : "rowfilter");

export function OverlapsTab({ ctx }: { ctx: TabContext }) {
  const profile = ctx.profile?.name ?? null;
  const { q, pairs } = useProfileOverlaps(profile);

  if (!profile) return <EmptyState icon={Layers} title="This engine downloads no policy profile, so no rule overlaps." />;
  if (q.isError) return <TabError what="overlaps" onRetry={() => void q.refetch()} />;
  if (!pairs) return <ListSkeleton rows={3} cols={2} />;
  if (pairs.length === 0) return <EmptyState icon={Layers} title={NO_OVERLAPS} />;

  return (
    <div className="space-y-3">
      <p className="text-sm text-muted-foreground">
        Each pair can decide one request two different ways. They were stored before overlaps were refused, so they
        block nothing, but the engine&apos;s choice between them is not something either one says. Break each pair to
        fix it.
      </p>
      <ul role="list" className="space-y-3">
        {pairs.map((o) => (
          <Pair key={[o.kind, o.coordinate, ...o.rules.map((r) => r.id ?? r.policyId ?? r.name)].join("|")} overlap={o} subject={ctx.subject} />
        ))}
      </ul>
    </div>
  );
}

function Pair({ overlap: o, subject }: { overlap: StoredOverlap; subject: EditorSubject }) {
  const headId = useId();
  const handWritten = o.rules.filter((m) => !m.id && m.policyId != null);
  return (
    <li aria-labelledby={headId} className="space-y-1 rounded-md border p-3 text-sm">
      <p id={headId}>
        <span className="font-medium">{o.kind === "mask" ? "Mask" : "Row filter"}</span>
        {" on "}
        <code className="break-all">{o.coordinate}</code>
      </p>
      <p>
        {o.rules.map((m, i) => (
          <span key={`${m.policyId ?? m.id ?? m.name}`}>
            {i > 0 ? " and " : null}
            <MemberLink member={m} kind={o.kind} subject={subject} />
          </span>
        ))}
        {o.servedToday ? <>{` — today the engine serves ${o.servedToday.name}.`}</> : null}
      </p>
      <p className="text-muted-foreground">{o.sentence}</p>
      <details>
        <summary className="cursor-pointer rounded-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
          How to fix
        </summary>
        <p className="mt-1">
          Delete one of the two, or narrow one so they no longer select a common resource. If either rule selects by
          classification tag, unassigning the tag from the column where they meet also breaks the pair. An edit that
          narrows a member but still meets the other is refused as a new overlap.
        </p>
        {handWritten.length > 0 ? (
          <ul role="list" className="mt-1 list-disc pl-5">
            {handWritten.map((m) => (
              <li key={m.policyId}>
                <Link
                  className="text-primary underline focus-visible:ring-2 focus-visible:ring-ring"
                  to={editorUrl(subject, editorKind(o.kind), m.policyId as number)}
                >
                  {`Switch off policy ${m.name}`}
                </Link>
                {" with the Enabled switch in its editor."}
              </li>
            ))}
          </ul>
        ) : null}
      </details>
    </li>
  );
}

/** A compiled member opens its rule; a hand-written one opens its policy in this page's Advanced editor. */
function MemberLink({ member, kind, subject }: { member: StoredMember; kind: string; subject: EditorSubject }) {
  const label = member.id ? `rule ${member.name}` : `policy ${member.name}`;
  const to = member.id
    ? `/governance/rules/${encodeURIComponent(member.id)}`
    : member.policyId != null
      ? editorUrl(subject, editorKind(kind), member.policyId)
      : null;
  return to ? (
    <Link className="font-medium text-primary underline focus-visible:ring-2 focus-visible:ring-ring" to={to}>
      {label}
    </Link>
  ) : (
    <span className="font-medium">{label}</span>
  );
}
