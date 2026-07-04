import { useEffect } from "react";
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
import { Textarea } from "@/components/ui/textarea";
import { validateRawJson } from "./PodSchedulingSection";
import {
  type AutoscaleMode,
  type AutoscaleTriggerForm,
  type HpaForm,
  type KedaForm,
  emptyTriggerForm,
} from "./useEngineConfigForm";

// ── raw-JSON text field (text-in-form, validated on every keystroke) ─────────

/**
 * A raw-JSON `<Textarea>` whose text lives directly in the form model (mirrors
 * PodSchedulingSection's affinity override). Validity is aggregated centrally
 * by the section's `onValidityChange` effect, so no per-field error callback is
 * needed — the inline message is derived from the current text.
 */
function RawJsonField({
  id,
  label,
  value,
  onChange,
  placeholder,
}: {
  id: string;
  label: string;
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
}) {
  const err = validateRawJson(value);
  return (
    <div className="space-y-1.5">
      <Label htmlFor={id} className="text-xs">
        {label}
      </Label>
      <Textarea
        id={id}
        value={value}
        rows={4}
        className="font-mono text-xs"
        placeholder={placeholder}
        onChange={(e) => onChange(e.target.value)}
      />
      {err && <p className="text-xs text-destructive">Invalid JSON: {err}</p>}
    </div>
  );
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
    <div className="space-y-4 rounded-lg border p-4">
      <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
        <div className="space-y-1.5">
          <Label htmlFor="as-hpa-max" className="text-xs">
            maxReplicas
          </Label>
          <Input
            id="as-hpa-max"
            type="number"
            value={value.maxReplicas}
            placeholder="10"
            onChange={(e) => set("maxReplicas", e.target.value)}
          />
        </div>
        <div className="space-y-1.5">
          <Label htmlFor="as-hpa-cpu" className="text-xs">
            targetCpuPercent
          </Label>
          <Input
            id="as-hpa-cpu"
            type="number"
            value={value.targetCpuPercent}
            placeholder="70"
            onChange={(e) => set("targetCpuPercent", e.target.value)}
          />
        </div>
        <div className="space-y-1.5">
          <Label htmlFor="as-hpa-mem" className="text-xs">
            targetMemoryPercent
          </Label>
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
        id="as-hpa-behavior"
        label="Scaling behavior (raw JSON, merged into spec.behavior)"
        value={value.behaviorRaw}
        onChange={(v) => set("behaviorRaw", v)}
        placeholder='{"scaleDown": {"stabilizationWindowSeconds": 300}}'
      />
    </div>
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
          <Label htmlFor={`${idPrefix}-kind`} className="text-xs">
            Trigger kind
          </Label>
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
            <Label htmlFor={`${idPrefix}-cron-start`} className="text-xs">
              start (cron)
            </Label>
            <Input
              id={`${idPrefix}-cron-start`}
              value={value.cron.start}
              placeholder="0 8 * * 1-5"
              onChange={(e) => setCron("start", e.target.value)}
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor={`${idPrefix}-cron-end`} className="text-xs">
              end (cron)
            </Label>
            <Input
              id={`${idPrefix}-cron-end`}
              value={value.cron.end}
              placeholder="0 18 * * 1-5"
              onChange={(e) => setCron("end", e.target.value)}
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor={`${idPrefix}-cron-replicas`} className="text-xs">
              desiredReplicas
            </Label>
            <Input
              id={`${idPrefix}-cron-replicas`}
              type="number"
              value={value.cron.desiredReplicas}
              placeholder="5"
              onChange={(e) => setCron("desiredReplicas", e.target.value)}
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor={`${idPrefix}-cron-tz`} className="text-xs">
              timezone
            </Label>
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
            <Label htmlFor={`${idPrefix}-prom-addr`} className="text-xs">
              serverAddress
            </Label>
            <Input
              id={`${idPrefix}-prom-addr`}
              value={value.prometheus.serverAddress}
              placeholder="http://prometheus:9090"
              onChange={(e) => setProm("serverAddress", e.target.value)}
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor={`${idPrefix}-prom-metric`} className="text-xs">
              metricName
            </Label>
            <Input
              id={`${idPrefix}-prom-metric`}
              value={value.prometheus.metricName}
              placeholder="trino_running_queries"
              onChange={(e) => setProm("metricName", e.target.value)}
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor={`${idPrefix}-prom-query`} className="text-xs">
              query
            </Label>
            <Input
              id={`${idPrefix}-prom-query`}
              value={value.prometheus.query}
              placeholder="sum(trino_running_queries)"
              onChange={(e) => setProm("query", e.target.value)}
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor={`${idPrefix}-prom-threshold`} className="text-xs">
              threshold
            </Label>
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
          value={value.rawOverride}
          onChange={(v) => onChange({ ...value, rawOverride: v })}
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
    <div className="space-y-4 rounded-lg border p-4">
      <p className="text-xs text-muted-foreground">
        KEDA requires the KEDA operator installed in the cluster.
      </p>

      <div className="grid grid-cols-1 gap-4 sm:grid-cols-4">
        <div className="space-y-1.5">
          <Label htmlFor="as-keda-min" className="text-xs">
            minReplicaCount
          </Label>
          <Input
            id="as-keda-min"
            type="number"
            value={value.minReplicaCount}
            placeholder="1"
            onChange={(e) => set("minReplicaCount", e.target.value)}
          />
        </div>
        <div className="space-y-1.5">
          <Label htmlFor="as-keda-max" className="text-xs">
            maxReplicaCount
          </Label>
          <Input
            id="as-keda-max"
            type="number"
            value={value.maxReplicaCount}
            placeholder="10"
            onChange={(e) => set("maxReplicaCount", e.target.value)}
          />
        </div>
        <div className="space-y-1.5">
          <Label htmlFor="as-keda-poll" className="text-xs">
            pollingInterval
          </Label>
          <Input
            id="as-keda-poll"
            type="number"
            value={value.pollingInterval}
            placeholder="30"
            onChange={(e) => set("pollingInterval", e.target.value)}
          />
        </div>
        <div className="space-y-1.5">
          <Label htmlFor="as-keda-cooldown" className="text-xs">
            cooldownPeriod
          </Label>
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
        id="as-keda-fallback"
        label="Fallback (raw JSON, merged into spec.fallback)"
        value={value.fallbackRaw}
        onChange={(v) => set("fallbackRaw", v)}
        placeholder='{"failureThreshold": 3, "replicas": 2}'
      />
    </div>
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
  /** Reports whether every raw-JSON override parses; blocks Save on false. */
  onValidityChange: (valid: boolean) => void;
}

/**
 * "Autoscaling" config sub-tab: scale the worker Deployment with either a native
 * HorizontalPodAutoscaler (CPU/memory targets) or a KEDA ScaledObject (cron /
 * Prometheus / raw triggers). Off drops `autoscaling` from the payload entirely.
 */
export function EngineAutoscaling({
  mode,
  setMode,
  hpa,
  setHpa,
  keda,
  setKeda,
  onValidityChange,
}: EngineAutoscalingProps) {
  // Aggregate raw-JSON validity across whichever mode's textareas are visible.
  useEffect(() => {
    let valid = true;
    if (mode === "hpa") {
      valid = validateRawJson(hpa.behaviorRaw) === null;
    } else if (mode === "keda") {
      valid =
        validateRawJson(keda.fallbackRaw) === null &&
        keda.triggers.every(
          (t) => t.kind !== "raw" || validateRawJson(t.rawOverride) === null,
        );
    }
    onValidityChange(valid);
  }, [mode, hpa, keda, onValidityChange]);

  return (
    <div className="space-y-4">
      <div className="max-w-xs space-y-1.5">
        <Label htmlFor="as-mode">Autoscaling</Label>
        <Select value={mode || "off"} onValueChange={(v) => setMode(v === "off" ? "" : (v as AutoscaleMode))}>
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
