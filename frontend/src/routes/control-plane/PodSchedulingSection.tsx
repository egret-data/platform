import { useEffect, useRef, useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import { Plus, Trash2 } from "lucide-react";

import type {
  DeploymentStrategy,
  EnvFromSource,
  ObjectMount,
  SecurityContext,
} from "@/api/controlPlane";
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
import { Textarea } from "@/components/ui/textarea";
import { type Kv, KeyValueEditor } from "./EngineResourceGroups";

// ── raw-JSON helpers (shared with Task 8's per-role editors) ─────────────────

/** Stringify a parsed raw-override value to editable JSON text (or ""). */
function stringifyRaw(v: unknown): string {
  return v === undefined || v === null ? "" : JSON.stringify(v, null, 2);
}

/**
 * Validate raw-JSON override text. Returns an error message when the (non-blank)
 * text does not parse, or `null` when it is blank or valid. Exported for reuse
 * by Task 8's per-role affinity / mini-container raw-override textareas.
 */
export function validateRawJson(text: string): string | null {
  const t = text.trim();
  if (t === "") return null;
  try {
    JSON.parse(t);
    return null;
  } catch (e) {
    return (e as Error).message || "Invalid JSON";
  }
}

/** Parse a text field to a number, or undefined when blank/invalid. */
function numOrUndef(s: string): number | undefined {
  const t = s.trim();
  if (t === "") return undefined;
  const n = Number(t);
  return Number.isFinite(n) ? n : undefined;
}

const SELECT_NONE = "__default__";

// ── raw-JSON override textarea (validate-on-blur) ────────────────────────────

/**
 * A raw-JSON `<Textarea>` that holds its text locally and, on blur, either
 * commits the parsed object upstream (valid / blank) or reports an inline error
 * (invalid). The upstream `value` is the already-parsed object; when it changes
 * externally (a server re-sync) the text is re-synced.
 */
function RawJsonOverride({
  id,
  label,
  value,
  onCommit,
  onError,
}: {
  id: string;
  label: string;
  value: unknown;
  onCommit: (parsed: unknown) => void;
  onError: (err: string | null) => void;
}) {
  const [text, setText] = useState(() => stringifyRaw(value));
  const [err, setErr] = useState<string | null>(null);
  const lastValue = useRef(value);

  useEffect(() => {
    if (value !== lastValue.current) {
      lastValue.current = value;
      setText(stringifyRaw(value));
      setErr(null);
      onError(null);
    }
  }, [value, onError]);

  const handleBlur = () => {
    const e = validateRawJson(text);
    setErr(e);
    onError(e);
    if (!e) {
      const parsed = text.trim() === "" ? undefined : JSON.parse(text);
      lastValue.current = parsed;
      onCommit(parsed);
    }
  };

  return (
    <div className="space-y-1.5">
      <Label htmlFor={id} className="text-xs">
        {label}
      </Label>
      <Textarea
        id={id}
        value={text}
        rows={4}
        className="font-mono text-xs"
        placeholder='{"readOnlyRootFilesystem": true}'
        onChange={(e) => setText(e.target.value)}
        onBlur={handleBlur}
      />
      {err && <p className="text-xs text-destructive">Invalid JSON: {err}</p>}
    </div>
  );
}

// ── security-context editor (pod + container) ────────────────────────────────

const SC_NUM_FIELDS: { key: "runAsUser" | "runAsGroup" | "fsGroup"; label: string }[] =
  [
    { key: "runAsUser", label: "runAsUser" },
    { key: "runAsGroup", label: "runAsGroup" },
    { key: "fsGroup", label: "fsGroup" },
  ];

function SecurityContextEditor({
  idPrefix,
  title,
  value,
  onChange,
  onRawError,
}: {
  idPrefix: string;
  title: string;
  value: SecurityContext;
  onChange: Dispatch<SetStateAction<SecurityContext>>;
  onRawError: (err: string | null) => void;
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
      <RawJsonOverride
        id={`${idPrefix}-raw`}
        label="Raw override (merged on top)"
        value={value.rawOverride}
        onCommit={(parsed) => setField("rawOverride", parsed)}
        onError={onRawError}
      />
    </div>
  );
}

// ── object-mount rows (config/secret mounts) ─────────────────────────────────

function ObjectMountRows({
  rows,
  onChange,
  namePlaceholder,
}: {
  rows: ObjectMount[];
  onChange: Dispatch<SetStateAction<ObjectMount[]>>;
  namePlaceholder: string;
}) {
  const set = (i: number, next: ObjectMount) =>
    onChange(rows.map((r, j) => (j === i ? next : r)));

  return (
    <div className="space-y-2">
      {rows.map((m, i) => (
        <div key={i} className="flex items-start gap-2">
          <div className="grid flex-1 grid-cols-2 gap-2 sm:grid-cols-4">
            <Input
              value={m.name}
              placeholder={namePlaceholder}
              onChange={(e) => set(i, { ...m, name: e.target.value })}
            />
            <Input
              value={m.mountPath}
              placeholder="/etc/extra"
              onChange={(e) => set(i, { ...m, mountPath: e.target.value })}
            />
            <Input
              value={m.subPath ?? ""}
              placeholder="subPath (optional)"
              onChange={(e) =>
                set(i, { ...m, subPath: e.target.value || undefined })
              }
            />
            <Input
              type="number"
              value={m.defaultMode?.toString() ?? ""}
              placeholder="defaultMode"
              onChange={(e) =>
                set(i, { ...m, defaultMode: numOrUndef(e.target.value) })
              }
            />
          </div>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="shrink-0 text-muted-foreground"
            onClick={() => onChange(rows.filter((_, j) => j !== i))}
            aria-label="Remove mount"
          >
            <Trash2 className="h-4 w-4" />
          </Button>
        </div>
      ))}
      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={() => onChange([...rows, { name: "", mountPath: "" }])}
      >
        <Plus className="h-4 w-4" />
        Add mount
      </Button>
    </div>
  );
}

// ── section props ────────────────────────────────────────────────────────────

export interface PodSchedulingSectionProps {
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
  envFrom: EnvFromSource[];
  setEnvFrom: Dispatch<SetStateAction<EnvFromSource[]>>;
  configMounts: ObjectMount[];
  setConfigMounts: Dispatch<SetStateAction<ObjectMount[]>>;
  secretMounts: ObjectMount[];
  setSecretMounts: Dispatch<SetStateAction<ObjectMount[]>>;
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
  /** Reports whether every raw-JSON override parses (Task 9 blocks Save on false). */
  onValidityChange: (valid: boolean) => void;
}

/**
 * Cluster-wide "Pod & Scheduling" config: pod-shape knobs applied to both the
 * coordinator & worker Deployments. (Per-role placement + init/sidecar
 * mini-containers are added by Task 8; the sub-tab is wired in Task 9.)
 */
export function PodSchedulingSection(props: PodSchedulingSectionProps) {
  const {
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
    envFrom,
    setEnvFrom,
    configMounts,
    setConfigMounts,
    secretMounts,
    setSecretMounts,
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
    onValidityChange,
  } = props;

  // Aggregate raw-JSON validity across the pod + container override textareas.
  const [rawErrors, setRawErrors] = useState<Record<string, string | null>>({});
  const setRawError = (key: string) => (err: string | null) =>
    setRawErrors((prev) => (prev[key] === err ? prev : { ...prev, [key]: err }));

  useEffect(() => {
    onValidityChange(!Object.values(rawErrors).some(Boolean));
  }, [rawErrors, onValidityChange]);

  const setEnvFromRow = (i: number, next: EnvFromSource) =>
    setEnvFrom(envFrom.map((r, j) => (j === i ? next : r)));

  return (
    <div className="space-y-6">
      {/* Service account + image pull */}
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

      {/* Security contexts */}
      <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
        <SecurityContextEditor
          idPrefix="ps-podsc"
          title="Pod security context"
          value={podSecurityContext}
          onChange={setPodSecurityContext}
          onRawError={setRawError("podSc")}
        />
        <SecurityContextEditor
          idPrefix="ps-ctrsc"
          title="Container security context"
          value={containerSecurityContext}
          onChange={setContainerSecurityContext}
          onRawError={setRawError("ctrSc")}
        />
      </div>

      {/* envFrom */}
      <div className="space-y-2">
        <Label>Env from (ConfigMap / Secret)</Label>
        {envFrom.map((e, i) => (
          <div key={i} className="flex items-center gap-2">
            <Input
              value={e.configMapRef ?? ""}
              placeholder="configMapRef"
              onChange={(ev) =>
                setEnvFromRow(i, { ...e, configMapRef: ev.target.value || undefined })
              }
            />
            <Input
              value={e.secretRef ?? ""}
              placeholder="secretRef"
              onChange={(ev) =>
                setEnvFromRow(i, { ...e, secretRef: ev.target.value || undefined })
              }
            />
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="shrink-0 text-muted-foreground"
              onClick={() => setEnvFrom(envFrom.filter((_, j) => j !== i))}
              aria-label="Remove envFrom"
            >
              <Trash2 className="h-4 w-4" />
            </Button>
          </div>
        ))}
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => setEnvFrom([...envFrom, {}])}
        >
          <Plus className="h-4 w-4" />
          Add envFrom
        </Button>
      </div>

      {/* Mounts */}
      <div className="space-y-2">
        <Label>ConfigMap mounts</Label>
        <ObjectMountRows
          rows={configMounts}
          onChange={setConfigMounts}
          namePlaceholder="configMap name"
        />
      </div>
      <div className="space-y-2">
        <Label>Secret mounts</Label>
        <ObjectMountRows
          rows={secretMounts}
          onChange={setSecretMounts}
          namePlaceholder="secret name"
        />
      </div>

      {/* Deployment strategy + rollout knobs */}
      <div className="space-y-3 rounded-md border p-3">
        <Label className="text-xs font-medium">Deployment</Label>
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

      {/* Deployment annotations */}
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

      {/* shareProcessNamespace */}
      <div className="flex items-center justify-between gap-4 rounded-md border p-3">
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
    </div>
  );
}
