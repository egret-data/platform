import type { Dispatch, SetStateAction } from "react";
import { Plus, Trash2 } from "lucide-react";

import type { DeploymentStrategy, SecurityContext } from "@/api/controlPlane";
import { Button } from "@/components/ui/button";
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
    <div className="space-y-3 rounded-md border p-3">
      <Label className="text-xs font-medium">{title}</Label>
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
        {SC_NUM_FIELDS.map((f) => (
          <div key={f.key} className="space-y-1.5">
            <Label htmlFor={`${idPrefix}-${f.key}`} className="text-xs">
              {f.label}
            </Label>
            <Input
              id={`${idPrefix}-${f.key}`}
              type="number"
              value={value[f.key]?.toString() ?? ""}
              placeholder="1000"
              onChange={(e) => setField(f.key, numOrUndef(e.target.value))}
            />
          </div>
        ))}
      </div>
      <div className="flex items-center justify-between gap-4">
        <Label htmlFor={`${idPrefix}-nonroot`} className="text-xs">
          runAsNonRoot
        </Label>
        <Switch
          id={`${idPrefix}-nonroot`}
          checked={value.runAsNonRoot ?? false}
          onCheckedChange={(v) => setField("runAsNonRoot", v)}
        />
      </div>
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
  imagePullSecrets: string[];
  setImagePullSecrets: Dispatch<SetStateAction<string[]>>;
  imagePullPolicy: string;
  setImagePullPolicy: (v: string) => void;
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
 * worker Deployments — service account, image pull, pod & container security
 * contexts, the Deployment rollout strategy, and shared-PID namespace.
 */
export function PodSection({
  serviceAccount,
  setServiceAccount,
  imagePullSecrets,
  setImagePullSecrets,
  imagePullPolicy,
  setImagePullPolicy,
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
        title="Service account & image pull"
        description="Applied to both the coordinator and worker pods."
      >
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
          <div className="space-y-1.5">
            <Label htmlFor="ps-sa">Service account</Label>
            <Input
              id="ps-sa"
              value={serviceAccount}
              placeholder="default"
              onChange={(e) => setServiceAccount(e.target.value)}
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="ps-pullpolicy">Image pull policy</Label>
            <Select
              value={imagePullPolicy || SELECT_NONE}
              onValueChange={(v) => setImagePullPolicy(v === SELECT_NONE ? "" : v)}
            >
              <SelectTrigger id="ps-pullpolicy" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={SELECT_NONE}>(default)</SelectItem>
                <SelectItem value="Always">Always</SelectItem>
                <SelectItem value="IfNotPresent">IfNotPresent</SelectItem>
                <SelectItem value="Never">Never</SelectItem>
              </SelectContent>
            </Select>
          </div>
        </div>

        <div className="space-y-2">
          <Label>Image pull secrets</Label>
          <p className="text-xs text-muted-foreground">
            Names of Kubernetes <code>docker-registry</code> secrets in the engine
            namespace.
          </p>
          {imagePullSecrets.map((s, i) => (
            <div key={i} className="flex items-center gap-2">
              <Input
                value={s}
                placeholder="regcred"
                onChange={(e) =>
                  setImagePullSecrets(
                    imagePullSecrets.map((v, j) => (j === i ? e.target.value : v)),
                  )
                }
              />
              <Button
                type="button"
                variant="ghost"
                size="icon"
                className="shrink-0 text-muted-foreground"
                onClick={() =>
                  setImagePullSecrets(imagePullSecrets.filter((_, j) => j !== i))
                }
                aria-label="Remove secret"
              >
                <Trash2 className="h-4 w-4" />
              </Button>
            </div>
          ))}
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => setImagePullSecrets([...imagePullSecrets, ""])}
          >
            <Plus className="h-4 w-4" />
            Add secret
          </Button>
        </div>
      </ConfigSection>

      <ConfigSection
        title="Security contexts"
        description="Pod- and container-level securityContext, with a raw override merged on top."
      >
        <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
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
        <div className="space-y-3 rounded-md border p-3">
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
            <div className="space-y-1.5">
              <Label htmlFor="ps-strategy" className="text-xs">
                Strategy
              </Label>
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
            </div>
            {deploymentStrategy.kind === "RollingUpdate" && (
              <>
                <div className="space-y-1.5">
                  <Label htmlFor="ps-maxsurge" className="text-xs">
                    maxSurge
                  </Label>
                  <Input
                    id="ps-maxsurge"
                    value={deploymentStrategy.maxSurge ?? ""}
                    placeholder="25%"
                    onChange={(e) =>
                      setDeploymentStrategy((prev) => ({
                        ...prev,
                        maxSurge: e.target.value || undefined,
                      }))
                    }
                  />
                </div>
                <div className="space-y-1.5">
                  <Label htmlFor="ps-maxunavail" className="text-xs">
                    maxUnavailable
                  </Label>
                  <Input
                    id="ps-maxunavail"
                    value={deploymentStrategy.maxUnavailable ?? ""}
                    placeholder="25%"
                    onChange={(e) =>
                      setDeploymentStrategy((prev) => ({
                        ...prev,
                        maxUnavailable: e.target.value || undefined,
                      }))
                    }
                  />
                </div>
              </>
            )}
          </div>
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
            <div className="space-y-1.5">
              <Label htmlFor="ps-revlimit" className="text-xs">
                revisionHistoryLimit
              </Label>
              <Input
                id="ps-revlimit"
                type="number"
                value={revisionHistoryLimit}
                placeholder="10"
                onChange={(e) => setRevisionHistoryLimit(e.target.value)}
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="ps-progress" className="text-xs">
                progressDeadlineSeconds
              </Label>
              <Input
                id="ps-progress"
                type="number"
                value={progressDeadlineSeconds}
                placeholder="600"
                onChange={(e) => setProgressDeadlineSeconds(e.target.value)}
              />
            </div>
          </div>
        </div>

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

      <ConfigSection title="Pod options">
        <div className="flex items-center justify-between gap-4">
          <div className="space-y-0.5">
            <Label htmlFor="ps-sharepid">Share process namespace</Label>
            <p className="text-xs text-muted-foreground">
              Let containers in the pod share a single PID namespace.
            </p>
          </div>
          <Switch
            id="ps-sharepid"
            checked={shareProcessNamespace}
            onCheckedChange={setShareProcessNamespace}
          />
        </div>
      </ConfigSection>
    </div>
  );
}
