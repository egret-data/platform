import { ChevronDown, Plus, Trash2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { Switch } from "@/components/ui/switch";
import { ConfigSection } from "./config/ConfigSection";
import { FieldLabel } from "./config/FieldLabel";
import { QuantityInput } from "./config/QuantityInput";
import {
  type ProbeForm,
  type RoleForm,
  PROBE_FIELDS,
} from "./useEngineConfigForm";

function StringListEditor({
  rows,
  onChange,
  placeholder,
}: {
  rows: string[];
  onChange: (rows: string[]) => void;
  placeholder?: string;
}) {
  return (
    <div className="space-y-2">
      {rows.map((row, i) => (
        <div key={i} className="flex items-center gap-2">
          <Input
            value={row}
            placeholder={placeholder}
            onChange={(e) =>
              onChange(rows.map((r, j) => (j === i ? e.target.value : r)))
            }
          />
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="shrink-0 text-muted-foreground"
            onClick={() => onChange(rows.filter((_, j) => j !== i))}
            aria-label="Remove line"
          >
            <Trash2 className="h-4 w-4" />
          </Button>
        </div>
      ))}
      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={() => onChange([...rows, ""])}
      >
        <Plus className="h-4 w-4" />
        Add JVM flag
      </Button>
    </div>
  );
}

/** A single collapsible probe editor (liveness/readiness/startup). */
function ProbeEditor({
  label,
  idPrefix,
  value,
  onChange,
}: {
  label: string;
  idPrefix: string;
  value: ProbeForm;
  onChange: (next: ProbeForm) => void;
}) {
  return (
    <Collapsible className="rounded-md border">
      <CollapsibleTrigger className="group flex w-full items-center justify-between px-3 py-2 text-sm font-medium">
        {label} probe
        <ChevronDown className="h-4 w-4 text-muted-foreground transition-transform group-data-[state=open]:rotate-180" />
      </CollapsibleTrigger>
      <CollapsibleContent className="border-t px-3 py-3">
        <div className="grid grid-cols-2 gap-3 sm:grid-cols-3">
          {PROBE_FIELDS.map((f) => (
            <div key={f.key} className="space-y-1.5">
              <FieldLabel
                fieldKey={`probe.${f.key}`}
                label={f.label}
                htmlFor={`${idPrefix}-${f.key}`}
              />
              <Input
                id={`${idPrefix}-${f.key}`}
                type="number"
                min={0}
                value={value[f.key]}
                placeholder="default"
                onChange={(e) => onChange({ ...value, [f.key]: e.target.value })}
              />
            </div>
          ))}
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
}

/** Shared editor for a Trino role (coordinator or worker) config block. */
function RoleConfigEditor({
  value,
  onChange,
  idPrefix,
  keyPrefix,
}: {
  value: RoleForm;
  onChange: (next: RoleForm) => void;
  idPrefix: string;
  keyPrefix: "coordinator" | "worker";
}) {
  const set = <K extends keyof RoleForm>(key: K, v: RoleForm[K]) =>
    onChange({ ...value, [key]: v });

  return (
    <div className="space-y-5">
      <ConfigSection
        title="Replicas & resources"
        description="Pod count and the Kubernetes CPU/memory request and limit for each pod."
        docsUrl="https://kubernetes.io/docs/concepts/configuration/manage-resources-containers/"
      >
        <div className="grid grid-cols-2 gap-4 sm:grid-cols-4">
          <div className="space-y-1.5">
            <FieldLabel
              fieldKey={`${keyPrefix}.replicas`}
              label="Replicas"
              htmlFor={`${idPrefix}-replicas`}
            />
            <Input
              id={`${idPrefix}-replicas`}
              type="number"
              min={0}
              value={value.replicas}
              placeholder="1"
              onChange={(e) => set("replicas", e.target.value)}
            />
          </div>
        </div>

        <div className="grid grid-cols-2 gap-4 sm:grid-cols-4">
          <QuantityInput
            fieldKey={`${keyPrefix}.resources.cpu`}
            label="CPU request"
            value={value.cpuRequest}
            placeholder="1"
            onChange={(v) => set("cpuRequest", v)}
          />
          <QuantityInput
            fieldKey={`${keyPrefix}.resources.cpu`}
            label="CPU limit"
            value={value.cpuLimit}
            placeholder="2"
            onChange={(v) => set("cpuLimit", v)}
          />
          <QuantityInput
            fieldKey={`${keyPrefix}.resources.memory`}
            label="Memory request"
            value={value.memRequest}
            placeholder="2Gi"
            onChange={(v) => set("memRequest", v)}
          />
          <QuantityInput
            fieldKey={`${keyPrefix}.resources.memory`}
            label="Memory limit"
            value={value.memLimit}
            placeholder="4Gi"
            onChange={(v) => set("memLimit", v)}
          />
        </div>
      </ConfigSection>

      <ConfigSection
        title="JVM heap"
        description="Size the JVM heap as a fixed value (-Xmx/-Xms) or as a percentage of the pod memory limit."
        docsUrl="https://trino.io/docs/current/installation/deployment.html#jvm-config"
      >
        <RadioGroup
          className="flex gap-6"
          value={value.heapMode}
          onValueChange={(v) => set("heapMode", v as RoleForm["heapMode"])}
        >
          <div className="flex items-center gap-2">
            <RadioGroupItem value="fixed" id={`${idPrefix}-heap-fixed`} />
            <Label htmlFor={`${idPrefix}-heap-fixed`} className="font-normal">
              Fixed size
            </Label>
          </div>
          <div className="flex items-center gap-2">
            <RadioGroupItem value="percent" id={`${idPrefix}-heap-percent`} />
            <Label htmlFor={`${idPrefix}-heap-percent`} className="font-normal">
              Percent
            </Label>
          </div>
        </RadioGroup>
        {value.heapMode === "fixed" ? (
          <div className="grid grid-cols-2 gap-4">
            <QuantityInput
              fieldKey={`${keyPrefix}.heap.maxSize`}
              label="Max size (-Xmx)"
              value={value.maxSize}
              placeholder="2G"
              onChange={(v) => set("maxSize", v)}
            />
            <QuantityInput
              fieldKey={`${keyPrefix}.heap.minSize`}
              label="Min size (-Xms)"
              value={value.minSize}
              placeholder="2G"
              onChange={(v) => set("minSize", v)}
            />
          </div>
        ) : (
          <div className="grid grid-cols-2 gap-4">
            <div className="space-y-1.5">
              <FieldLabel
                fieldKey={`${keyPrefix}.heap.maxPercent`}
                label="Max RAM percent"
                htmlFor={`${idPrefix}-heap-maxpct`}
              />
              <Input
                id={`${idPrefix}-heap-maxpct`}
                type="number"
                min={0}
                max={100}
                value={value.maxPercent}
                placeholder="80"
                onChange={(e) => set("maxPercent", e.target.value)}
              />
            </div>
            <div className="space-y-1.5">
              <FieldLabel
                fieldKey={`${keyPrefix}.heap.initialPercent`}
                label="Initial RAM percent"
                htmlFor={`${idPrefix}-heap-initpct`}
              />
              <Input
                id={`${idPrefix}-heap-initpct`}
                type="number"
                min={0}
                max={100}
                value={value.initialPercent}
                placeholder="80"
                onChange={(e) => set("initialPercent", e.target.value)}
              />
            </div>
          </div>
        )}
      </ConfigSection>

      <ConfigSection
        title="Query memory"
        description="Per-node query memory limits for this role."
        docsUrl="https://trino.io/docs/current/admin/properties-resource-management.html"
      >
        <div className="grid grid-cols-2 gap-4">
          <QuantityInput
            fieldKey="queryMaxMemoryPerNode"
            label="Query max memory / node"
            value={value.queryMaxMemoryPerNode}
            placeholder="1GB"
            onChange={(v) => set("queryMaxMemoryPerNode", v)}
          />
          <QuantityInput
            fieldKey="memoryHeapHeadroomPerNode"
            label="Heap headroom / node"
            value={value.heapHeadroomPerNode}
            placeholder="2GB"
            onChange={(v) => set("heapHeadroomPerNode", v)}
          />
        </div>
      </ConfigSection>

      <ConfigSection
        title="Additional JVM config"
        description="Extra lines appended to jvm.config."
        docsUrl="https://trino.io/docs/current/installation/deployment.html#jvm-config"
      >
        <StringListEditor
          rows={value.additionalJvmConfig}
          onChange={(rows) => set("additionalJvmConfig", rows)}
          placeholder="-XX:MaxHeapFreeRatio=100"
        />
      </ConfigSection>

      <ConfigSection
        title="Health probes"
        description="Kubernetes liveness, readiness, and startup probe timings."
        docsUrl="https://kubernetes.io/docs/concepts/workloads/pods/pod-lifecycle/#configuring-probes"
      >
        <div className="space-y-2">
          <ProbeEditor
            label="Liveness"
            idPrefix={`${idPrefix}-live`}
            value={value.liveness}
            onChange={(p) => set("liveness", p)}
          />
          <ProbeEditor
            label="Readiness"
            idPrefix={`${idPrefix}-ready`}
            value={value.readiness}
            onChange={(p) => set("readiness", p)}
          />
          <ProbeEditor
            label="Startup"
            idPrefix={`${idPrefix}-startup`}
            value={value.startup}
            onChange={(p) => set("startup", p)}
          />
        </div>
      </ConfigSection>
    </div>
  );
}

/**
 * Per-role Trino config (coordinator or worker). Renders the shared
 * `RoleConfigEditor`; the worker adds the graceful-shutdown block, driven by the
 * optional `gs*` props (present only for the worker role).
 */
export function EngineRoleConfig({
  role,
  value,
  onChange,
  gsEnabled,
  onGsEnabledChange,
  gracePeriod,
  onGracePeriodChange,
}: {
  role: "coordinator" | "worker";
  value: RoleForm;
  onChange: React.Dispatch<React.SetStateAction<RoleForm>>;
  // worker-only graceful shutdown (undefined for coordinator):
  gsEnabled?: boolean;
  onGsEnabledChange?: (v: boolean) => void;
  gracePeriod?: string;
  onGracePeriodChange?: (v: string) => void;
}) {
  const idPrefix = role === "coordinator" ? "ced-coord" : "ced-worker";

  if (role === "coordinator") {
    return (
      <RoleConfigEditor
        idPrefix={idPrefix}
        keyPrefix="coordinator"
        value={value}
        onChange={onChange}
      />
    );
  }

  return (
    <div className="space-y-5">
      <RoleConfigEditor
        idPrefix={idPrefix}
        keyPrefix="worker"
        value={value}
        onChange={onChange}
      />
      <ConfigSection
        title="Graceful shutdown"
        description="Drain in-flight queries before a worker terminates."
        docsUrl="https://trino.io/docs/current/admin/graceful-shutdown.html"
      >
        <div className="flex items-center justify-between gap-4">
          <FieldLabel
            fieldKey="worker.gracefulShutdown"
            label="Enable graceful shutdown"
            htmlFor="ced-gs-enabled"
          />
          <Switch
            id="ced-gs-enabled"
            checked={gsEnabled}
            onCheckedChange={onGsEnabledChange}
          />
        </div>
        {gsEnabled && (
          <div className="space-y-1.5">
            <FieldLabel
              fieldKey="worker.gracePeriod"
              label="Grace period (s)"
              htmlFor="ced-gs-grace"
            />
            <Input
              id="ced-gs-grace"
              type="number"
              min={0}
              value={gracePeriod}
              placeholder="120"
              onChange={(e) => onGracePeriodChange?.(e.target.value)}
            />
          </div>
        )}
      </ConfigSection>
    </div>
  );
}
