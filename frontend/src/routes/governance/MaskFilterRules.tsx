import { EyeOff } from "lucide-react";

import { EmptyState } from "@/components/explore/EmptyState";

/**
 * Governance → Mask & filter rules (SP4b-2 Task 1: reachable shell only).
 *
 * Steward-facing: named `perm.rules` (SP4a) that apply masking or row-filter
 * behavior by classification tag or by path. Task 4 fills this in with the
 * rule list + its editor; this task only makes the page reachable and gated
 * so later work has somewhere to land.
 */
export default function MaskFilterRules() {
  return (
    <div className="space-y-5">
      <div>
        <h1 className="text-2xl font-semibold leading-tight">Mask &amp; filter rules</h1>
        <p className="text-sm text-muted-foreground">
          Named rules that mask columns or filter rows wherever a classification
          or a path matches.
        </p>
      </div>

      <EmptyState
        icon={EyeOff}
        variant="dashed"
        title="Nothing here yet"
        description="Masking and row-filter rules will appear here once authoring lands."
      />
    </div>
  );
}
