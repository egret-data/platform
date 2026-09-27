import { UserCheck } from "lucide-react";

import { EmptyState } from "@/components/explore/EmptyState";

/**
 * Governance → Data permissions (SP4b-2 Task 1: reachable shell only).
 *
 * Steward-facing: "who can do what" as a sentence built from `perm.grants`
 * (SP4a). NOT the RBAC permission *catalog* at Control Plane → Access →
 * Permissions — that page lists the fixed set of fine-grained permissions
 * egret understands; this one lists grants over data. Deliberately kept
 * distinct in name (see the design spec's naming hazard) so the two are never
 * confused in conversation or in a bug report.
 *
 * Task 3 fills this in with the grant list + `GrantDialog`; this task only
 * makes the page reachable and gated so later work has somewhere to land.
 */
export default function DataPermissions() {
  return (
    <div className="space-y-5">
      <div>
        <h1 className="text-2xl font-semibold leading-tight">Data permissions</h1>
        <p className="text-sm text-muted-foreground">
          Who can do what with data, read as a sentence — e.g. "analyst can read
          wh.sales.* except column pii".
        </p>
      </div>

      <EmptyState
        icon={UserCheck}
        variant="dashed"
        title="Nothing here yet"
        description="Grants will appear here once authoring lands."
      />
    </div>
  );
}
