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
              <Label htmlFor={`${idPrefix}-${f.key}`} className="text-xs">
                {f.label}
              </Label>
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
}: {
  value: RoleForm;
  onChange: (next: RoleForm) => void;
  idPrefix: string;
}) {
  const set = <K extends keyof RoleForm>(key: K, v: RoleForm[K]) =>
    onChange({ ...value, [key]: v });

  return (
    <div className="space-y-5">
      <div className="grid grid-cols-2 gap-4 sm:grid-cols-4">
        <div className="space-y-1.5">
          <Label htmlFor={`${idPrefix}-replicas`}>Replicas</Label>
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

      <div className="space-y-2">
        <Label>Resources</Label>
        <div className="grid grid-cols-2 gap-4 sm:grid-cols-4">
          <div className="space-y-1.5">
            <Label htmlFor={`${idPrefix}-cpu-req`} className="text-xs">
              CPU request
            </Label>
            <Input
              id={`${idPrefix}-cpu-req`}
              value={value.cpuRequest}
              placeholder="1"
              onChange={(e) => set("cpuRequest", e.target.value)}
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor={`${idPrefix}-cpu-lim`} className="text-xs">
              CPU limit
            </Label>
            <Input
              id={`${idPrefix}-cpu-lim`}
              value={value.cpuLimit}
              placeholder="2"
              onChange={(e) => set("cpuLimit", e.target.value)}
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor={`${idPrefix}-mem-req`} className="text-xs">
              Memory request
            </Label>
            <Input
              id={`${idPrefix}-mem-req`}
              value={value.memRequest}
              placeholder="2Gi"
              onChange={(e) => set("memRequest", e.target.value)}
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor={`${idPrefix}-mem-lim`} className="text-xs">
              Memory limit
            </Label>
            <Input
              id={`${idPrefix}-mem-lim`}
              value={value.memLimit}
              placeholder="4Gi"
              onChange={(e) => set("memLimit", e.target.value)}
            />
          </div>
        </div>
      </div>

      <div className="space-y-2">
        <Label>JVM heap</Label>
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
            <div className="space-y-1.5">
              <Label htmlFor={`${idPrefix}-heap-max`} className="text-xs">
                Max size (-Xmx)
              </Label>
              <Input
                id={`${idPrefix}-heap-max`}
                value={value.maxSize}
                placeholder="2G"
                onChange={(e) => set("maxSize", e.target.value)}
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor={`${idPrefix}-heap-min`} className="text-xs">
                Min size (-Xms)
              </Label>
              <Input
                id={`${idPrefix}-heap-min`}
                value={value.minSize}
                placeholder="2G"
                onChange={(e) => set("minSize", e.target.value)}
              />
            </div>
          </div>
        ) : (
          <div className="grid grid-cols-2 gap-4">
            <div className="space-y-1.5">
              <Label htmlFor={`${idPrefix}-heap-maxpct`} className="text-xs">
                Max RAM percent
              </Label>
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
              <Label htmlFor={`${idPrefix}-heap-initpct`} className="text-xs">
                Initial RAM percent
              </Label>
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
      </div>

      <div className="grid grid-cols-2 gap-4">
        <div className="space-y-1.5">
          <Label htmlFor={`${idPrefix}-qmmpn`}>Query max memory / node</Label>
          <Input
            id={`${idPrefix}-qmmpn`}
            value={value.queryMaxMemoryPerNode}
            placeholder="1GB"
            onChange={(e) => set("queryMaxMemoryPerNode", e.target.value)}
          />
        </div>
        <div className="space-y-1.5">
          <Label htmlFor={`${idPrefix}-hhpn`}>Heap headroom / node</Label>
          <Input
            id={`${idPrefix}-hhpn`}
            value={value.heapHeadroomPerNode}
            placeholder="2GB"
            onChange={(e) => set("heapHeadroomPerNode", e.target.value)}
          />
        </div>
      </div>

      <div className="space-y-1.5">
        <Label>Additional JVM config</Label>
        <p className="text-xs text-muted-foreground">
          Extra lines appended to <code>jvm.config</code>.
        </p>
        <StringListEditor
          rows={value.additionalJvmConfig}
          onChange={(rows) => set("additionalJvmConfig", rows)}
          placeholder="-XX:MaxHeapFreeRatio=100"
        />
      </div>

      <div className="space-y-2">
        <Label>Probes</Label>
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
      </div>
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
    return <RoleConfigEditor idPrefix={idPrefix} value={value} onChange={onChange} />;
  }

  return (
    <div className="space-y-5">
      <RoleConfigEditor idPrefix={idPrefix} value={value} onChange={onChange} />
      <div className="space-y-3 rounded-lg border p-4">
        <div className="flex items-center justify-between gap-4">
          <div className="space-y-0.5">
            <Label htmlFor="ced-gs-enabled">Graceful shutdown</Label>
            <p className="text-xs text-muted-foreground">
              Drain in-flight queries before a worker terminates.
            </p>
          </div>
          <Switch
            id="ced-gs-enabled"
            checked={gsEnabled}
            onCheckedChange={onGsEnabledChange}
          />
        </div>
        {gsEnabled && (
          <div className="space-y-1.5">
            <Label htmlFor="ced-gs-grace">Grace period (s)</Label>
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
      </div>
    </div>
  );
}
