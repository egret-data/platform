import {
  ResourceGroupsSection,
  SessionPropertiesSection,
} from "../EngineResourceGroups";
import type { EngineConfigForm } from "../useEngineConfigForm";

/**
 * "Workload" merged section: resource groups + session properties (from
 * `EngineResourceGroups`). Composes existing editors — no new fields.
 */
export function WorkloadSection({ cfg }: { cfg: EngineConfigForm }) {
  const { resourceGroups, setResourceGroups, sessionRules, setSessionRules } =
    cfg;

  return (
    <div className="space-y-5">
      <ResourceGroupsSection
        value={resourceGroups}
        onChange={setResourceGroups}
      />
      <SessionPropertiesSection value={sessionRules} onChange={setSessionRules} />
    </div>
  );
}
