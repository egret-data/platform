import { ChevronDown, Trash2 } from "lucide-react";

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
import { ConfigTable, ConfigTableRow } from "./config/ConfigTable";
import { RolePodShapeSection } from "./config/ContainersMountsSection";
import { FieldLabel } from "./config/FieldLabel";
import { JvmFlagPicker } from "./config/JvmFlagPicker";
import { QuantityValue } from "./config/QuantityInput";
import {
  type ProbeForm,
  type RoleForm,
  PROBE_FIELDS,
} from "./useEngineConfigForm";

function StringListEditor({
  rows,
  onChange,
  placeholder,
  addControl,
}: {
  rows: string[];
  onChange: (rows: string[]) => void;
  placeholder?: string;
  addControl: React.ReactNode;
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
      {addControl}
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
        <ConfigTable>
          <ConfigTableRow
            label="Replicas"
            fieldKey={`${keyPrefix}.replicas`}
            htmlFor={`${idPrefix}-replicas`}
          >
            <Input
              id={`${idPrefix}-replicas`}
              className="w-full"
              type="number"
              min={0}
              value={value.replicas}
              placeholder="1"
              onChange={(e) => set("replicas", e.target.value)}
            />
          </ConfigTableRow>
          <ConfigTableRow
            label="CPU request"
            fieldKey={`${keyPrefix}.resources.cpu`}
            htmlFor={`${idPrefix}-cpu-request`}
          >
            <QuantityValue
              id={`${idPrefix}-cpu-request`}
              value={value.cpuRequest}
              placeholder="1"
              onChange={(v) => set("cpuRequest", v)}
            />
          </ConfigTableRow>
          <ConfigTableRow
            label="CPU limit"
            fieldKey={`${keyPrefix}.resources.cpu`}
            htmlFor={`${idPrefix}-cpu-limit`}
          >
            <QuantityValue
              id={`${idPrefix}-cpu-limit`}
              value={value.cpuLimit}
              placeholder="2"
              onChange={(v) => set("cpuLimit", v)}
            />
          </ConfigTableRow>
          <ConfigTableRow
            label="Memory request"
            fieldKey={`${keyPrefix}.resources.memory`}
            htmlFor={`${idPrefix}-mem-request`}
          >
            <QuantityValue
              id={`${idPrefix}-mem-request`}
              value={value.memRequest}
              placeholder="2Gi"
              onChange={(v) => set("memRequest", v)}
            />
          </ConfigTableRow>
          <ConfigTableRow
            label="Memory limit"
            fieldKey={`${keyPrefix}.resources.memory`}
            htmlFor={`${idPrefix}-mem-limit`}
          >
            <QuantityValue
              id={`${idPrefix}-mem-limit`}
              value={value.memLimit}
              placeholder="4Gi"
              onChange={(v) => set("memLimit", v)}
            />
          </ConfigTableRow>
        </ConfigTable>
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
          <ConfigTable>
            <ConfigTableRow
              label="-Xmx"
              mono
              fieldKey={`${keyPrefix}.heap.maxSize`}
              htmlFor={`${idPrefix}-heap-maxsize`}
            >
              <QuantityValue
                id={`${idPrefix}-heap-maxsize`}
                value={value.maxSize}
                placeholder="2G"
                onChange={(v) => set("maxSize", v)}
              />
            </ConfigTableRow>
            <ConfigTableRow
              label="-Xms"
              mono
              fieldKey={`${keyPrefix}.heap.minSize`}
              htmlFor={`${idPrefix}-heap-minsize`}
            >
              <QuantityValue
                id={`${idPrefix}-heap-minsize`}
                value={value.minSize}
                placeholder="2G"
                onChange={(v) => set("minSize", v)}
              />
            </ConfigTableRow>
          </ConfigTable>
        ) : (
          <ConfigTable>
            <ConfigTableRow
              label="-XX:MaxRAMPercentage"
              mono
              fieldKey={`${keyPrefix}.heap.maxPercent`}
              htmlFor={`${idPrefix}-heap-maxpct`}
            >
              <Input
                id={`${idPrefix}-heap-maxpct`}
                className="w-full"
                type="number"
                min={0}
                max={100}
                value={value.maxPercent}
                placeholder="80"
                onChange={(e) => set("maxPercent", e.target.value)}
              />
            </ConfigTableRow>
            <ConfigTableRow
              label="-XX:InitialRAMPercentage"
              mono
              fieldKey={`${keyPrefix}.heap.initialPercent`}
              htmlFor={`${idPrefix}-heap-initpct`}
            >
              <Input
                id={`${idPrefix}-heap-initpct`}
                className="w-full"
                type="number"
                min={0}
                max={100}
                value={value.initialPercent}
                placeholder="80"
                onChange={(e) => set("initialPercent", e.target.value)}
              />
            </ConfigTableRow>
          </ConfigTable>
        )}
      </ConfigSection>

      <ConfigSection
        title="Query memory"
        description="Per-node query memory limits for this role."
        docsUrl="https://trino.io/docs/current/admin/properties-resource-management.html"
      >
        <ConfigTable>
          <ConfigTableRow
            label="query.max-memory-per-node"
            mono
            fieldKey="queryMaxMemoryPerNode"
            htmlFor={`${idPrefix}-qmmpn`}
          >
            <QuantityValue
              id={`${idPrefix}-qmmpn`}
              value={value.queryMaxMemoryPerNode}
              placeholder="1GB"
              onChange={(v) => set("queryMaxMemoryPerNode", v)}
            />
          </ConfigTableRow>
          <ConfigTableRow
            label="memory.heap-headroom-per-node"
            mono
            fieldKey="memoryHeapHeadroomPerNode"
            htmlFor={`${idPrefix}-hhpn`}
          >
            <QuantityValue
              id={`${idPrefix}-hhpn`}
              value={value.heapHeadroomPerNode}
              placeholder="2GB"
              onChange={(v) => set("heapHeadroomPerNode", v)}
            />
          </ConfigTableRow>
        </ConfigTable>
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
          addControl={
            <JvmFlagPicker
              onAdd={(flag) =>
                set("additionalJvmConfig", [...value.additionalJvmConfig, flag])
              }
            />
          }
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

      <RolePodShapeSection role={keyPrefix} value={value} onChange={onChange} />
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
        <ConfigTable>
          <ConfigTableRow
            label="Enable graceful shutdown"
            fieldKey="worker.gracefulShutdown"
            htmlFor="ced-gs-enabled"
          >
            <Switch
              id="ced-gs-enabled"
              checked={gsEnabled}
              onCheckedChange={onGsEnabledChange}
            />
          </ConfigTableRow>
          {gsEnabled && (
            <ConfigTableRow
              label="Grace period (s)"
              fieldKey="worker.gracePeriod"
              htmlFor="ced-gs-grace"
            >
              <Input
                id="ced-gs-grace"
                className="w-full"
                type="number"
                min={0}
                value={gracePeriod}
                placeholder="120"
                onChange={(e) => onGracePeriodChange?.(e.target.value)}
              />
            </ConfigTableRow>
          )}
        </ConfigTable>
      </ConfigSection>
    </div>
  );
}
