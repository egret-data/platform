import {
  ResourceGroupsSection,
  SessionPropertiesSection,
} from "../EngineResourceGroups";
import { EngineConfigReliability } from "../EngineConfigReliability";
import type { EngineConfigForm } from "../useEngineConfigForm";

/**
 * "Workload" merged section: resource groups + session properties (from
 * `EngineResourceGroups`) plus the fault-tolerant execution editor (retry policy
 * + exchange storage connection + prefix, from `EngineConfigReliability`).
 * Composes existing editors — no new fields.
 */
export function WorkloadSection({ cfg }: { cfg: EngineConfigForm }) {
  const {
    resourceGroups,
    setResourceGroups,
    sessionRules,
    setSessionRules,
    ftEnabled,
    setFtEnabled,
    retryPolicy,
    setRetryPolicy,
    exchangeStorageConnId,
    setExchangeStorageConnId,
    exchangePrefix,
    setExchangePrefix,
    getProp,
    setProp,
  } = cfg;

  return (
    <div className="space-y-5">
      <ResourceGroupsSection
        value={resourceGroups}
        onChange={setResourceGroups}
      />
      <SessionPropertiesSection value={sessionRules} onChange={setSessionRules} />
      <EngineConfigReliability
        ftEnabled={ftEnabled}
        setFtEnabled={setFtEnabled}
        retryPolicy={retryPolicy}
        setRetryPolicy={setRetryPolicy}
        exchangeStorageConnId={exchangeStorageConnId}
        setExchangeStorageConnId={setExchangeStorageConnId}
        exchangePrefix={exchangePrefix}
        setExchangePrefix={setExchangePrefix}
        getProp={getProp}
        setProp={setProp}
      />
    </div>
  );
}
