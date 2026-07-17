import { useState } from "react";
import { useSearchParams } from "react-router-dom";

import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { SaveBar } from "./EngineDetail";
import { StarRocksSecuritySection } from "./StarRocksSecuritySection";
import { KeyedPropertyList } from "./config/KeyedPropertyList";
import { RawJsonField } from "./config/RawJsonField";
import { ValidityProvider } from "./config/validityRegistry";
import {
  type AutoscalingForm,
  type RoleResourcesForm,
  type StarRocksConfigForm,
  type StarRocksPod,
  DEFAULT_SR_CN_IMAGE,
  DEFAULT_SR_FE_IMAGE,
} from "./useStarRocksConfigForm";

// Left-nav sections for the StarRocks Settings tab. Storage is read-only
// (configured from the warehouse at creation), the rest are editable; Security &
// Delegation is folded in as the LAST item, sharing the single form + Save.
const NAV_ITEMS: { key: string; label: string }[] = [
  { key: "image", label: "Image" },
  { key: "compute", label: "Compute & Resources" },
  { key: "fe-config", label: "FE config" },
  { key: "cn-config", label: "CN config" },
  { key: "autoscaling", label: "Autoscaling" },
  { key: "pod", label: "Pod & Containers" },
  { key: "storage", label: "Storage" },
  { key: "security", label: "Security & Delegation" },
];

/** True when a role-resources form carries at least one non-blank field. */
function resourcesSet(r: RoleResourcesForm): boolean {
  return [r.cpuRequest, r.cpuLimit, r.memRequest, r.memLimit].some(
    (v) => v.trim() !== "",
  );
}

/** Count the set fields in a StarRocks pod override block (drives its badge). */
function podFieldCount(pod: StarRocksPod | undefined): number {
  if (!pod) return 0;
  const nonEmpty = (v: unknown): boolean => {
    if (v == null) return false;
    if (typeof v === "string") return v.trim() !== "";
    if (Array.isArray(v)) return v.length > 0;
    if (typeof v === "object") return Object.keys(v as object).length > 0;
    return true;
  };
  return [
    pod.serviceAccount,
    pod.nodeSelector,
    pod.annotations,
    pod.podLabels,
    pod.imagePullSecrets,
    pod.env,
    pod.tolerations,
    pod.affinity,
    pod.initContainers,
    pod.sidecars,
  ].filter(nonEmpty).length;
}

/**
 * Per-section non-default field count for the StarRocks nav badges (parity with
 * the Trino `sectionCounts`). Only sections with a meaningful "configured" signal
 * are counted; image/compute/storage always carry seeded values, so they stay
 * unbadged.
 */
function srSectionCounts(cfg: StarRocksConfigForm): Record<string, number> {
  return {
    // Compute (FE/CN replicas) always carries seeded values so it's unbadged;
    // the merged section's badge therefore reflects only the set resources.
    compute:
      (resourcesSet(cfg.feResources) ? 1 : 0) +
      (resourcesSet(cfg.cnResources) ? 1 : 0),
    "fe-config": Object.keys(cfg.feConfig).length,
    "cn-config": Object.keys(cfg.cnConfig).length,
    autoscaling: cfg.autoscaling.enabled ? 1 : 0,
    pod: podFieldCount(cfg.pod),
    security: cfg.delegationMode !== "none" ? 1 : 0,
  };
}

/**
 * Control Plane → Engine detail → Settings (StarRocks).
 *
 * StarRocks-shaped mirror of the Trino `ConfigurationTab` shell: a `Card` with a
 * left-nav of sections + a content pane + the shared `SaveBar`. Edits FE/CN
 * image, replicas, and per-role resources; storage is read-only; the
 * `StarRocksSecuritySection` (delegation + TLS) is folded in as the last nav
 * item. Takes the `useStarRocksConfigForm` instance from `EngineTabs`, so the
 * single Save covers config + security.
 */
export function StarRocksConfigTab({ cfg }: { cfg: StarRocksConfigForm }) {
  // Goes true when any raw-JSON pod field (tolerations / affinity / env /
  // init / sidecars) fails to parse; combined with the autoscaling gate to
  // block Save on a bad payload. Mirrors Trino's `ConfigurationTab`.
  const [podRawInvalid, setPodRawInvalid] = useState(false);
  const [sp, setSp] = useSearchParams();
  // "resources" was folded into "compute" (one "Compute & Resources" section);
  // alias the legacy deep-link so old ?section=resources URLs still resolve.
  const rawSection = sp.get("section");
  const section = rawSection === "resources" ? "compute" : (rawSection ?? "image");
  const setSection = (v: string) =>
    setSp(
      (prev) => {
        const n = new URLSearchParams(prev);
        n.set("section", v);
        return n;
      },
      { replace: true },
    );

  // Non-default field count per nav item (drives the badges — Trino parity).
  const counts = srSectionCounts(cfg);
  const countFor = (key: string): number => counts[key] ?? 0;

  return (
    <Card>
      <CardHeader>
        <div className="space-y-1.5">
          <CardTitle className="text-lg">Settings</CardTitle>
          <CardDescription>
            FE/CN image, replicas, and compute resources plus per-user delegation
            and transport security. Saving re-provisions the engine.
          </CardDescription>
        </div>
      </CardHeader>
      <CardContent>
        <div className="space-y-4">
          <div className="flex flex-col gap-4 md:flex-row">
            <nav className="shrink-0 space-y-1 md:w-48">
              {NAV_ITEMS.map((item) => {
                const count = countFor(item.key);
                const active = section === item.key;
                return (
                  <button
                    key={item.key}
                    type="button"
                    onClick={() => setSection(item.key)}
                    className={`flex w-full items-center justify-between rounded-md px-3 py-2 text-left text-sm transition-colors hover:bg-muted ${
                      active ? "bg-muted font-medium" : "text-muted-foreground"
                    }`}
                  >
                    <span>{item.label}</span>
                    {count > 0 && (
                      <Badge variant="secondary" className="ml-2">
                        {count}
                      </Badge>
                    )}
                  </button>
                );
              })}
            </nav>

            <ValidityProvider onAnyInvalidChange={setPodRawInvalid}>
              <div className="min-w-0 flex-1">
                {section === "image" && <ImageSection cfg={cfg} />}
                {section === "compute" && (
                  <div className="space-y-8">
                    <ComputeSection cfg={cfg} />
                    <ResourcesSection cfg={cfg} />
                  </div>
                )}
                {section === "fe-config" && <FeConfigSection cfg={cfg} />}
                {section === "cn-config" && <CnConfigSection cfg={cfg} />}
                {section === "autoscaling" && <AutoscalingSection cfg={cfg} />}
                {section === "pod" && <PodContainersSection cfg={cfg} />}
                {section === "storage" && <StorageSection cfg={cfg} />}
                {section === "security" && <StarRocksSecuritySection cfg={cfg} />}
              </div>
            </ValidityProvider>
          </div>

          <SaveBar
            cfg={cfg}
            disabled={cfg.autoscalingInvalid || podRawInvalid}
          />
        </div>
      </CardContent>
    </Card>
  );
}

function ImageSection({ cfg }: { cfg: StarRocksConfigForm }) {
  return (
    <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
      <div className="space-y-1.5">
        <Label htmlFor="sr-fe-image">FE image</Label>
        <Input
          id="sr-fe-image"
          value={cfg.feImage}
          onChange={(e) => cfg.setFeImage(e.target.value)}
          placeholder={DEFAULT_SR_FE_IMAGE}
        />
        <p className="text-xs text-muted-foreground">
          Frontend (metadata) container image.
        </p>
      </div>
      <div className="space-y-1.5">
        <Label htmlFor="sr-cn-image">CN image</Label>
        <Input
          id="sr-cn-image"
          value={cfg.cnImage}
          onChange={(e) => cfg.setCnImage(e.target.value)}
          placeholder={DEFAULT_SR_CN_IMAGE}
        />
        <p className="text-xs text-muted-foreground">
          Compute node container image.
        </p>
      </div>
    </div>
  );
}

function ComputeSection({ cfg }: { cfg: StarRocksConfigForm }) {
  return (
    <div className="space-y-3">
      <div className="space-y-0.5">
        <p className="text-sm font-medium">Replicas</p>
        <p className="text-xs text-muted-foreground">
          Fixed pod counts per role (CN autoscaling, when enabled, overrides the
          CN count).
        </p>
      </div>
      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
        <div className="space-y-1.5">
          <Label htmlFor="sr-fe-replicas">FE replicas</Label>
          <Input
            id="sr-fe-replicas"
            type="number"
            min={1}
            value={cfg.feReplicas}
            onChange={(e) => cfg.setFeReplicas(e.target.value)}
          />
        </div>
        <div className="space-y-1.5">
          <Label htmlFor="sr-cn-replicas">CN replicas</Label>
          <Input
            id="sr-cn-replicas"
            type="number"
            min={1}
            value={cfg.cnReplicas}
            onChange={(e) => cfg.setCnReplicas(e.target.value)}
          />
        </div>
      </div>
    </div>
  );
}

/** One role's CPU/memory request+limit inputs. */
function ResourceFields({
  role,
  prefix,
  value,
  onChange,
}: {
  role: string;
  prefix: string;
  value: RoleResourcesForm;
  onChange: (v: RoleResourcesForm) => void;
}) {
  const field = (
    key: keyof RoleResourcesForm,
    label: string,
    placeholder: string,
  ) => (
    <div className="space-y-1.5">
      <Label htmlFor={`${prefix}-${key}`}>{`${role} ${label}`}</Label>
      <Input
        id={`${prefix}-${key}`}
        value={value[key]}
        onChange={(e) => onChange({ ...value, [key]: e.target.value })}
        placeholder={placeholder}
      />
    </div>
  );
  return (
    <div className="space-y-3">
      <p className="text-sm font-medium">{role}</p>
      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
        {field("cpuRequest", "CPU request", "1")}
        {field("cpuLimit", "CPU limit", "2")}
        {field("memRequest", "memory request", "4Gi")}
        {field("memLimit", "memory limit", "8Gi")}
      </div>
    </div>
  );
}

function ResourcesSection({ cfg }: { cfg: StarRocksConfigForm }) {
  return (
    <div className="space-y-6">
      <p className="text-sm font-medium">Resources</p>
      <ResourceFields
        role="FE"
        prefix="sr-fe-res"
        value={cfg.feResources}
        onChange={cfg.setFeResources}
      />
      <ResourceFields
        role="CN"
        prefix="sr-cn-res"
        value={cfg.cnResources}
        onChange={cfg.setCnResources}
      />
      <p className="text-xs text-muted-foreground">
        Set CPU / memory requests and limits per role as Kubernetes resource
        strings (e.g. <code className="font-mono">500m</code>,{" "}
        <code className="font-mono">4Gi</code>). Leaving a field blank keeps the
        engine&apos;s current sizing unchanged.
      </p>
    </div>
  );
}

/** A keyed-property editor over a `Record<string,string>` config map (fe.conf or
 * cn.conf). No Trino catalog — every key is a free-text custom key. Blanking a
 * value removes the entry. */
function ConfigMapEditor({
  value,
  onChange,
}: {
  value: Record<string, string>;
  onChange: (v: Record<string, string>) => void;
}) {
  return (
    <KeyedPropertyList
      catalog={[]}
      catalogByKey={{}}
      presentKeys={Object.keys(value)}
      getValue={(k) => value[k] ?? ""}
      setValue={(k, v) => {
        const next = { ...value };
        if (v === "") delete next[k];
        else next[k] = v;
        onChange(next);
      }}
    />
  );
}

function FeConfigSection({ cfg }: { cfg: StarRocksConfigForm }) {
  return (
    <div className="space-y-3">
      <ConfigMapEditor value={cfg.feConfig} onChange={cfg.setFeConfig} />
      <p className="text-xs text-muted-foreground">
        Overrides merged into the generated <code className="font-mono">fe.conf</code>{" "}
        (e.g. <code className="font-mono">JAVA_OPTS</code>,{" "}
        <code className="font-mono">sys_log_level</code>,{" "}
        <code className="font-mono">query_timeout</code>). egret-managed keys
        (storage, run_mode, ssl, ports) can&apos;t be overridden.
      </p>
    </div>
  );
}

function CnConfigSection({ cfg }: { cfg: StarRocksConfigForm }) {
  return (
    <div className="space-y-3">
      <ConfigMapEditor value={cfg.cnConfig} onChange={cfg.setCnConfig} />
      <p className="text-xs text-muted-foreground">
        Overrides merged into the generated <code className="font-mono">cn.conf</code>.
        egret-managed keys (storage, run_mode, ssl, ports) can&apos;t be overridden.
      </p>
    </div>
  );
}

function AutoscalingSection({ cfg }: { cfg: StarRocksConfigForm }) {
  const a = cfg.autoscaling;
  const set = (patch: Partial<AutoscalingForm>) =>
    cfg.setAutoscaling({ ...a, ...patch });
  const maxMissing = a.enabled && a.maxReplicas.trim() === "";
  const cpuMissing = a.enabled && a.targetCpuPercent.trim() === "";
  return (
    <div className="space-y-4">
      <div className="flex items-center gap-3">
        <Switch
          id="sr-autoscaling-enabled"
          checked={a.enabled}
          onCheckedChange={(enabled) => set({ enabled })}
          aria-label="Enable CN autoscaling"
        />
        <Label htmlFor="sr-autoscaling-enabled">Enable CN autoscaling</Label>
      </div>
      {a.enabled && (
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
          <div className="space-y-1.5">
            <Label htmlFor="sr-as-min">Min replicas</Label>
            <Input
              id="sr-as-min"
              type="number"
              min={1}
              value={a.minReplicas}
              onChange={(e) => set({ minReplicas: e.target.value })}
              placeholder="optional"
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="sr-as-max">Max replicas</Label>
            <Input
              id="sr-as-max"
              type="number"
              min={1}
              value={a.maxReplicas}
              onChange={(e) => set({ maxReplicas: e.target.value })}
            />
            {maxMissing && (
              <p className="text-xs text-destructive">Max replicas is required.</p>
            )}
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="sr-as-cpu">Target CPU %</Label>
            <Input
              id="sr-as-cpu"
              type="number"
              min={1}
              value={a.targetCpuPercent}
              onChange={(e) => set({ targetCpuPercent: e.target.value })}
            />
            {cpuMissing && (
              <p className="text-xs text-destructive">Target CPU % is required.</p>
            )}
          </div>
        </div>
      )}
      <p className="text-xs text-muted-foreground">
        Enabling hands the CN replica count to a Kubernetes HorizontalPodAutoscaler
        (CPU-target based); the fixed CN replica count above no longer applies.
      </p>
    </div>
  );
}

/** A comma-separated string-list input (used for imagePullSecrets). Holds its
 * own editable text (seeded once) so intermediate commas/spaces survive typing;
 * commits the compacted `string[]` (undefined when empty) upstream. */
function CommaListInput({
  id,
  value,
  onChange,
  placeholder,
}: {
  id: string;
  value: string[] | undefined;
  onChange: (v: string[] | undefined) => void;
  placeholder?: string;
}) {
  const [text, setText] = useState(() => (value ?? []).join(", "));
  return (
    <Input
      id={id}
      value={text}
      placeholder={placeholder}
      onChange={(e) => {
        setText(e.target.value);
        const arr = e.target.value
          .split(",")
          .map((s) => s.trim())
          .filter((s) => s !== "");
        onChange(arr.length ? arr : undefined);
      }}
    />
  );
}

/**
 * Pod & Containers section: the cluster-wide pod/container overrides the backend
 * renders onto BOTH the FE and CN roles. serviceAccount is a text input;
 * nodeSelector / annotations / podLabels are keyed-property (key-value) editors;
 * imagePullSecrets is a comma-separated list; env / tolerations / affinity /
 * initContainers / sidecars are raw-JSON fields (invalid JSON blocks Save via the
 * surrounding `ValidityProvider`). securityContext is intentionally NOT exposed —
 * the StarRocks operator doesn't support it. Every field maps into the hook's
 * `pod` block, which flows through the same dirty/save path as the rest of the
 * form (empty sub-fields are dropped, so a blank pod is never dirty).
 */
function PodContainersSection({ cfg }: { cfg: StarRocksConfigForm }) {
  const pod: StarRocksPod = cfg.pod ?? {};
  // Functional updater: several debounced RawJsonField editors feed the one
  // `pod` object, so each patch must merge against the LATEST state (not the
  // `pod` snapshot captured when this handler was created) or a same-macrotask
  // pair of commits would clobber each other.
  const patch = (p: Partial<StarRocksPod>) =>
    cfg.setPod((prev) => ({ ...(prev ?? {}), ...p }));

  return (
    <div className="space-y-6">
      <div className="space-y-1.5">
        <Label htmlFor="sr-pod-sa">Service account</Label>
        <Input
          id="sr-pod-sa"
          value={pod.serviceAccount ?? ""}
          onChange={(e) => patch({ serviceAccount: e.target.value || undefined })}
          placeholder="default"
        />
        <p className="text-xs text-muted-foreground">
          ServiceAccount both the FE and CN pods run as.
        </p>
      </div>

      <div className="space-y-2">
        <Label>Node selector</Label>
        <ConfigMapEditor
          value={pod.nodeSelector ?? {}}
          onChange={(v) => patch({ nodeSelector: v })}
        />
        <p className="text-xs text-muted-foreground">
          Schedule pods only onto nodes matching these labels.
        </p>
      </div>

      <div className="space-y-2">
        <Label>Annotations</Label>
        <ConfigMapEditor
          value={pod.annotations ?? {}}
          onChange={(v) => patch({ annotations: v })}
        />
      </div>

      <div className="space-y-2">
        <Label>Pod labels</Label>
        <ConfigMapEditor
          value={pod.podLabels ?? {}}
          onChange={(v) => patch({ podLabels: v })}
        />
      </div>

      <div className="space-y-1.5">
        <Label htmlFor="sr-pod-pull-secrets">Image pull secrets</Label>
        <CommaListInput
          id="sr-pod-pull-secrets"
          value={pod.imagePullSecrets}
          onChange={(v) => patch({ imagePullSecrets: v })}
          placeholder="regcred, other-secret"
        />
        <p className="text-xs text-muted-foreground">
          Comma-separated Secret names used to pull private images.
        </p>
      </div>

      <RawJsonField
        id="sr-pod-env"
        label="Env"
        value={pod.env}
        onChange={(v) => patch({ env: v })}
        placeholder='[{"name": "TZ", "value": "UTC"}]'
      />

      <RawJsonField
        id="sr-pod-tolerations"
        label="Tolerations"
        value={pod.tolerations}
        onChange={(v) => patch({ tolerations: v })}
        placeholder='[{"key": "dedicated", "operator": "Exists"}]'
      />

      <RawJsonField
        id="sr-pod-affinity"
        label="Affinity"
        value={pod.affinity}
        onChange={(v) => patch({ affinity: v })}
        placeholder='{"nodeAffinity": {}}'
      />

      <RawJsonField
        id="sr-pod-init"
        label="Init containers"
        value={pod.initContainers}
        onChange={(v) => patch({ initContainers: v })}
        placeholder='[{"name": "wait", "image": "busybox:1.36"}]'
      />

      <RawJsonField
        id="sr-pod-sidecars"
        label="Sidecars"
        value={pod.sidecars}
        onChange={(v) => patch({ sidecars: v })}
        placeholder='[{"name": "agent", "image": "agent:1.0"}]'
      />

      <p className="text-xs text-muted-foreground">
        These pod &amp; container overrides apply to both the FE and CN roles.
        Tolerations, affinity, env, init containers, and sidecars are raw
        Kubernetes JSON.
      </p>
    </div>
  );
}

function StorageSection({ cfg }: { cfg: StarRocksConfigForm }) {
  const endpoint = cfg.storedConfig?.storageEndpoint;
  const path = cfg.storedConfig?.storagePath;
  if (endpoint || path) {
    return (
      <dl className="space-y-3 text-sm">
        {endpoint && (
          <div className="space-y-0.5">
            <dt className="text-muted-foreground">Endpoint</dt>
            <dd className="font-mono text-xs">{endpoint}</dd>
          </div>
        )}
        {path && (
          <div className="space-y-0.5">
            <dt className="text-muted-foreground">Path</dt>
            <dd className="font-mono text-xs">{path}</dd>
          </div>
        )}
        <p className="text-xs text-muted-foreground">
          Object storage is configured from the warehouse at creation and
          isn&apos;t editable here.
        </p>
      </dl>
    );
  }
  return (
    <p className="rounded-md border bg-muted/40 px-4 py-6 text-sm text-muted-foreground">
      Object storage is configured from the warehouse at creation and isn&apos;t
      editable here.
    </p>
  );
}
