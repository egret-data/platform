import { Link } from "react-router-dom";
import { EyeOff } from "lucide-react";
import type { RuleView } from "@/api/generated/model";
import { usePermListRules } from "@/api/generated/perm/perm";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { useWorkspaceNames } from "@/routes/governance/openInData";
import { TERM } from "@/routes/governance/glossary";
import { formatRuleKind } from "@/routes/governance/permFormat";
import { PrincipalNames } from "@/routes/governance/PrincipalNames";
import { rulesOnTag } from "@/routes/governance/ruleMatch";
import { Section, TabError } from "@/routes/governance/tabParts";

/** How a listed rule reaches the classification: written on it, or on another one whose name matches. */
function reachText(r: RuleView, tag: { key: string; name: string }): string {
  if (r.selector.type !== "tag" || r.selector.key === tag.key) return `This ${TERM.classification.toLowerCase()}`;
  if (r.tag_name === tag.name) return `${TERM.classification} ${r.selector.key}, which has the same name`;
  return `${TERM.classification} ${r.selector.key}, whose name “${r.tag_name}” matches this one's`;
}

/** A rule's page, opened from this classification's Rules tab, so its way back returns here. */
function ruleHref(ruleId: string, tagId: string): string {
  return `/governance/rules/${encodeURIComponent(ruleId)}?from=classification:${encodeURIComponent(tagId)}`;
}

/**
 * A classification's mask and row-filter rules (DG redesign #2, P9): the home
 * of classification-selected rules now that the flat rule list is gone.
 * Engines match such a rule by the classification's name (`rulesOnTag`), so a
 * rule written on another classification of the same name is listed here too,
 * and says which one it was written on. Each rule links to its own page with
 * `?from=classification:<tagId>`, so the page's way back names this one. Read
 * from `GET /perm/rules`, so the page renders this only for a `policy:read`
 * holder.
 */
export default function TagRulesSection({ tag, tagId }: { tag: { key: string; name: string }; tagId: string }) {
  const rulesQ = usePermListRules({ query: { retry: false } });
  const workspaceName = useWorkspaceNames();

  let body;
  if (rulesQ.isError) body = <TabError what="rules" onRetry={() => void rulesQ.refetch()} />;
  else if (rulesQ.data?.status !== 200) body = <ListSkeleton rows={3} cols={3} />;
  else {
    const rules = rulesOnTag(rulesQ.data.data, tag).sort((a, b) => a.name.localeCompare(b.name));
    body =
      rules.length === 0 ? (
        <EmptyState icon={EyeOff} title="No rule uses this classification yet." />
      ) : (
        <Table aria-label="Rules on this classification">
          <TableHeader>
            <TableRow>
              <TableHead>{TERM.rule}</TableHead>
              <TableHead>Kind</TableHead>
              <TableHead>Applies through</TableHead>
              <TableHead>{TERM.exempt}</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {rules.map((r) => (
              <TableRow key={r.id}>
                <TableCell className="whitespace-normal">
                  <Link to={ruleHref(r.id, tagId)} className="font-medium text-primary hover:underline">
                    {r.name}
                  </Link>
                </TableCell>
                <TableCell className="whitespace-normal break-words font-mono text-xs">{formatRuleKind(r)}</TableCell>
                <TableCell className="whitespace-normal text-sm">{reachText(r, tag)}</TableCell>
                <TableCell className="whitespace-normal text-sm">
                  <PrincipalNames list={r.exempt.include} workspaceName={workspaceName} />
                  {r.exempt.exclude.length > 0 ? (
                    <>
                      {" except "}
                      <PrincipalNames list={r.exempt.exclude} workspaceName={workspaceName} />
                    </>
                  ) : null}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      );
  }

  return (
    <Section title={TERM.rules}>
      <p className="text-sm text-muted-foreground">
        Rules that mask or filter data carrying this classification. Engines match a rule by the classification&apos;s
        name, so a rule written on another classification with the same name applies here too.
      </p>
      {body}
    </Section>
  );
}
