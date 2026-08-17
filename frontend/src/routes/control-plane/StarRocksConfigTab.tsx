import { useState } from "react";
import { useSearchParams } from "react-router-dom";

import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { SaveBar } from "./EngineDetail";
import { StarRocksSecuritySection } from "./StarRocksSecuritySection";
import { type Kv, KeyValueEditor } from "./EngineResourceGroups";
import { ConfigTable, ConfigTableRow } from "@/components/config/ConfigTable";
import { KeyedPropertyList } from "./config/KeyedPropertyList";
import { RawJsonField } from "./config/RawJsonField";
import { TolerationsEditor } from "./config/PlacementSection";
import {
  type ContainerForm,
  ContainerListEditor,
} from "./config/ContainersMountsSection";
import { ValidityProvider } from "./config/validityRegistry";
import { type TolerationForm } from "./useEngineConfigForm";
import {
  containersJsonToForms,
  envJsonToKv,
  formsToContainersJson,
  kvToEnvJson,
  rowsToTolerationsJson,
  tolerationsJsonToRows,
} from "./starrocksPodShape";
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
    // Auto ("") is the unconfigured default (omitted from the PUT body); any
    // explicit choice — including "none" — counts as configured.
    security: cfg.delegationMode !== "" ? 1 : 0,
  };
}

/**
 * Control Plane → Engine detail → Settings (StarRocks).
 *
 * StarRocks-shaped mirror of the Trino `ConfigurationTab` shell: a card-less
 * `<section>` with a left-nav of sections + a content pane + the shared
 * `SaveBar`. Edits FE/CN image, replicas, and per-role resources; storage is
 * read-only; the
 * `StarRocksSecuritySection` (delegation + TLS) is folded in as the last nav
 * item. Takes the `useStarRocksConfigForm` instance from `EngineTabs`, so the
 * single Save covers config + security.
 */
export function StarRocksConfigTab({ cfg }: { cfg: StarRocksConfigForm }) {
  // Goes true when a raw-JSON pod field fails to parse — the affinity escape or
  // any container's Advanced-JSON override (env / tolerations / init / sidecars
  // are structured now); combined with the autoscaling gate to block Save on a
  // bad payload. Mirrors Trino's `ConfigurationTab`.
  const [podRawInvalid, setPodRawInvalid] = useState(false);
  const [sp, setSp] = useSearchParams();
  // "resources" was folded into "compute" (one "Compute & Resources" section);
  // alias the legacy deep-link so old ?section=resources URLs still resolve.
  // Unknown/stale values normalize to "image" (no blank pane).
  const rawSection = sp.get("section") === "resources" ? "compute" : sp.get("section");
  const section = NAV_ITEMS.some((i) => i.key === rawSection)
    ? (rawSection as string)
    : "image";
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
    <section className="space-y-4">
      <div className="space-y-1">
        <h2 className="text-base font-semibold">Settings</h2>
        <p className="text-sm text-muted-foreground">
          FE/CN image, replicas, and compute resources plus per-user delegation
          and transport security. Saving re-provisions the engine.
        </p>
      </div>
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
    </section>
  );
}

function ImageSection({ cfg }: { cfg: StarRocksConfigForm }) {
  return (
    <ConfigTable>
      <ConfigTableRow
        label="FE image"
        htmlFor="sr-fe-image"
        details="Frontend (metadata) container image."
      >
        <Input
          id="sr-fe-image"
          className="w-full"
          value={cfg.feImage}
          onChange={(e) => cfg.setFeImage(e.target.value)}
          placeholder={DEFAULT_SR_FE_IMAGE}
        />
      </ConfigTableRow>
      <ConfigTableRow
        label="CN image"
        htmlFor="sr-cn-image"
        details="Compute node container image."
      >
        <Input
          id="sr-cn-image"
          className="w-full"
          value={cfg.cnImage}
          onChange={(e) => cfg.setCnImage(e.target.value)}
          placeholder={DEFAULT_SR_CN_IMAGE}
        />
      </ConfigTableRow>
    </ConfigTable>
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
      <ConfigTable>
        <ConfigTableRow
          label="FE replicas"
          htmlFor="sr-fe-replicas"
          details="Frontend (metadata) pod count."
        >
          <Input
            id="sr-fe-replicas"
            className="w-full"
            type="number"
            min={1}
            value={cfg.feReplicas}
            onChange={(e) => cfg.setFeReplicas(e.target.value)}
          />
        </ConfigTableRow>
        <ConfigTableRow
          label="CN replicas"
          htmlFor="sr-cn-replicas"
          details="Compute node pod count (overridden by CN autoscaling when enabled)."
        >
          <Input
            id="sr-cn-replicas"
            className="w-full"
            type="number"
            min={1}
            value={cfg.cnReplicas}
            onChange={(e) => cfg.setCnReplicas(e.target.value)}
          />
        </ConfigTableRow>
      </ConfigTable>
    </div>
  );
}

const SR_RES_FIELDS: {
  key: keyof RoleResourcesForm;
  label: string;
  placeholder: string;
  details: string;
}[] = [
  { key: "cpuRequest", label: "CPU request", placeholder: "1", details: "Kubernetes CPU request." },
  { key: "cpuLimit", label: "CPU limit", placeholder: "2", details: "Kubernetes CPU limit." },
  { key: "memRequest", label: "memory request", placeholder: "4Gi", details: "Kubernetes memory request." },
  { key: "memLimit", label: "memory limit", placeholder: "8Gi", details: "Kubernetes memory limit." },
];

/** One role's CPU/memory request+limit inputs, as a config table. */
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
  return (
    <div className="space-y-3">
      <p className="text-sm font-medium">{role}</p>
      <ConfigTable>
        {SR_RES_FIELDS.map((f) => (
          <ConfigTableRow
            key={f.key}
            label={`${role} ${f.label}`}
            htmlFor={`${prefix}-${f.key}`}
            details={f.details}
          >
            <Input
              id={`${prefix}-${f.key}`}
              className="w-full"
              value={value[f.key]}
              onChange={(e) => onChange({ ...value, [f.key]: e.target.value })}
              placeholder={f.placeholder}
            />
          </ConfigTableRow>
        ))}
      </ConfigTable>
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
        <ConfigTable>
          <ConfigTableRow
            label="Min replicas"
            htmlFor="sr-as-min"
            details="Fewest CN pods to keep (optional)."
          >
            <Input
              id="sr-as-min"
              className="w-full"
              type="number"
              min={1}
              value={a.minReplicas}
              onChange={(e) => set({ minReplicas: e.target.value })}
              placeholder="optional"
            />
          </ConfigTableRow>
          <ConfigTableRow
            label="Max replicas"
            htmlFor="sr-as-max"
            details="Most CN pods the HPA may scale to (required)."
          >
            <Input
              id="sr-as-max"
              className="w-full"
              type="number"
              min={1}
              value={a.maxReplicas}
              onChange={(e) => set({ maxReplicas: e.target.value })}
              aria-invalid={maxMissing}
            />
            {maxMissing && (
              <p className="mt-1 text-xs text-destructive">Max replicas is required.</p>
            )}
          </ConfigTableRow>
          <ConfigTableRow
            label="Target CPU %"
            htmlFor="sr-as-cpu"
            details="Target average CPU utilization that drives scaling (required)."
          >
            <Input
              id="sr-as-cpu"
              className="w-full"
              type="number"
              min={1}
              value={a.targetCpuPercent}
              onChange={(e) => set({ targetCpuPercent: e.target.value })}
              aria-invalid={cpuMissing}
            />
            {cpuMissing && (
              <p className="mt-1 text-xs text-destructive">Target CPU % is required.</p>
            )}
          </ConfigTableRow>
        </ConfigTable>
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

/** A patch merger over the hook's `pod` block (functional so debounced/commit
 * pairs never clobber each other). */
type PodPatch = (p: Partial<StarRocksPod>) => void;

/**
 * Working-state Environment editor: seeds `Kv[]` ONCE from the stored `pod.env`
 * and owns the rows locally. The lossy `kvToEnvJson` serializer drops blank-key
 * rows, so a half-typed row would vanish if it were derived fresh from `pod`
 * every render — holding the rows here keeps an in-progress row visible while
 * still committing the serialized `[{name,value}]` upstream (same pattern as
 * `CommaListInput`).
 */
function PodEnvEditor({ pod, patch }: { pod: StarRocksPod; patch: PodPatch }) {
  const [rows, setRows] = useState<Kv[]>(() => envJsonToKv(pod.env));
  return (
    <KeyValueEditor
      rows={rows}
      onChange={(next) => {
        setRows(next);
        patch({ env: kvToEnvJson(next) });
      }}
      keyPlaceholder="Variable name"
      valuePlaceholder="Variable value"
      addLabel="Add env var"
    />
  );
}

/** Working-state Tolerations editor (seeds `TolerationForm[]` once from
 * `pod.tolerations`; commits the omit-empty k8s array upstream). */
function PodTolerationsEditor({
  pod,
  patch,
}: {
  pod: StarRocksPod;
  patch: PodPatch;
}) {
  const [rows, setRows] = useState<TolerationForm[]>(() =>
    tolerationsJsonToRows(pod.tolerations),
  );
  return (
    <TolerationsEditor
      rows={rows}
      onChange={(next) => {
        setRows(next);
        patch({ tolerations: rowsToTolerationsJson(next) });
      }}
    />
  );
}

/** Working-state container-list editor (init or sidecar): seeds `ContainerForm[]`
 * once from the stored JSON and commits the re-serialized k8s array upstream. A
 * distinct `idPrefix` per list keeps each container's Advanced-JSON RawJsonField
 * id unique in the surrounding `ValidityProvider`. */
function PodContainerListEditor({
  value,
  onChange,
  idPrefix,
  addLabel,
}: {
  value: unknown;
  onChange: (json: unknown[]) => void;
  idPrefix: string;
  addLabel: string;
}) {
  const [rows, setRows] = useState<ContainerForm[]>(() =>
    containersJsonToForms(value),
  );
  return (
    <ContainerListEditor
      rows={rows}
      idPrefix={idPrefix}
      addLabel={addLabel}
      onChange={(next) => {
        setRows(next);
        onChange(formsToContainersJson(next));
      }}
    />
  );
}

/**
 * Pod & Containers section: the cluster-wide pod/container overrides the backend
 * renders onto BOTH the FE and CN roles, grouped into four sub-sections —
 * Environment, Scheduling, Containers, Identity.
 *
 * env / tolerations / initContainers / sidecars are edited through STRUCTURED
 * add-row editors (reusing the Trino config primitives via `starrocksPodShape`
 * converters), each wrapped in a working-state component so in-progress rows
 * survive the lossy serializers. Only `affinity` stays a raw-JSON field — its
 * nested nodeAffinity/podAffinity shape has no compact form — so invalid JSON
 * there (or in a container's Advanced-JSON override) still blocks Save via the
 * surrounding `ValidityProvider`. serviceAccount is a text input; nodeSelector /
 * annotations / podLabels are keyed-property editors; imagePullSecrets is a
 * comma-separated list. securityContext is intentionally NOT exposed — the
 * StarRocks operator doesn't support it. Every field maps into the hook's `pod`
 * block, which flows through the same dirty/save path as the rest of the form
 * (empty sub-fields are dropped, so a blank pod is never dirty).
 */
function PodContainersSection({ cfg }: { cfg: StarRocksConfigForm }) {
  const pod: StarRocksPod = cfg.pod ?? {};
  // Functional updater: several independent editors feed the one `pod` object,
  // so each patch must merge against the LATEST state (not the `pod` snapshot
  // captured when this handler was created) or a same-macrotask pair of commits
  // would clobber each other.
  const patch: PodPatch = (p) =>
    cfg.setPod((prev) => ({ ...(prev ?? {}), ...p }));

  return (
    <div className="space-y-8">
      {/* 1 · Environment */}
      <div className="space-y-3">
        <div className="space-y-0.5">
          <h3 className="text-sm font-medium">Environment</h3>
          <p className="text-xs text-muted-foreground">
            Environment variables set on both the FE and CN containers.
          </p>
        </div>
        <PodEnvEditor pod={pod} patch={patch} />
      </div>

      {/* 2 · Scheduling */}
      <div className="space-y-4">
        <div className="space-y-0.5">
          <h3 className="text-sm font-medium">Scheduling</h3>
          <p className="text-xs text-muted-foreground">
            Where the FE and CN pods are placed.
          </p>
        </div>

        <div className="space-y-2">
          <p className="text-sm font-medium">Node selector</p>
          <ConfigMapEditor
            value={pod.nodeSelector ?? {}}
            onChange={(v) => patch({ nodeSelector: v })}
          />
          <p className="text-xs text-muted-foreground">
            Schedule pods only onto nodes matching these labels.
          </p>
        </div>

        <div className="space-y-2">
          <p className="text-sm font-medium">Tolerations</p>
          <PodTolerationsEditor pod={pod} patch={patch} />
        </div>

        <RawJsonField
          id="sr-pod-affinity"
          label="Affinity (advanced JSON)"
          value={pod.affinity}
          onChange={(v) => patch({ affinity: v })}
          placeholder='{"nodeAffinity": {}}'
        />
        <p className="text-xs text-muted-foreground">
          The nested nodeAffinity/podAffinity shape has no compact form — edit as
          raw Kubernetes JSON.
        </p>
      </div>

      {/* 3 · Containers */}
      <div className="space-y-4">
        <div className="space-y-0.5">
          <h3 className="text-sm font-medium">Containers</h3>
          <p className="text-xs text-muted-foreground">
            Extra containers added to both the FE and CN pods.
          </p>
        </div>

        <div className="space-y-2">
          <p className="text-sm font-medium">Init containers</p>
          <p className="text-xs text-muted-foreground">
            Run to completion before the StarRocks container starts (both roles).
          </p>
          <PodContainerListEditor
            value={pod.initContainers}
            onChange={(v) => patch({ initContainers: v })}
            idPrefix="sr-init"
            addLabel="Add init container"
          />
        </div>

        <div className="space-y-2">
          <p className="text-sm font-medium">Sidecars</p>
          <p className="text-xs text-muted-foreground">
            Extra containers that run alongside StarRocks (both roles).
          </p>
          <PodContainerListEditor
            value={pod.sidecars}
            onChange={(v) => patch({ sidecars: v })}
            idPrefix="sr-sidecar"
            addLabel="Add sidecar"
          />
        </div>
      </div>

      {/* 4 · Identity */}
      <div className="space-y-4">
        <div className="space-y-0.5">
          <h3 className="text-sm font-medium">Identity</h3>
          <p className="text-xs text-muted-foreground">
            Service account, image-pull credentials, and pod metadata.
          </p>
        </div>

        <ConfigTable>
          <ConfigTableRow
            label="Service account"
            htmlFor="sr-pod-sa"
            details="ServiceAccount both the FE and CN pods run as."
          >
            <Input
              id="sr-pod-sa"
              className="w-full"
              value={pod.serviceAccount ?? ""}
              onChange={(e) =>
                patch({ serviceAccount: e.target.value || undefined })
              }
              placeholder="default"
            />
          </ConfigTableRow>
          <ConfigTableRow
            label="Image pull secrets"
            htmlFor="sr-pod-pull-secrets"
            details="Comma-separated Secret names used to pull private images."
          >
            <CommaListInput
              id="sr-pod-pull-secrets"
              value={pod.imagePullSecrets}
              onChange={(v) => patch({ imagePullSecrets: v })}
              placeholder="regcred, other-secret"
            />
          </ConfigTableRow>
        </ConfigTable>

        <div className="space-y-2">
          <p className="text-sm font-medium">Pod labels</p>
          <ConfigMapEditor
            value={pod.podLabels ?? {}}
            onChange={(v) => patch({ podLabels: v })}
          />
        </div>

        <div className="space-y-2">
          <p className="text-sm font-medium">Annotations</p>
          <ConfigMapEditor
            value={pod.annotations ?? {}}
            onChange={(v) => patch({ annotations: v })}
          />
        </div>
      </div>
    </div>
  );
}

function StorageSection({ cfg }: { cfg: StarRocksConfigForm }) {
  const endpoint = cfg.storedConfig?.storageEndpoint;
  const path = cfg.storedConfig?.storagePath;
  if (endpoint || path) {
    return (
      <div className="space-y-3">
        <ConfigTable>
          {endpoint && (
            <ConfigTableRow
              label="Endpoint"
              details="Object-store endpoint (from the warehouse; read-only)."
            >
              <span className="font-mono text-xs">{endpoint}</span>
            </ConfigTableRow>
          )}
          {path && (
            <ConfigTableRow
              label="Path"
              details="Object-store path prefix (from the warehouse; read-only)."
            >
              <span className="font-mono text-xs">{path}</span>
            </ConfigTableRow>
          )}
        </ConfigTable>
        <p className="text-xs text-muted-foreground">
          Object storage is configured from the warehouse at creation and
          isn&apos;t editable here.
        </p>
      </div>
    );
  }
  return (
    <p className="rounded-md border bg-muted/40 px-4 py-6 text-sm text-muted-foreground">
      Object storage is configured from the warehouse at creation and isn&apos;t
      editable here.
    </p>
  );
}
