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
import { ConfigSection } from "./ConfigSection";
import { ConfigTable, ConfigTableRow } from "./ConfigTable";
import { RawJsonField } from "./RawJsonField";
import { SELECT_NONE, numOrUndef } from "./podEditors";

// ── security-context editor (pod + container) ────────────────────────────────

const SC_NUM_FIELDS: { key: "runAsUser" | "runAsGroup" | "fsGroup"; label: string }[] =
  [
    { key: "runAsUser", label: "runAsUser" },
    { key: "runAsGroup", label: "runAsGroup" },
    { key: "fsGroup", label: "fsGroup" },
  ];

function SecurityContextEditor({
  idPrefix,
  rawId,
  title,
  keyPrefix,
  value,
  onChange,
}: {
  idPrefix: string;
  rawId: string;
  title: string;
  /** Chart values prefix — `securityContext` (pod) or `containerSecurityContext`. */
  keyPrefix: string;
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
            label={`${keyPrefix}.${f.key}`}
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
          label={`${keyPrefix}.runAsNonRoot`}
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
  shareProcessNamespace: boolean;
  setShareProcessNamespace: (v: boolean) => void;
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
  shareProcessNamespace,
  setShareProcessNamespace,
}: PodSectionProps) {
  return (
    <div className="space-y-5">
      <ConfigSection
        title="Service account & environment"
        description="Applied to both the coordinator and worker pods."
      >
        <ConfigTable>
          <ConfigTableRow
            label="serviceAccount.name"
            mono
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
          <Label className="font-mono text-xs">env</Label>
          <p className="text-xs text-muted-foreground">
            Extra environment variables applied to both coordinator and worker pods.
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
            keyPrefix="securityContext"
            value={podSecurityContext}
            onChange={setPodSecurityContext}
          />
          <SecurityContextEditor
            idPrefix="ps-ctrsc"
            rawId="pod.containerSecurityContext.raw"
            title="Container security context"
            keyPrefix="containerSecurityContext"
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
            label="deployment.strategy.type"
            mono
            htmlFor="ps-strategy"
            details="Deployment rollout strategy, applied to the coordinator and worker."
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
                label="deployment.strategy.rollingUpdate.maxSurge"
                mono
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
                label="deployment.strategy.rollingUpdate.maxUnavailable"
                mono
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
            label="deployment.revisionHistoryLimit"
            mono
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
            label="deployment.progressDeadlineSeconds"
            mono
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
          <Label className="font-mono text-xs">deployment.annotations</Label>
          <KeyValueEditor
            rows={deploymentAnnotations}
            onChange={setDeploymentAnnotations}
            keyPlaceholder="team"
            valuePlaceholder="data"
            addLabel="Add annotation"
          />
        </div>
      </ConfigSection>

      <ConfigSection title="Pod options">
        <ConfigTable>
          <ConfigTableRow
            label="shareProcessNamespace"
            mono
            htmlFor="ps-sharepid"
            details="Let containers in the pod share a single PID namespace."
          >
            <Switch
              id="ps-sharepid"
              checked={shareProcessNamespace}
              onCheckedChange={setShareProcessNamespace}
            />
          </ConfigTableRow>
        </ConfigTable>
      </ConfigSection>
    </div>
  );
}
