import type { Dispatch, SetStateAction } from "react";

import type { DeploymentStrategy, SecurityContext } from "@/api/controlPlane";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { type Kv, KeyValueEditor } from "../EngineResourceGroups";
import { ConfigSection } from "@/components/form/ConfigSection";
import { ConfigTable, ConfigTableRow } from "@/components/config/ConfigTable";
import { RawJsonField } from "./RawJsonField";
import { SELECT_NONE, numOrUndef } from "./podEditors";

// ── security-context editor (pod + container) ────────────────────────────────

const SC_NUM_FIELDS: { key: "runAsUser" | "runAsGroup" | "fsGroup" }[] = [
  { key: "runAsUser" },
  { key: "runAsGroup" },
  { key: "fsGroup" },
];

function SecurityContextEditor({
  idPrefix,
  rawId,
  title,
  value,
  onChange,
}: {
  idPrefix: string;
  rawId: string;
  title: string;
  value: SecurityContext;
  onChange: Dispatch<SetStateAction<SecurityContext>>;
}) {
  const setField = <K extends keyof SecurityContext>(k: K, v: SecurityContext[K]) =>
    onChange((prev) => ({ ...prev, [k]: v }));

  return (
    <div className="space-y-3">
      <Label className="text-xs font-medium">{title}</Label>
      <ConfigTable>
        {SC_NUM_FIELDS.map((f) => (
          <ConfigTableRow
            key={f.key}
            label={f.key}
            mono
            details={null}
            htmlFor={`${idPrefix}-${f.key}`}
          >
            <Input
              id={`${idPrefix}-${f.key}`}
              className="w-full"
              type="number"
              value={value[f.key]?.toString() ?? ""}
              placeholder="1000"
              onChange={(e) => setField(f.key, numOrUndef(e.target.value))}
            />
          </ConfigTableRow>
        ))}
        <ConfigTableRow
          label="runAsNonRoot"
          mono
          details={null}
          htmlFor={`${idPrefix}-nonroot`}
        >
          <Switch
            id={`${idPrefix}-nonroot`}
            checked={value.runAsNonRoot ?? false}
            onCheckedChange={(v) => setField("runAsNonRoot", v)}
          />
        </ConfigTableRow>
      </ConfigTable>
      <RawJsonField
        id={rawId}
        label="Raw override (merged on top)"
        value={value.rawOverride}
        onChange={(parsed) => setField("rawOverride", parsed)}
        placeholder='{"readOnlyRootFilesystem": true}'
      />
    </div>
  );
}

// ── section props ────────────────────────────────────────────────────────────

export interface PodSectionProps {
  serviceAccount: string;
  setServiceAccount: (v: string) => void;
  env: Kv[];
  setEnv: Dispatch<SetStateAction<Kv[]>>;
  podSecurityContext: SecurityContext;
  setPodSecurityContext: Dispatch<SetStateAction<SecurityContext>>;
  containerSecurityContext: SecurityContext;
  setContainerSecurityContext: Dispatch<SetStateAction<SecurityContext>>;
  deploymentStrategy: DeploymentStrategy;
  setDeploymentStrategy: Dispatch<SetStateAction<DeploymentStrategy>>;
  deploymentAnnotations: Kv[];
  setDeploymentAnnotations: Dispatch<SetStateAction<Kv[]>>;
  revisionHistoryLimit: string;
  setRevisionHistoryLimit: (v: string) => void;
  progressDeadlineSeconds: string;
  setProgressDeadlineSeconds: (v: string) => void;
}

/**
 * "Pod" config: cluster-wide pod-shape knobs applied to both the coordinator &
 * worker Deployments — service account, env vars, pod & container security
 * contexts, the Deployment rollout strategy, and shared-PID namespace.
 */
export function PodSection({
  serviceAccount,
  setServiceAccount,
  env,
  setEnv,
  podSecurityContext,
  setPodSecurityContext,
  containerSecurityContext,
  setContainerSecurityContext,
  deploymentStrategy,
  setDeploymentStrategy,
  deploymentAnnotations,
  setDeploymentAnnotations,
  revisionHistoryLimit,
  setRevisionHistoryLimit,
  progressDeadlineSeconds,
  setProgressDeadlineSeconds,
}: PodSectionProps) {
  return (
    <div className="space-y-5">
      <ConfigSection
        title="Service account & environment"
        description="Applied to both the coordinator and worker pods."
      >
        <ConfigTable>
          <ConfigTableRow
            label="Service account"
            htmlFor="ps-sa"
            details="Kubernetes ServiceAccount for both the coordinator and worker pods."
          >
            <Input
              id="ps-sa"
              className="w-full"
              value={serviceAccount}
              placeholder="default"
              onChange={(e) => setServiceAccount(e.target.value)}
            />
          </ConfigTableRow>
        </ConfigTable>
        <div className="space-y-2">
          <Label>Environment variables</Label>
          <p className="text-xs text-muted-foreground">
            Extra env applied to both coordinator and worker pods.
          </p>
          <KeyValueEditor
            rows={env}
            onChange={setEnv}
            keyPlaceholder="MY_VAR"
            valuePlaceholder="value"
            addLabel="Add variable"
          />
        </div>
      </ConfigSection>

      <ConfigSection
        title="Security contexts"
        description="Pod- and container-level securityContext, with a raw override merged on top."
      >
        <div className="space-y-6">
          <SecurityContextEditor
            idPrefix="ps-podsc"
            rawId="pod.podSecurityContext.raw"
            title="Pod security context"
            value={podSecurityContext}
            onChange={setPodSecurityContext}
          />
          <SecurityContextEditor
            idPrefix="ps-ctrsc"
            rawId="pod.containerSecurityContext.raw"
            title="Container security context"
            value={containerSecurityContext}
            onChange={setContainerSecurityContext}
          />
        </div>
      </ConfigSection>

      <ConfigSection
        title="Deployment"
        description="Rollout strategy and Deployment-level metadata for both roles."
      >
        <ConfigTable>
          <ConfigTableRow
            label="Strategy"
            htmlFor="ps-strategy"
            details="Deployment rollout strategy for both roles."
          >
            <Select
              value={deploymentStrategy.kind || SELECT_NONE}
              onValueChange={(v) =>
                setDeploymentStrategy((prev) => ({
                  ...prev,
                  kind: v === SELECT_NONE ? "" : v,
                }))
              }
            >
              <SelectTrigger id="ps-strategy" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={SELECT_NONE}>(default)</SelectItem>
                <SelectItem value="RollingUpdate">RollingUpdate</SelectItem>
                <SelectItem value="Recreate">Recreate</SelectItem>
              </SelectContent>
            </Select>
          </ConfigTableRow>
          {deploymentStrategy.kind === "RollingUpdate" && (
            <>
              <ConfigTableRow
                label="maxSurge"
                htmlFor="ps-maxsurge"
                details="Extra pods allowed above the desired count during a rollout."
              >
                <Input
                  id="ps-maxsurge"
                  className="w-full"
                  value={deploymentStrategy.maxSurge ?? ""}
                  placeholder="25%"
                  onChange={(e) =>
                    setDeploymentStrategy((prev) => ({
                      ...prev,
                      maxSurge: e.target.value || undefined,
                    }))
                  }
                />
              </ConfigTableRow>
              <ConfigTableRow
                label="maxUnavailable"
                htmlFor="ps-maxunavail"
                details="Pods that may be unavailable during a rollout."
              >
                <Input
                  id="ps-maxunavail"
                  className="w-full"
                  value={deploymentStrategy.maxUnavailable ?? ""}
                  placeholder="25%"
                  onChange={(e) =>
                    setDeploymentStrategy((prev) => ({
                      ...prev,
                      maxUnavailable: e.target.value || undefined,
                    }))
                  }
                />
              </ConfigTableRow>
            </>
          )}
          <ConfigTableRow
            label="revisionHistoryLimit"
            htmlFor="ps-revlimit"
            details="Old ReplicaSets retained for rollback."
          >
            <Input
              id="ps-revlimit"
              className="w-full"
              type="number"
              value={revisionHistoryLimit}
              placeholder="10"
              onChange={(e) => setRevisionHistoryLimit(e.target.value)}
            />
          </ConfigTableRow>
          <ConfigTableRow
            label="progressDeadlineSeconds"
            htmlFor="ps-progress"
            details="Seconds before a stalled rollout is marked failed."
          >
            <Input
              id="ps-progress"
              className="w-full"
              type="number"
              value={progressDeadlineSeconds}
              placeholder="600"
              onChange={(e) => setProgressDeadlineSeconds(e.target.value)}
            />
          </ConfigTableRow>
        </ConfigTable>

        <div className="space-y-2">
          <Label>Deployment annotations</Label>
          <KeyValueEditor
            rows={deploymentAnnotations}
            onChange={setDeploymentAnnotations}
            keyPlaceholder="team"
            valuePlaceholder="data"
            addLabel="Add annotation"
          />
        </div>
      </ConfigSection>
    </div>
  );
}
