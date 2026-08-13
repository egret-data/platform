import { EngineRoleConfig } from "../EngineRoleConfig";
import type { EngineConfigForm } from "../useEngineConfigForm";
import { ConfigSection } from "@/components/form/ConfigSection";
import { RolePlacementEditor } from "./PlacementSection";

/**
 * "Worker" section — mirrors the chart `worker` block: the role's sizing / JVM /
 * resources / probes editor (incl. graceful shutdown) plus its placement fields.
 */
export function WorkerSection({ cfg }: { cfg: EngineConfigForm }) {
  const { worker, setWorker, gsEnabled, setGsEnabled, gracePeriod, setGracePeriod } = cfg;
  return (
    <div className="space-y-5">
      <ConfigSection
        title="Worker resources"
        description="CPU, memory, JVM, node config & graceful shutdown for the worker Deployment."
      >
        <EngineRoleConfig
          role="worker"
          value={worker}
          onChange={setWorker}
          gsEnabled={gsEnabled}
          onGsEnabledChange={setGsEnabled}
          gracePeriod={gracePeriod}
          onGracePeriodChange={setGracePeriod}
        />
      </ConfigSection>
      <ConfigSection
        title="Worker placement"
        description="Node selector, tolerations, topology spread & affinity for the worker."
      >
        <RolePlacementEditor
          idPrefix="worker"
          rawId="placement.worker.affinity.raw"
          form={worker}
          onChange={setWorker}
        />
      </ConfigSection>
    </div>
  );
}
