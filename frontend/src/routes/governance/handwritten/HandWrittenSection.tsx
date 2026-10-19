import { Fragment } from "react";
import { Link } from "react-router-dom";
import { FileCode2 } from "lucide-react";
import type { ApiError } from "@/api/client";
import type { HandWrittenPolicy, HandWrittenProfile, MatchWord } from "@/api/generated/model";
import { useControlPlaneGovernanceObjectHandwritten } from "@/api/generated/control-plane-governance/control-plane-governance";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Badge } from "@/components/ui/badge";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { usePolicyPermissions } from "@/routes/governance/policyWrites";
import { Section, STACKED_TABLE, TabError } from "@/routes/governance/tabParts";
import { editorUrl, engineUrl, profileUrl, type EditorSubject } from "@/routes/governance/engines/engineUrl";
import { HandWrittenItems } from "@/routes/governance/handwritten/HandWrittenItems";
import {
  DENY_ALL_ELSE,
  MATCH_WORD,
  NEEDS_POLICY_READ,
  NOT_DETERMINED,
  policyTypeWord,
} from "@/routes/governance/handwritten/handwrittenText";

/** Which rows a placement shows (spec §6): access rows, masking and row-filter rows, or every row. */
export type HandWrittenShow = "access" | "rules" | "all";

const SHOWN: Record<HandWrittenShow, (p: HandWrittenPolicy) => boolean> = {
  access: (p) => p.policyType === "access",
  rules: (p) => p.policyType === "masking" || p.policyType === "rowfilter",
  all: () => true,
};

const MATCH_ORDER: MatchWord[] = ["applies", "part", "parent", "unknown"];

const byName = <T extends { name: string }>(a: T, b: T) => a.name.localeCompare(b.name);

/** The profile a row is stored in: by name, or the profile whose tag service stores it. */
function profileOf(p: HandWrittenPolicy, profiles: HandWrittenProfile[]): HandWrittenProfile | undefined {
  return profiles.find((pr) => (p.inTagService ? pr.tagService === p.service : pr.name === p.service));
}

/** The engine page (first bound engine) or the unbound profile's page that edits a profile's policies. */
function subjectOf(profile: HandWrittenProfile | undefined): EditorSubject | null {
  if (!profile) return null;
  const engine = [...profile.engines].sort(byName)[0];
  return engine ? { engineId: engine.id } : { profile: profile.name };
}

function viaTagText(v: NonNullable<HandWrittenPolicy["viaTag"]>): string {
  if (v.column) return `Through tag ${v.name} on column ${v.column}`;
  return v.on === "this" ? `Through tag ${v.name} on this object` : `Through tag ${v.name} on a parent`;
}

/**
 * The hand-written Ranger policies that reach a Data object (spec §6, F12),
 * from route A, so none is hidden: each row badged "Hand-written", with its
 * match level (Q2: a resource match, never "grants"), its items (group and
 * role items reach nobody on an engine, Q1), whether it is enabled, the
 * constructs egret does not evaluate (CL9), and a link to the engine's
 * Advanced editor. egret-seeded policies are counted, not listed. Read with
 * `policy:read`; without it the section says so and asks nothing. A failed
 * read is an error with Retry, never "none".
 */
export function HandWrittenSection({
  catalogId,
  schema,
  table,
  column,
  show,
}: {
  catalogId: string;
  schema?: string;
  table?: string;
  column?: string;
  show: HandWrittenShow;
}) {
  const canRead = usePolicyPermissions()("policy:read");
  if (!canRead) {
    return (
      <Section title="Hand-written policies">
        <p className="text-sm text-muted-foreground">{NEEDS_POLICY_READ}</p>
      </Section>
    );
  }
  return <Body catalogId={catalogId} schema={schema} table={table} column={column} show={show} />;
}

function Body({
  catalogId,
  schema,
  table,
  column,
  show,
}: {
  catalogId: string;
  schema?: string;
  table?: string;
  column?: string;
  show: HandWrittenShow;
}) {
  const params = {
    ...(schema !== undefined ? { schema } : {}),
    ...(table !== undefined ? { table } : {}),
    ...(column !== undefined ? { column } : {}),
  };
  const q = useControlPlaneGovernanceObjectHandwritten(catalogId, params, { query: { retry: false } });

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
        <ListSkeleton rows={2} cols={4} />
      </Section>
    );
  }
  const d = q.data.data;
  if (d.state === "not_determined") {
    return (
      <Section title="Hand-written policies">
        <p className="text-sm text-muted-foreground">
          {d.reason ? NOT_DETERMINED[d.reason] : "Which hand-written policies reach this object could not be determined."}
        </p>
      </Section>
    );
  }

  const rows = d.policies
    .filter(SHOWN[show])
    .sort(
      (a, b) =>
        MATCH_ORDER.indexOf(a.match) - MATCH_ORDER.indexOf(b.match) ||
        a.service.localeCompare(b.service) ||
        byName(a, b),
    );
  const seeded = show === "rules" ? [] : d.seeded.filter((s) => s.count > 0);
  const seededTotal = seeded.reduce((n, s) => n + s.count, 0);

  return (
    <Section title={`Hand-written policies (${rows.length})`}>
      {rows.length === 0 ? (
        <EmptyState icon={FileCode2} title="No hand-written policy reaches this object." />
      ) : (
        <div className="overflow-x-auto rounded-md border">
          <Table aria-label="Hand-written policies" className={STACKED_TABLE}>
            <TableHeader>
              <TableRow>
                <TableHead>Policy</TableHead>
                <TableHead>Reach</TableHead>
                <TableHead>Items</TableHead>
                <TableHead>State</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.map((p) => (
                <PolicyRow key={`${p.service}/${p.id}`} p={p} profiles={d.profiles} />
              ))}
            </TableBody>
          </Table>
        </div>
      )}
      {seededTotal > 0 ? (
        <p className="text-sm text-muted-foreground">
          {`${seededTotal} egret-seeded ${seededTotal === 1 ? "policy also reaches" : "policies also reach"} this object; see Engine & policy: `}
          {seeded.map((s, i) => {
            const profile = d.profiles.find((pr) => pr.name === s.service);
            const engines = profile ? [...profile.engines].sort(byName) : [];
            const links =
              engines.length > 0
                ? engines.map((e) => ({ to: engineUrl(e.id, "policies"), label: e.name }))
                : [{ to: profileUrl(s.service, "policies"), label: s.service }];
            return (
              <Fragment key={s.service}>
                {i > 0 ? ", " : null}
                {links.map((l, j) => (
                  <Fragment key={l.to}>
                    {j > 0 ? ", " : null}
                    <Link to={l.to} className="text-primary underline">
                      {l.label}
                    </Link>
                  </Fragment>
                ))}
              </Fragment>
            );
          })}
          .
        </p>
      ) : null}
    </Section>
  );
}

function PolicyRow({ p, profiles }: { p: HandWrittenPolicy; profiles: HandWrittenProfile[] }) {
  const subject = subjectOf(profileOf(p, profiles));
  const href = subject ? `${editorUrl(subject, p.policyType, p.id)}${p.inTagService ? "?scope=tag" : ""}` : null;
  return (
    <TableRow>
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
      <TableCell data-label="Reach" className="whitespace-normal align-top text-sm">
        <span className="block">
          {p.match === "unknown"
            ? `${MATCH_WORD.unknown}${p.parseError ? `: ${p.parseError}` : ""}`
            : `${MATCH_WORD[p.match]}${p.throughPattern ? " · through a pattern" : ""}`}
        </span>
        {p.match === "parent" ? (
          <span className="block text-xs text-muted-foreground">
            {`Names ${p.resourceSummary} only; engines do not apply it to this object's data.`}
          </span>
        ) : null}
        {p.viaTag ? <span className="block text-xs text-muted-foreground">{viaTagText(p.viaTag)}</span> : null}
      </TableCell>
      <TableCell data-label="Items" className="whitespace-normal align-top">
        <HandWrittenItems items={p.items} />
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
}
