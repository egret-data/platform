import { Link } from "react-router-dom";
import { EyeOff } from "lucide-react";
import type { RuleView } from "@/api/generated/model";
import { usePermListRules } from "@/api/generated/perm/perm";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { useWorkspaceNames } from "@/routes/governance/openInData";
import { formatRuleKind } from "@/routes/governance/permFormat";
import { PrincipalNames } from "@/routes/governance/PrincipalNames";
import { rulesOnTag } from "@/routes/governance/ruleMatch";
import { Section, TabError } from "@/routes/governance/tabParts";

/** How a listed rule reaches the tag: written on it, or on another tag whose name matches. */
function reachText(r: RuleView, tag: { key: string; name: string }): string {
  if (r.selector.type !== "tag" || r.selector.key === tag.key) return "This tag";
  if (r.tag_name === tag.name) return `Written on tag ${r.selector.key}, which has this tag's name too`;
  return `Written on tag ${r.selector.key}, whose name “${r.tag_name}” matches this tag's name`;
}

/**
 * A tag's mask and row-filter rules (DG redesign #2, P9): the home of
 * tag-selected rules now that the flat rule list is gone. Engines match a tag
 * rule by the tag's name (`rulesOnTag`), so a rule written on another tag of
 * the same name is listed here too, and says which tag it was written on.
 * Each rule links to its own page. Read from `GET /perm/rules`, so the page
 * renders this only for a `policy:read` holder.
 */
export default function TagRulesSection({ tag }: { tag: { key: string; name: string } }) {
  const rulesQ = usePermListRules({ query: { retry: false } });
  const workspaceName = useWorkspaceNames();

  let body;
  if (rulesQ.isError) body = <TabError what="rules" onRetry={() => void rulesQ.refetch()} />;
  else if (rulesQ.data?.status !== 200) body = <ListSkeleton rows={3} cols={3} />;
  else {
    const rules = rulesOnTag(rulesQ.data.data, tag).sort((a, b) => a.name.localeCompare(b.name));
    body =
      rules.length === 0 ? (
        <EmptyState icon={EyeOff} title="No egret rule selects this tag." />
      ) : (
        <Table aria-label="Rules on this tag">
          <TableHeader>
            <TableRow>
              <TableHead>Rule</TableHead>
              <TableHead>Kind</TableHead>
              <TableHead>Reaches this tag</TableHead>
              <TableHead>Exempt</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {rules.map((r) => (
              <TableRow key={r.id}>
                <TableCell className="whitespace-normal">
                  <Link to={`/governance/rules/${encodeURIComponent(r.id)}`} className="font-medium text-primary hover:underline">
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
    <Section title="Mask and filter rules">
      <p className="text-sm text-muted-foreground">
        egret rules that mask or filter by this tag. Engines match a rule by its tag's name, so a rule written on
        another tag with the same name applies here too.
      </p>
      {body}
    </Section>
  );
}
