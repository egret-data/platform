import { Link } from "react-router-dom";
import { CircleCheck, TriangleAlert } from "lucide-react";
import type { Place, ReuseFinding, ReuseKind, Verdict } from "@/api/generated/model";
import { usePermUsernameReuse } from "@/api/generated/perm/perm";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Button } from "@/components/ui/button";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { fullTimestamp } from "@/lib/relativeTime";
import { engineUrl, profileUrl } from "@/routes/governance/engines/engineUrl";
import { principalUrl } from "@/routes/governance/people/peopleUrl";
import { STACKED_TABLE, TabError } from "@/routes/governance/tabParts";

const VERDICT: Record<Verdict, string> = { definite: "Definite", possible: "Possible", stale: "Stale", dead: "Dead" };
const VERDICT_ORDER: Record<Verdict, number> = { definite: 0, possible: 1, stale: 2, dead: 3 };

/** What each kind of finding means, in words. */
const KIND: Record<ReuseKind, string> = {
  ref_predates_holder: "Written before its holder joined",
  policy_predates_holder: "Policy older than its holder",
  admin_policy_named_non_admin: "Admin policy names someone who is not an active admin",
  holderless_ref: "Names nobody, and nobody can hold it again",
};

/** Where the name is written: the page it is fixed on, or text when there is none. */
function PlaceLink({ p }: { p: Place }) {
  const [label, to] =
    p.type === "grant"
      ? ["Grant", p.id ? `/governance/grants/${encodeURIComponent(p.id)}` : null]
      : p.type === "rule"
        ? ["Rule", p.id ? `/governance/rules/${encodeURIComponent(p.id)}` : null]
        : p.type === "engine_grant"
          ? ["Engine grant", p.engineId && p.id ? engineUrl(p.engineId, "engine-permissions", { grant: p.id }) : null]
          : [`Policy ${p.name ?? p.policyId ?? ""} on ${p.service ?? "an unknown profile"}`, p.service ? profileUrl(p.service, "policies") : null];
  return to ? (
    <Link to={to} className="break-all text-primary hover:underline">
      {label}
    </Link>
  ) : (
    <span className="break-all">{label}</span>
  );
}

function VerdictCell({ f }: { f: ReuseFinding }) {
  return (
    <span className="flex flex-col gap-0.5">
      <span className="font-medium">{VERDICT[f.verdict]}</span>
      {f.retired?.failsOpen ? (
        <span className="inline-flex items-center gap-1 font-medium text-destructive">
          <TriangleAlert className="size-4" aria-hidden />
          Widens access
        </span>
      ) : null}
    </span>
  );
}

function HolderCell({ f }: { f: ReuseFinding }) {
  if (!f.holder) return <span className="text-muted-foreground">Nobody</span>;
  return (
    <span className="flex flex-col gap-0.5">
      <Link to={principalUrl({ kind: "user", userId: f.holder.id })} className="break-all text-primary hover:underline">
        {f.holder.email}
      </Link>
      <span className="text-xs text-muted-foreground">
        {`Since ${fullTimestamp(f.holder.createdAt).slice(0, 10)}`}
      </span>
    </span>
  );
}

/**
 * The index's Reused names tab (username-takeover spec 2026-10-23 §4.5),
 * platform admins only: every engine username written for someone other than
 * the person who holds it now (`GET /perm/username-reuse`), definite findings
 * first, each with where it is written, its holder and the evidence. `q`
 * filters on the username, here in the page: the route answers the whole list.
 * Nothing is changed automatically; each place is fixed on its own page.
 */
export function ReusedNames({ q, onClearSearch }: { q: string; onClearSearch: () => void }) {
  const report = usePermUsernameReuse({ query: { retry: false } });

  if (report.isError) return <TabError what="reused names" onRetry={() => void report.refetch()} />;
  if (report.data?.status !== 200) return <ListSkeleton rows={4} cols={5} />;
  const { findings, baselineTakenAt } = report.data.data;
  if (findings.length === 0) {
    return (
      <EmptyState
        icon={CircleCheck}
        title="No reused engine usernames found."
        description="Nothing names a username from before the person who holds it now existed, and every egret-admin policy names only active platform admins. History past audit retention cannot be checked."
      />
    );
  }
  const needle = q.toLowerCase();
  // `sort` is stable: within each verdict the server's order stays.
  const rows = findings
    .filter((f) => f.username.toLowerCase().includes(needle))
    .sort((a, b) => VERDICT_ORDER[a.verdict] - VERDICT_ORDER[b.verdict]);
  if (rows.length === 0) {
    return (
      <EmptyState
        title={`No reused name matches “${q}”.`}
        action={
          <Button variant="outline" size="sm" onClick={onClearSearch}>
            Clear search
          </Button>
        }
      />
    );
  }

  return (
    <div className="space-y-3">
      <p className="text-sm text-muted-foreground">
        Engine usernames written into grants, rules and policies for someone other than the person who holds the name now,
        for example before a user was deleted and the name went to someone else. Definite findings are proven by an event
        or a timestamp; possible ones cannot be told apart from a later edit. Stale ones are egret-admin policies that
        still name someone who is no longer an active platform admin, not a reuse. Dead names match nobody, and nobody can
        be given them again.
      </p>
      <Table className={STACKED_TABLE}>
        <TableHeader>
          <TableRow>
            <TableHead>Username</TableHead>
            <TableHead>Verdict</TableHead>
            <TableHead>Finding</TableHead>
            <TableHead>In</TableHead>
            <TableHead>Holder</TableHead>
            <TableHead>Evidence</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {rows.map((f, i) => (
            <TableRow key={`${f.kind}|${f.place.type}|${f.place.id ?? f.place.policyId ?? ""}|${f.username}|${i}`}>
              <TableCell>
                <span className="break-all font-mono text-xs">{f.username}</span>
              </TableCell>
              <TableCell data-label="Verdict" className="whitespace-nowrap">
                <VerdictCell f={f} />
              </TableCell>
              <TableCell data-label="Finding" className="min-w-40 whitespace-normal max-sm:min-w-0">
                {KIND[f.kind] ?? f.kind}
              </TableCell>
              <TableCell data-label="In" className="whitespace-normal">
                <PlaceLink p={f.place} />
              </TableCell>
              <TableCell data-label="Holder" className="whitespace-normal">
                <HolderCell f={f} />
              </TableCell>
              <TableCell data-label="Evidence" className="min-w-48 whitespace-normal break-words max-sm:min-w-0">
                <span className="flex flex-col gap-0.5">
                  {f.evidenceAt ? (
                    <time dateTime={f.evidenceAt} className="tabular-nums">
                      {fullTimestamp(f.evidenceAt)}
                    </time>
                  ) : null}
                  <span className="text-muted-foreground">{f.evidence}</span>
                </span>
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
      {baselineTakenAt ? (
        <p className="text-xs text-muted-foreground">
          {`Policy bodies as they were on ${fullTimestamp(baselineTakenAt).slice(0, 10)}, when this check was installed, are included.`}
        </p>
      ) : null}
    </div>
  );
}
