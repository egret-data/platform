import { Search } from "lucide-react";

import { EmptyState } from "@/components/explore/EmptyState";

/**
 * Governance → Explain access (SP4b-2 Task 1: reachable shell only).
 *
 * Admin-facing: subject + action + target in, `allowed` and the reason class
 * out (D9). Task 5 fills this in with the explain form and result; this task
 * only makes the page reachable and gated so later work has somewhere to
 * land.
 */
export default function ExplainAccess() {
  return (
    <div className="space-y-5">
      <div>
        <h1 className="text-2xl font-semibold leading-tight">Explain access</h1>
        <p className="text-sm text-muted-foreground">
          Why a subject can or cannot reach a target — the reason class, not
          just the boolean.
        </p>
      </div>

      <EmptyState
        icon={Search}
        variant="dashed"
        title="Nothing here yet"
        description="The explain form will appear here once it lands."
      />
    </div>
  );
}
