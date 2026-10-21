import { useState, type ReactNode } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { Plus } from "lucide-react";
import type { RuleView } from "@/api/generated/model";
import { useControlPlaneGovernanceTagUsage } from "@/api/generated/control-plane-governance/control-plane-governance";
import { usePermListRules } from "@/api/generated/perm/perm";
import type { Tag } from "@/api/tags";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { useCanManageTags } from "@/routes/governance/classifications/tagParts";
import { TERM } from "@/routes/governance/glossary";
import { formatRuleKind } from "@/routes/governance/permFormat";
import { usePolicyPermissions } from "@/routes/governance/policyWrites";
import { RuleDialog } from "@/routes/governance/RuleDialog";
import { rulesOnTag } from "@/routes/governance/ruleMatch";
import { TabError } from "@/routes/governance/tabParts";

/** The status line of a classification nothing uses (spec C2). */
export const DOES_NOTHING_YET = "Does nothing yet.";
export const NOTHING_USES_IT =
  "Nothing uses this classification yet. Attach it to data, add a rule, or set clearance to give it an effect.";
export const NO_RULE_YET = "No rule uses this classification yet.";
export const NO_CLEARANCE = "None: anyone who can read the data can open it.";
export const NOT_ON_DATA_YET = "Not on any data yet.";
export const ATTACH_HOW = "Open a table or a column and use Add classification.";
export const RULES_NEED_POLICY_READ_SHORT = "Reading rules needs the policy:read permission.";
export const ADD_RULE_NEEDS_POLICY_UPDATE = "Adding a rule needs the policy:update permission.";
export const SET_CLEARANCE_NEEDS_TAG_MANAGE = "Setting clearance needs the tag:manage permission.";

const count = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** "Mask · Hash: Hash email", and " and N more" when several rules reach the classification. */
function rulesText(rules: RuleView[]): string {
  const [first, ...rest] = [...rules].sort((a, b) => a.name.localeCompare(b.name));
  const head = `${formatRuleKind(first)}: ${first.name}`;
  return rest.length > 0 ? `${head} and ${rest.length} more` : head;
}

/** What the stored clearance says; egret enforces it, engines only through its roles (spec C9). */
function clearanceText(tag: Tag): string {
  const roles = tag.required_roles.length;
  const groups = tag.required_groups.length;
  if (roles === 0 && groups === 0) return NO_CLEARANCE;
  const who = [roles > 0 ? count(roles, "role", "roles") : null, groups > 0 ? count(groups, "group", "groups") : null]
    .filter(Boolean)
    .join(" or ");
  const engines = roles === 0 ? " Engines do not enforce it." : "";
  return `Only ${who} may open data with it in egret.${engines}`;
}

function Fact({ term, children }: { term: string; children: ReactNode }) {
  return (
    <div className="min-w-0 space-y-1">
      <dt className="text-xs font-medium uppercase tracking-wide text-muted-foreground">{term}</dt>
      {children}
    </div>
  );
}

function Loading() {
  return (
    <dd>
      <Skeleton className="h-4 w-40" />
    </dd>
  );
}

/** A missing permission, said beside the fact (FE-ACTION-4). */
function Needs({ children }: { children: string }) {
  return <dd className="text-xs text-muted-foreground">{children}</dd>;
}

/**
 * What a classification does, stated from the data (spec C2): the rules that
 * select it, its clearance and where it is attached, each with the action that
 * changes it. Every fact has its own loading and error (FE-STATE-1); a fact the
 * viewer may not read says which permission reads it. "Does nothing yet." is
 * said only when all three are known to be empty, never while one is unknown.
 * The strip claims no enforcement: it repeats what is stored.
 *
 * Refreshing: it reads `GET /perm/rules` (invalidated by every rule write),
 * the tag list (`tagsQueryKey`, invalidated by every clearance write) and the
 * tag's usage (invalidated by every attach and remove through
 * `invalidateTagAssignment`/`invalidateTag`).
 */
export function EffectStrip({ tag }: { tag: Tag }) {
  const can = usePolicyPermissions();
  const canReadRules = can("policy:read");
  const canAddRule = can("policy:update");
  const canManage = useCanManageTags();
  const rulesQ = usePermListRules({ query: { retry: false, enabled: canReadRules } });
  const usageQ = useControlPlaneGovernanceTagUsage(tag.id, { query: { retry: false } });
  const [, setSearch] = useSearchParams();
  const [adding, setAdding] = useState(false);

  // Rules: null while unknown (no permission, loading, failed).
  let rules: RuleView[] | null = null;
  let rulesFact: ReactNode;
  if (!canReadRules) rulesFact = <Needs>{RULES_NEED_POLICY_READ_SHORT}</Needs>;
  else if (rulesQ.isError)
    rulesFact = (
      <dd>
        <TabError what="rules" onRetry={() => void rulesQ.refetch()} />
      </dd>
    );
  else if (rulesQ.data?.status !== 200) rulesFact = <Loading />;
  else {
    rules = rulesOnTag(rulesQ.data.data, tag);
    rulesFact = <dd className="text-sm">{rules.length === 0 ? NO_RULE_YET : rulesText(rules)}</dd>;
  }

  // Where used: null while unknown.
  let unused: boolean | null = null;
  let usageFact: ReactNode;
  if (usageQ.isError)
    usageFact = (
      <dd>
        <TabError what="classification's usage" onRetry={() => void usageQ.refetch()} />
      </dd>
    );
  else if (usageQ.data?.status !== 200) usageFact = <Loading />;
  else {
    const { data, bi } = usageQ.data.data;
    const biObjects = bi.readable.length + bi.unreadable.count;
    unused =
      data.tableCount === 0 && data.columnCount === 0 && data.orphaned === 0 && biObjects === 0 && bi.remaining === 0;
    const text = unused
      ? NOT_ON_DATA_YET
      : [
          count(data.tableCount, "table", "tables"),
          count(data.columnCount, "column", "columns"),
          count(biObjects, "BI object", "BI objects"),
        ].join(" · ") + (bi.remaining > 0 ? " (not every BI object was checked)" : "");
    usageFact = <dd className="text-sm tabular-nums">{text}</dd>;
  }

  const hasClearance = tag.required_roles.length + tag.required_groups.length > 0;
  const doesNothing = rules !== null && rules.length === 0 && !hasClearance && unused === true;

  return (
    <section aria-label="Effect" className="space-y-3 rounded-md border p-3">
      {doesNothing ? (
        <div>
          <p className="text-sm font-medium">{DOES_NOTHING_YET}</p>
          <p className="text-sm text-muted-foreground">{NOTHING_USES_IT}</p>
        </div>
      ) : null}
      <dl className="grid grid-cols-1 gap-3 sm:grid-cols-3">
        <Fact term={TERM.rules}>
          {rulesFact}
          {canAddRule ? (
            <dd>
              <Button size="sm" variant="outline" onClick={() => setAdding(true)}>
                <Plus className="mr-1 h-4 w-4" aria-hidden />
                {TERM.addRule}
              </Button>
            </dd>
          ) : canReadRules ? (
            <Needs>{ADD_RULE_NEEDS_POLICY_UPDATE}</Needs>
          ) : null}
        </Fact>
        <Fact term={TERM.clearance}>
          <dd className="text-sm">{clearanceText(tag)}</dd>
          {canManage ? (
            <dd>
              <Button
                size="sm"
                variant="outline"
                onClick={() =>
                  setSearch((prev) => {
                    const params = new URLSearchParams(prev);
                    params.set("tab", "clearance");
                    return params;
                  })
                }
              >
                Set clearance
              </Button>
            </dd>
          ) : (
            <Needs>{SET_CLEARANCE_NEEDS_TAG_MANAGE}</Needs>
          )}
        </Fact>
        <Fact term={TERM.whereUsed}>
          {usageFact}
          <dd className="text-xs text-muted-foreground">
            <Link to="/governance/data" className="font-medium text-primary hover:underline">
              Attach in Data
            </Link>{" "}
            {ATTACH_HOW}
          </dd>
        </Fact>
      </dl>
      {canAddRule ? (
        <RuleDialog
          open={adding}
          onOpenChange={setAdding}
          entry={{ kind: "classification", tagKey: tag.key, tagName: tag.name }}
        />
      ) : null}
    </section>
  );
}
