import { useState } from "react";
import { Link } from "react-router-dom";
import { EyeOff, Plus } from "lucide-react";
import type { RuleView } from "@/api/generated/model";
import { usePermListRules } from "@/api/generated/perm/perm";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Button } from "@/components/ui/button";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { useWorkspaceNames } from "@/routes/governance/openInData";
import { usePolicyPermissions } from "@/routes/governance/policyWrites";
import { RuleDialog } from "@/routes/governance/RuleDialog";
import { TERM } from "@/routes/governance/glossary";
import { RuleKindLabel } from "@/routes/governance/RuleKindLabel";
import { NotEnforcedBadge, useRuleEnforcement } from "@/routes/governance/NotEnforced";
import { PrincipalNames } from "@/routes/governance/PrincipalNames";
import { rulesOnTag } from "@/routes/governance/ruleMatch";
import { Section, TabError } from "@/routes/governance/tabParts";
import { classificationRuleUrl } from "@/routes/governance/classifications/tagUrl";

/** How a listed rule reaches the classification: written on it, or on another one whose name matches. */
function reachText(r: RuleView, tag: { key: string; name: string }): string {
  if (r.selector.type !== "tag" || r.selector.key === tag.key) return `This ${TERM.classification.toLowerCase()}`;
  if (r.tag_name === tag.name) return `${TERM.classification} ${r.selector.key}, which has the same name`;
  return `${TERM.classification} ${r.selector.key}, whose name “${r.tag_name}” matches this one's`;
}

/**
 * A classification's mask and row-filter rules (DG redesign #2, P9): the home
 * of classification-selected rules now that the flat rule list is gone.
 * Engines match such a rule by the classification's name (`rulesOnTag`), so a
 * rule written on another classification of the same name is listed here too,
 * and says which one it was written on. Each rule links to its own page with
 * `?from=classification:<tagId>`, so the page's way back names this one. Read
 * from `GET /perm/rules`, so the page renders this only for a `policy:read`
 * holder. **Add rule** (toolbar and empty state, `policy:update`) opens the
 * rule dialog on this classification (spec C2, entry `classification`);
 * without the permission the section says so (FE-ACTION-4). A rule no engine
 * enforces says "Not enforced yet — <reason>" on its row, once
 * `GET /perm/rules/enforcement` says so.
 */
export default function TagRulesSection({ tag, tagId }: { tag: { key: string; name: string }; tagId: string }) {
  const rulesQ = usePermListRules({ query: { retry: false } });
  const workspaceName = useWorkspaceNames();
  const canAdd = usePolicyPermissions()("policy:update");
  const [adding, setAdding] = useState(false);
  const listedRules =
    rulesQ.data?.status === 200 ? rulesOnTag(rulesQ.data.data, tag).sort((a, b) => a.name.localeCompare(b.name)) : [];
  const enforcement = useRuleEnforcement(listedRules.map((r) => r.id));
  const addButton = (
    <Button size="sm" variant="outline" onClick={() => setAdding(true)}>
      <Plus className="mr-1 h-4 w-4" aria-hidden />
      {TERM.addRule}
    </Button>
  );

  let body;
  // How many rules the list shows; the toolbar's Add rule appears only beside a list.
  let listed = 0;
  if (rulesQ.isError) body = <TabError what="rules" onRetry={() => void rulesQ.refetch()} />;
  else if (rulesQ.data?.status !== 200) body = <ListSkeleton rows={3} cols={3} />;
  else {
    const rules = listedRules;
    listed = rules.length;
    body =
      rules.length === 0 ? (
        <EmptyState icon={EyeOff} title="No rule uses this classification yet." action={canAdd ? addButton : undefined} />
      ) : (
        <Table aria-label="Rules on this classification">
          <TableHeader>
            <TableRow>
              <TableHead>{TERM.rule}</TableHead>
              <TableHead>{TERM.kind}</TableHead>
              <TableHead>{TERM.appliesThrough}</TableHead>
              <TableHead>{TERM.exempt}</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {rules.map((r) => (
              <TableRow key={r.id}>
                <TableCell className="whitespace-normal">
                  <Link to={classificationRuleUrl(r.id, tagId)} className="font-medium text-primary hover:underline">
                    {r.name}
                  </Link>
                  <div>
                    <NotEnforcedBadge enforcement={enforcement?.get(r.id)} appliesTo={r.selector.type} />
                  </div>
                </TableCell>
                <TableCell className="whitespace-normal break-words text-sm">
                  <RuleKindLabel rule={r} />
                </TableCell>
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
      {/* While the list is empty its empty state carries Add rule, so the toolbar does not repeat it. */}
      {!canAdd ? (
        <p className="text-xs text-muted-foreground">Adding a rule needs the policy:update permission.</p>
      ) : listed > 0 ? (
        <div className="flex justify-end">{addButton}</div>
      ) : null}
      {body}
      {canAdd ? (
        <RuleDialog
          open={adding}
          onOpenChange={setAdding}
          entry={{ kind: "classification", tagKey: tag.key, tagName: tag.name }}
        />
      ) : null}
    </Section>
  );
}
