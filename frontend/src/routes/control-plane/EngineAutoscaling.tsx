import type { Dispatch, SetStateAction } from "react";
import { Plus, Trash2 } from "lucide-react";

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
import { ConfigSection } from "./config/ConfigSection";
import { FieldLabel } from "./config/FieldLabel";
import { RawJsonField } from "./config/RawJsonField";
import {
  type AutoscaleMode,
  type AutoscaleTriggerForm,
  type HpaForm,
  type KedaForm,
  emptyTriggerForm,
} from "./useEngineConfigForm";

// ── raw-JSON text ⇄ parsed bridge ─────────────────────────────────────────────
// The autoscaling form model stores raw overrides as JSON *text* (the payload
// builder parses them). The shared `RawJsonField` works in *parsed* values, so
// convert at the boundary: seed the field from parsed text and store the
// re-serialized parsed value back — the built payload stays byte-identical.

/** Parse raw-override text; undefined when blank or invalid. */
function textToRaw(s: string): unknown {
  const t = s.trim();
  if (t === "") return undefined;
  try {
    return JSON.parse(t);
  } catch {
    return undefined;
  }
}

/** Serialize a parsed raw override back to editable JSON text (or ""). */
function rawToText(v: unknown): string {
  return v === undefined || v === null ? "" : JSON.stringify(v, null, 2);
}

// ── HPA editor ───────────────────────────────────────────────────────────────

function HpaEditor({
  value,
  onChange,
}: {
  value: HpaForm;
  onChange: Dispatch<SetStateAction<HpaForm>>;
}) {
  const set = <K extends keyof HpaForm>(k: K, v: HpaForm[K]) =>
    onChange((prev) => ({ ...prev, [k]: v }));

  return (
    <ConfigSection
      title="HorizontalPodAutoscaler"
      description="Scale the worker Deployment on CPU/memory targets via a native Kubernetes HPA."
      docsUrl="https://kubernetes.io/docs/tasks/run-application/horizontal-pod-autoscale/"
    >
      <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
        <div className="space-y-1.5">
          <FieldLabel
            htmlFor="as-hpa-max"
            label="Max replicas"
            help="Upper bound on worker pods the HPA may scale to."
          />
          <Input
            id="as-hpa-max"
            type="number"
            value={value.maxReplicas}
            placeholder="10"
            onChange={(e) => set("maxReplicas", e.target.value)}
          />
        </div>
        <div className="space-y-1.5">
          <FieldLabel
            htmlFor="as-hpa-cpu"
            label="Target CPU %"
            help="Target average CPU utilization; the HPA adds pods above this."
          />
          <Input
            id="as-hpa-cpu"
            type="number"
            value={value.targetCpuPercent}
            placeholder="70"
            onChange={(e) => set("targetCpuPercent", e.target.value)}
          />
        </div>
        <div className="space-y-1.5">
          <FieldLabel
            htmlFor="as-hpa-mem"
            label="Target memory %"
            help="Target average memory utilization; the HPA adds pods above this."
          />
          <Input
            id="as-hpa-mem"
            type="number"
            value={value.targetMemoryPercent}
            placeholder="80"
            onChange={(e) => set("targetMemoryPercent", e.target.value)}
          />
        </div>
      </div>
      <RawJsonField
        id="autoscaling.hpa.behavior"
        label="Scaling behavior (raw JSON, merged into spec.behavior)"
        value={textToRaw(value.behaviorRaw)}
        onChange={(v) => set("behaviorRaw", rawToText(v))}
        placeholder='{"scaleDown": {"stabilizationWindowSeconds": 300}}'
      />
    </ConfigSection>
  );
}

// ── KEDA trigger row ─────────────────────────────────────────────────────────

const TRIGGER_KINDS = ["cron", "prometheus", "raw"];

function TriggerRow({
  idPrefix,
  value,
  onChange,
  onRemove,
}: {
  idPrefix: string;
  value: AutoscaleTriggerForm;
  onChange: (next: AutoscaleTriggerForm) => void;
  onRemove: () => void;
}) {
  const setCron = <K extends keyof AutoscaleTriggerForm["cron"]>(
    k: K,
    v: AutoscaleTriggerForm["cron"][K],
  ) => onChange({ ...value, cron: { ...value.cron, [k]: v } });
  const setProm = <K extends keyof AutoscaleTriggerForm["prometheus"]>(
    k: K,
    v: AutoscaleTriggerForm["prometheus"][K],
  ) => onChange({ ...value, prometheus: { ...value.prometheus, [k]: v } });

  return (
    <div className="space-y-3 rounded-md border p-3">
      <div className="flex items-start gap-2">
        <div className="flex-1 space-y-1.5">
          <FieldLabel
            htmlFor={`${idPrefix}-kind`}
            label="Trigger kind"
            help="Which KEDA scaler drives this trigger."
          />
          <Select
            value={value.kind}
            onValueChange={(v) => onChange({ ...value, kind: v })}
          >
            <SelectTrigger id={`${idPrefix}-kind`} className="w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {TRIGGER_KINDS.map((k) => (
                <SelectItem key={k} value={k}>
                  {k}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="mt-6 shrink-0 text-muted-foreground"
          onClick={onRemove}
          aria-label="Remove trigger"
        >
          <Trash2 className="h-4 w-4" />
        </Button>
      </div>

      {value.kind === "cron" && (
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          <div className="space-y-1.5">
            <FieldLabel
              htmlFor={`${idPrefix}-cron-start`}
              label="Start (cron)"
              help="Cron expression marking the start of the scaled-up window."
            />
            <Input
              id={`${idPrefix}-cron-start`}
              value={value.cron.start}
              placeholder="0 8 * * 1-5"
              onChange={(e) => setCron("start", e.target.value)}
            />
          </div>
          <div className="space-y-1.5">
            <FieldLabel
              htmlFor={`${idPrefix}-cron-end`}
              label="End (cron)"
              help="Cron expression marking the end of the scaled-up window."
            />
            <Input
              id={`${idPrefix}-cron-end`}
              value={value.cron.end}
              placeholder="0 18 * * 1-5"
              onChange={(e) => setCron("end", e.target.value)}
            />
          </div>
          <div className="space-y-1.5">
            <FieldLabel
              htmlFor={`${idPrefix}-cron-replicas`}
              label="Desired replicas"
              help="Replica count to hold during the cron window."
            />
            <Input
              id={`${idPrefix}-cron-replicas`}
              type="number"
              value={value.cron.desiredReplicas}
              placeholder="5"
              onChange={(e) => setCron("desiredReplicas", e.target.value)}
            />
          </div>
          <div className="space-y-1.5">
            <FieldLabel
              htmlFor={`${idPrefix}-cron-tz`}
              label="Timezone"
              help="IANA timezone the cron expressions are evaluated in."
            />
            <Input
              id={`${idPrefix}-cron-tz`}
              value={value.cron.timezone}
              placeholder="UTC"
              onChange={(e) => setCron("timezone", e.target.value)}
            />
          </div>
        </div>
      )}

      {value.kind === "prometheus" && (
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          <div className="space-y-1.5">
            <FieldLabel
              htmlFor={`${idPrefix}-prom-addr`}
              label="Server address"
              help="Prometheus server URL KEDA queries."
            />
            <Input
              id={`${idPrefix}-prom-addr`}
              value={value.prometheus.serverAddress}
              placeholder="http://prometheus:9090"
              onChange={(e) => setProm("serverAddress", e.target.value)}
            />
          </div>
          <div className="space-y-1.5">
            <FieldLabel
              htmlFor={`${idPrefix}-prom-metric`}
              label="Metric name"
              help="Name KEDA records the external metric under."
            />
            <Input
              id={`${idPrefix}-prom-metric`}
              value={value.prometheus.metricName}
              placeholder="trino_running_queries"
              onChange={(e) => setProm("metricName", e.target.value)}
            />
          </div>
          <div className="space-y-1.5">
            <FieldLabel
              htmlFor={`${idPrefix}-prom-query`}
              label="Query"
              help="PromQL query whose result drives scaling."
            />
            <Input
              id={`${idPrefix}-prom-query`}
              value={value.prometheus.query}
              placeholder="sum(trino_running_queries)"
              onChange={(e) => setProm("query", e.target.value)}
            />
          </div>
          <div className="space-y-1.5">
            <FieldLabel
              htmlFor={`${idPrefix}-prom-threshold`}
              label="Threshold"
              help="Target value per replica; KEDA scales to keep the query at this."
            />
            <Input
              id={`${idPrefix}-prom-threshold`}
              value={value.prometheus.threshold}
              placeholder="5"
              onChange={(e) => setProm("threshold", e.target.value)}
            />
          </div>
        </div>
      )}

      {value.kind === "raw" && (
        <RawJsonField
          id={`${idPrefix}-raw`}
          label="Raw trigger (KEDA ScaledObject trigger JSON)"
          value={textToRaw(value.rawOverride)}
          onChange={(v) => onChange({ ...value, rawOverride: rawToText(v) })}
          placeholder='{"type": "kafka", "metadata": {}}'
        />
      )}
    </div>
  );
}

// ── KEDA editor ──────────────────────────────────────────────────────────────

function KedaEditor({
  value,
  onChange,
}: {
  value: KedaForm;
  onChange: Dispatch<SetStateAction<KedaForm>>;
}) {
  const set = <K extends keyof KedaForm>(k: K, v: KedaForm[K]) =>
    onChange((prev) => ({ ...prev, [k]: v }));

  return (
    <ConfigSection
      title="KEDA ScaledObject"
      description="Event-driven autoscaling (cron / Prometheus / raw triggers). Requires the KEDA operator installed in the cluster."
      docsUrl="https://keda.sh/docs/latest/concepts/scaling-deployments/"
    >
      <div className="grid grid-cols-1 gap-4 sm:grid-cols-4">
        <div className="space-y-1.5">
          <FieldLabel
            htmlFor="as-keda-min"
            label="Min replicas"
            help="Fewest worker pods to keep; KEDA can scale to zero when 0."
          />
          <Input
            id="as-keda-min"
            type="number"
            value={value.minReplicaCount}
            placeholder="1"
            onChange={(e) => set("minReplicaCount", e.target.value)}
          />
        </div>
        <div className="space-y-1.5">
          <FieldLabel
            htmlFor="as-keda-max"
            label="Max replicas"
            help="Most worker pods KEDA may scale to."
          />
          <Input
            id="as-keda-max"
            type="number"
            value={value.maxReplicaCount}
            placeholder="10"
            onChange={(e) => set("maxReplicaCount", e.target.value)}
          />
        </div>
        <div className="space-y-1.5">
          <FieldLabel
            htmlFor="as-keda-poll"
            label="Polling interval (s)"
            help="How often KEDA evaluates the triggers, in seconds."
          />
          <Input
            id="as-keda-poll"
            type="number"
            value={value.pollingInterval}
            placeholder="30"
            onChange={(e) => set("pollingInterval", e.target.value)}
          />
        </div>
        <div className="space-y-1.5">
          <FieldLabel
            htmlFor="as-keda-cooldown"
            label="Cooldown (s)"
            help="Seconds to wait after the last trigger before scaling back down."
          />
          <Input
            id="as-keda-cooldown"
            type="number"
            value={value.cooldownPeriod}
            placeholder="300"
            onChange={(e) => set("cooldownPeriod", e.target.value)}
          />
        </div>
      </div>

      <div className="space-y-2">
        <Label className="text-sm font-medium">Triggers</Label>
        {value.triggers.map((t, i) => (
          <TriggerRow
            key={i}
            idPrefix={`as-keda-trig-${i}`}
            value={t}
            onChange={(next) =>
              set(
                "triggers",
                value.triggers.map((x, j) => (j === i ? next : x)),
              )
            }
            onRemove={() =>
              set(
                "triggers",
                value.triggers.filter((_, j) => j !== i),
              )
            }
          />
        ))}
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => set("triggers", [...value.triggers, emptyTriggerForm()])}
        >
          <Plus className="h-4 w-4" />
          Add trigger
        </Button>
      </div>

      <RawJsonField
        id="autoscaling.keda.fallback"
        label="Fallback (raw JSON, merged into spec.fallback)"
        value={textToRaw(value.fallbackRaw)}
        onChange={(v) => set("fallbackRaw", rawToText(v))}
        placeholder='{"failureThreshold": 3, "replicas": 2}'
      />
    </ConfigSection>
  );
}

// ── section ──────────────────────────────────────────────────────────────────

export interface EngineAutoscalingProps {
  mode: AutoscaleMode;
  setMode: (v: AutoscaleMode) => void;
  hpa: HpaForm;
  setHpa: Dispatch<SetStateAction<HpaForm>>;
  keda: KedaForm;
  setKeda: Dispatch<SetStateAction<KedaForm>>;
}

/**
 * "Autoscaling" config sub-tab: scale the worker Deployment with either a native
 * HorizontalPodAutoscaler (CPU/memory targets) or a KEDA ScaledObject (cron /
 * Prometheus / raw triggers). Off drops `autoscaling` from the payload entirely.
 *
 * The `RawJsonField`s here register their validity in the single
 * `ValidityProvider` the ConfigurationTab wraps around all sections, so an
 * unparseable override blocks Save without any per-section validity prop.
 */
export function EngineAutoscaling({
  mode,
  setMode,
  hpa,
  setHpa,
  keda,
  setKeda,
}: EngineAutoscalingProps) {
  return (
    <div className="space-y-4">
      <div className="max-w-xs space-y-1.5">
        <Label htmlFor="as-mode">Autoscaling</Label>
        <Select
          value={mode || "off"}
          onValueChange={(v) => setMode(v === "off" ? "" : (v as AutoscaleMode))}
        >
          <SelectTrigger id="as-mode" className="w-full">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="off">Off</SelectItem>
            <SelectItem value="hpa">HPA (HorizontalPodAutoscaler)</SelectItem>
            <SelectItem value="keda">KEDA (ScaledObject)</SelectItem>
          </SelectContent>
        </Select>
        <p className="text-xs text-muted-foreground">
          Scales the worker Deployment. Off leaves the fixed worker replica count
          in place.
        </p>
      </div>

      {mode === "hpa" && <HpaEditor value={hpa} onChange={setHpa} />}
      {mode === "keda" && <KedaEditor value={keda} onChange={setKeda} />}
    </div>
  );
}
