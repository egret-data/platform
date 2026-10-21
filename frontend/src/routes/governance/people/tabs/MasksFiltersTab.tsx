import { Link } from "react-router-dom";
import { EyeOff } from "lucide-react";
import type { RuleView } from "@/api/generated/model";
import { usePermListRules } from "@/api/generated/perm/perm";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { OpenInDataLink, scopeToDataLink, useCatalogNameById } from "@/routes/governance/openInData";
import { formatRuleKind, formatSelector } from "@/routes/governance/permFormat";
import { HandWrittenPointer, Section, TabError } from "@/routes/governance/tabParts";
import { PrincipalHandWrittenSection } from "@/routes/governance/handwritten/PrincipalHandWrittenSection";
import { principalSubject, principalTitle, type Principal } from "@/routes/governance/people/principalData";
import { REACH_GROUPS, reachesAtAll, reachGroup, setReach, type Reach, type ReachGroup } from "@/routes/governance/people/reach";
import { GROUP_TITLE, NO_USERNAME_NOTE, reachText } from "@/routes/governance/people/tabs/PermissionsTab";
import { TERM } from "@/routes/governance/glossary";

/** The Permissions tab's group titles; an exclude here takes the principal out of an exemption. */
const RULE_GROUP_TITLE: Record<ReachGroup, string> = { ...GROUP_TITLE, excluded: "Excluded from the exemption" };

/**
 * Under the "Not applied anywhere" group (an exemption held through a group, which the
 * compiler does not expand). Rules are compiled to engine masking and
 * row-filter policies only; nothing in egret reads them to mask or filter, so
 * such an exemption exempts nobody anywhere. Not said of an expired path,
 * which engines may still apply until the next recompile.
 */
export const EXEMPTION_NOT_ON_ENGINES =
  "egret applies no mask or row filter itself, so an exemption engines do not apply exempts nobody.";

/** `N other rules apply to everyone they do not exempt; …`, in the singular for one. */
export function otherRulesText(n: number): string {
  return n === 1
    ? "1 other rule applies to everyone it does not exempt; each object's Rules tab lists it."
    : `${n} other rules apply to everyone they do not exempt; each object's Rules tab lists them.`;
}

type Row = { rule: RuleView; reach: Reach[] };

/**
 * A principal's Rules tab (spec §2; "Masks & filters" before the glossary): every egret mask and row-filter
 * rule whose exemption stands for this principal (`setReach` on `exempt`,
 * against the server's `standsFor`), grouped as the Permissions tab groups
 * grants (`reachGroup`), each row saying every path and whether engines apply
 * it. The rules exempting nobody here are counted, not listed: they apply to
 * the principal like to everyone else, and each object's Rules tab
 * lists them. The rules come from `GET /perm/rules` (`policy:read`, which the
 * People area's route already requires). Read-only: a rule is edited on its
 * own page, `/governance/rules/:id`.
 */
export function MasksFiltersTab({ principal }: { principal: Principal }) {
  // #4b (spec §6): the hand-written masking and row-filter policies naming the
  // principal follow egret's rules. `public` is named by no item as itself.
  const isPublic = principal.kind === "group" && principal.data.isPublic;
  return (
    <div className="space-y-6">
      <EgretRules principal={principal} />
      {isPublic ? null : <PrincipalHandWrittenSection subject={principalSubject(principal)} show="rules" />}
    </div>
  );
}

function EgretRules({ principal }: { principal: Principal }) {
  const rulesQ = usePermListRules({ query: { retry: false } });
  const catalogNameById = useCatalogNameById();
  const name = principalTitle(principal, principal.data.name);
  const isPublic = principal.kind === "group" && principal.data.isPublic;
  const noUsername = principal.kind === "user" && !principal.data.username;

  if (rulesQ.isError) return <TabError what="rules" onRetry={() => void rulesQ.refetch()} />;
  if (rulesQ.data?.status !== 200) return <ListSkeleton rows={4} cols={3} />;

  const all = rulesQ.data.data;
  const exempting: Row[] = [];
  for (const rule of all) {
    const reach = setReach(rule.exempt, principal.data.standsFor);
    if (reachesAtAll(reach)) exempting.push({ rule, reach });
  }
  const others = all.length - exempting.length;

  // `public` has no Check access tab (spec §2), so its note names none.
  const header = noUsername ? <p className="text-sm">{NO_USERNAME_NOTE}</p> : null;
  const pointer = (
    <HandWrittenPointer
      subject={TERM.rules}
      lead="To see the engines' answer, use"
      explainLabel={isPublic ? null : "Check access"}
      explainTo={isPublic ? undefined : "?tab=check"}
    />
  );
  const rest = others > 0 ? <p className="text-sm text-muted-foreground">{otherRulesText(others)}</p> : null;

  if (exempting.length === 0) {
    return (
      <div className="space-y-2">
        {header}
        <EmptyState icon={EyeOff} title={`No rule exempts ${name}`} />
        {rest}
        {pointer}
      </div>
    );
  }

  const byGroup = new Map<ReachGroup, Row[]>();
  for (const r of exempting) {
    const g = reachGroup(r.reach);
    byGroup.set(g, [...(byGroup.get(g) ?? []), r]);
  }
  for (const rows of byGroup.values()) rows.sort((a, b) => a.rule.name.localeCompare(b.rule.name));

  return (
    <div className="space-y-6">
      {header}
      {REACH_GROUPS.filter((g) => byGroup.has(g)).map((g) => (
        <Section key={g} title={RULE_GROUP_TITLE[g]}>
          {g === "egret_only" ? (
            <p className="text-sm text-muted-foreground">{EXEMPTION_NOT_ON_ENGINES}</p>
          ) : null}
          <RuleTable label={RULE_GROUP_TITLE[g]} rows={byGroup.get(g) ?? []} catalogNameById={catalogNameById} />
        </Section>
      ))}
      {rest}
      {pointer}
    </div>
  );
}

function RuleTable({
  label,
  rows,
  catalogNameById,
}: {
  label: string;
  rows: Row[];
  catalogNameById: ReadonlyMap<string, string>;
}) {
  return (
    <Table aria-label={label}>
      <TableHeader>
        <TableRow>
          <TableHead>Rule</TableHead>
          <TableHead>{TERM.appliesTo}</TableHead>
          <TableHead>How the exemption reaches</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {rows.map(({ rule, reach }) => {
          // A path rule opens the object its path names; a tag rule's tag is on the rule's page.
          const link = rule.selector.type === "path" ? scopeToDataLink(rule.selector, catalogNameById, "masks") : null;
          return (
            <TableRow key={rule.id}>
              <TableCell className="whitespace-normal">
                <div className="space-y-1">
                  <Link to={`/governance/rules/${encodeURIComponent(rule.id)}`} className="text-sm font-medium text-primary hover:underline">
                    {rule.name}
                  </Link>
                  <p className="text-xs text-muted-foreground break-words">{formatRuleKind(rule)}</p>
                </div>
              </TableCell>
              <TableCell className="whitespace-normal">
                <div className="space-y-1">
                  <p className="text-sm break-words">{formatSelector(rule)}</p>
                  {link ? <OpenInDataLink to={link.to} subject={rule.name} stoppedAtPattern={link.stoppedAtPattern} /> : null}
                </div>
              </TableCell>
              <TableCell className="whitespace-normal">
                <ul role="list" className="space-y-0.5 text-sm">
                  {reach.map((r, i) => (
                    <li key={i}>{reachText(r, "rule")}</li>
                  ))}
                </ul>
              </TableCell>
            </TableRow>
          );
        })}
      </TableBody>
    </Table>
  );
}
