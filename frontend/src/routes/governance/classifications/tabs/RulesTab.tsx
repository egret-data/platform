import type { Tag } from "@/api/tags";
import { usePolicyPermissions } from "@/routes/governance/policyWrites";
import TagRulesSection from "@/routes/governance/TagRulesSection";

export const RULES_NEED_POLICY_READ =
  "Rules are read with the policy:read permission, which this account does not have.";

/**
 * Rules (spec §5.3; "Mask & filter" before the glossary): the rules that mask
 * or filter data carrying this classification (`TagRulesSection`, each linking
 * to its rule page, which links back here). Rules are read with
 * `policy:read`; without it the tab says so and asks nothing.
 */
export function RulesTab({ tag }: { tag: Tag }) {
  const canReadRules = usePolicyPermissions()("policy:read");
  if (!canReadRules) return <p className="text-sm text-muted-foreground">{RULES_NEED_POLICY_READ}</p>;
  return <TagRulesSection tag={tag} tagId={tag.id} />;
}
