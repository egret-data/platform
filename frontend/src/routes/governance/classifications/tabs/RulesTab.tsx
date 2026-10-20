import type { Tag } from "@/api/tags";
import { usePolicyPermissions } from "@/routes/governance/policyWrites";
import TagRulesSection from "@/routes/governance/TagRulesSection";

export const RULES_NEED_POLICY_READ =
  "Rules are read with the policy:read permission, which this account does not have.";

/**
 * Mask & filter (spec §5.3): the egret rules that mask or filter by this tag
 * (`TagRulesSection`, each linking to its rule page). Rules are read with
 * `policy:read`; without it the tab says so and asks nothing.
 */
export function RulesTab({ tag }: { tag: Tag }) {
  const canReadRules = usePolicyPermissions()("policy:read");
  if (!canReadRules) return <p className="text-sm text-muted-foreground">{RULES_NEED_POLICY_READ}</p>;
  return <TagRulesSection tag={tag} />;
}
