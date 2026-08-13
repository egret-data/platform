import { EngineRoleConfig } from "../EngineRoleConfig";
import type { EngineConfigForm } from "../useEngineConfigForm";
import { ConfigSection } from "@/components/form/ConfigSection";
import { RolePlacementEditor } from "./PlacementSection";

/**
 * "Coordinator" section — mirrors the chart `coordinator` block: the role's
 * sizing / JVM / resources / probes editor plus its pod-placement fields.
 */
export function CoordinatorSection({ cfg }: { cfg: EngineConfigForm }) {
  const { coordinator, setCoordinator } = cfg;
  return (
    <div className="space-y-5">
      <ConfigSection
        title="Coordinator resources"
        description="CPU, memory, JVM & node config for the coordinator Deployment."
      >
        <EngineRoleConfig role="coordinator" value={coordinator} onChange={setCoordinator} />
      </ConfigSection>
      <ConfigSection
        title="Coordinator placement"
        description="Node selector, tolerations, topology spread & affinity for the coordinator."
      >
        <RolePlacementEditor
          idPrefix="coord"
          rawId="placement.coordinator.affinity.raw"
          form={coordinator}
          onChange={setCoordinator}
        />
      </ConfigSection>
    </div>
  );
}
