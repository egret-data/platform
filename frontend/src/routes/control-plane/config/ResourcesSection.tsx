import { useState } from "react";

import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { KeyValueEditor } from "../EngineResourceGroups";
import { EngineRoleConfig } from "../EngineRoleConfig";
import type { EngineConfigForm } from "../useEngineConfigForm";
import { ConfigSection } from "./ConfigSection";

/**
 * "Resources" merged section: a Coordinator/Worker role toggle rendering the
 * existing per-role editor (`EngineRoleConfig`) for the selected role, plus the
 * cluster-wide environment variables editor (moved out of the former General
 * section). Composes existing editors — no new fields.
 */
export function ResourcesSection({ cfg }: { cfg: EngineConfigForm }) {
  const {
    coordinator,
    setCoordinator,
    worker,
    setWorker,
    gsEnabled,
    setGsEnabled,
    gracePeriod,
    setGracePeriod,
    env,
    setEnv,
  } = cfg;

  const [role, setRole] = useState<"coordinator" | "worker">("coordinator");

  return (
    <div className="space-y-5">
      <ConfigSection
        title="Per-role resources"
        description="CPU, memory, JVM & node config applied to the selected role's Deployment."
      >
        <Tabs value={role} onValueChange={(v) => setRole(v as typeof role)}>
          <TabsList>
            <TabsTrigger value="coordinator">Coordinator</TabsTrigger>
            <TabsTrigger value="worker">Worker</TabsTrigger>
          </TabsList>
        </Tabs>
        <div className="pt-4">
          {role === "coordinator" ? (
            <EngineRoleConfig
              role="coordinator"
              value={coordinator}
              onChange={setCoordinator}
            />
          ) : (
            <EngineRoleConfig
              role="worker"
              value={worker}
              onChange={setWorker}
              gsEnabled={gsEnabled}
              onGsEnabledChange={setGsEnabled}
              gracePeriod={gracePeriod}
              onGracePeriodChange={setGracePeriod}
            />
          )}
        </div>
      </ConfigSection>

      <ConfigSection
        title="Environment variables"
        description="Extra env applied to both coordinator and worker pods."
      >
        <KeyValueEditor
          rows={env}
          onChange={setEnv}
          keyPlaceholder="MY_VAR"
          valuePlaceholder="value"
          addLabel="Add variable"
        />
      </ConfigSection>
    </div>
  );
}
