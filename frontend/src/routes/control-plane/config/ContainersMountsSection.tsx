import { useEffect, useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import { Plus, Trash2 } from "lucide-react";

import type {
  EnvFromSource,
  MiniContainer,
  ObjectMount,
  VolumeMount,
} from "@/api/controlPlane";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  type Kv,
  KeyValueEditor,
  kvToRecord,
  recordToKv,
} from "../EngineResourceGroups";
import { ConfigSection } from "./ConfigSection";
import { RawJsonField } from "./RawJsonField";
import { numOrUndef } from "./podEditors";

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

// ── small string-list editor (command / args) ───────────────────────────────

function StringListEditor({
  rows,
  onChange,
  placeholder,
  addLabel,
}: {
  rows: string[];
  onChange: (rows: string[]) => void;
  placeholder?: string;
  addLabel: string;
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
        {addLabel}
      </Button>
    </div>
  );
}

// ── record<string,string> editor backed by a KeyValueEditor ──────────────────

/**
 * A `KeyValueEditor` over a `Record<string,string>` model field (e.g. a
 * mini-container's `env`). Holds the rows locally so an in-progress empty-key
 * row survives; commits the compacted record upstream (undefined when empty).
 * Re-syncs only on an external change (a server prefill), never on its own echo.
 */
function RecordKvEditor({
  value,
  onChange,
  keyPlaceholder,
  valuePlaceholder,
  addLabel,
}: {
  value: Record<string, string> | undefined;
  onChange: (next: Record<string, string> | undefined) => void;
  keyPlaceholder: string;
  valuePlaceholder: string;
  addLabel: string;
}) {
  const [rows, setRows] = useState<Kv[]>(() => recordToKv(value));

  useEffect(() => {
    // Ignore echoes of our own commits; re-sync when the incoming record differs.
    if (JSON.stringify(kvToRecord(rows)) !== JSON.stringify(value ?? {})) {
      setRows(recordToKv(value));
    }
  }, [value, rows]);

  const update = (next: Kv[]) => {
    setRows(next);
    const rec = kvToRecord(next);
    onChange(Object.keys(rec).length ? rec : undefined);
  };

  return (
    <KeyValueEditor
      rows={rows}
      onChange={update}
      keyPlaceholder={keyPlaceholder}
      valuePlaceholder={valuePlaceholder}
      addLabel={addLabel}
    />
  );
}

// ── mini-container editor (init / sidecar) ───────────────────────────────────

/**
 * A reusable init/sidecar container editor: name/image, command + args string
 * lists, an `env` KV, volume-mount rows, and a raw-JSON override that (when
 * present) wins verbatim server-side. The raw field registers its validity in
 * the surrounding provider so an invalid override blocks Save.
 */
function MiniContainerEditor({
  idPrefix,
  rawId,
  value,
  onChange,
}: {
  idPrefix: string;
  rawId: string;
  value: MiniContainer;
  onChange: (next: MiniContainer) => void;
}) {
  const set = <K extends keyof MiniContainer>(k: K, v: MiniContainer[K]) =>
    onChange({ ...value, [k]: v });
  const vms = value.volumeMounts ?? [];
  const setVm = (i: number, next: VolumeMount) =>
    set("volumeMounts", vms.map((m, j) => (j === i ? next : m)));

  return (
    <div className="space-y-3 rounded-md border p-3">
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
        <div className="space-y-1.5">
          <Label htmlFor={`${idPrefix}-name`} className="text-xs">
            name
          </Label>
          <Input
            id={`${idPrefix}-name`}
            value={value.name}
            placeholder="wait-for-dep"
            onChange={(e) => set("name", e.target.value)}
          />
        </div>
        <div className="space-y-1.5">
          <Label htmlFor={`${idPrefix}-image`} className="text-xs">
            image
          </Label>
          <Input
            id={`${idPrefix}-image`}
            value={value.image}
            placeholder="busybox:1.36"
            onChange={(e) => set("image", e.target.value)}
          />
        </div>
      </div>

      <div className="space-y-1.5">
        <Label className="text-xs">command</Label>
        <StringListEditor
          rows={value.command ?? []}
          onChange={(rows) => set("command", rows.length ? rows : undefined)}
          placeholder="sh"
          addLabel="Add command arg"
        />
      </div>

      <div className="space-y-1.5">
        <Label className="text-xs">args</Label>
        <StringListEditor
          rows={value.args ?? []}
          onChange={(rows) => set("args", rows.length ? rows : undefined)}
          placeholder="-c"
          addLabel="Add arg"
        />
      </div>

      <div className="space-y-1.5">
        <Label className="text-xs">env</Label>
        <RecordKvEditor
          value={value.env}
          onChange={(rec) => set("env", rec)}
          keyPlaceholder="NAME"
          valuePlaceholder="value"
          addLabel="Add env var"
        />
      </div>

      <div className="space-y-1.5">
        <Label className="text-xs">volume mounts</Label>
        <div className="space-y-2">
          {vms.map((m, i) => (
            <div key={i} className="flex items-start gap-2">
              <div className="grid flex-1 grid-cols-1 gap-2 sm:grid-cols-3">
                <Input
                  value={m.name}
                  placeholder="volume name"
                  onChange={(e) => setVm(i, { ...m, name: e.target.value })}
                />
                <Input
                  value={m.mountPath}
                  placeholder="/mnt/data"
                  onChange={(e) => setVm(i, { ...m, mountPath: e.target.value })}
                />
                <Input
                  value={m.subPath ?? ""}
                  placeholder="subPath (optional)"
                  onChange={(e) =>
                    setVm(i, { ...m, subPath: e.target.value || undefined })
                  }
                />
              </div>
              <Button
                type="button"
                variant="ghost"
                size="icon"
                className="shrink-0 text-muted-foreground"
                onClick={() =>
                  set(
                    "volumeMounts",
                    vms.filter((_, j) => j !== i),
                  )
                }
                aria-label="Remove volume mount"
              >
                <Trash2 className="h-4 w-4" />
              </Button>
            </div>
          ))}
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => set("volumeMounts", [...vms, { name: "", mountPath: "" }])}
          >
            <Plus className="h-4 w-4" />
            Add volume mount
          </Button>
        </div>
      </div>

      <RawJsonField
        id={rawId}
        label="Raw override (wins verbatim)"
        value={value.rawOverride}
        onChange={(parsed) => set("rawOverride", parsed)}
        placeholder='{"resources": {}}'
      />
    </div>
  );
}

/** A list of `MiniContainerEditor`s with add/remove (init or sidecar). */
function MiniContainerList({
  idPrefix,
  items,
  onChange,
  addLabel,
}: {
  idPrefix: string;
  items: MiniContainer[];
  onChange: Dispatch<SetStateAction<MiniContainer[]>>;
  addLabel: string;
}) {
  return (
    <div className="space-y-3">
      {items.map((c, i) => (
        <div key={i} className="space-y-2">
          <div className="flex items-center justify-end">
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="text-muted-foreground"
              onClick={() => onChange(items.filter((_, j) => j !== i))}
            >
              <Trash2 className="h-4 w-4" />
              Remove container
            </Button>
          </div>
          <MiniContainerEditor
            idPrefix={`${idPrefix}-${i}`}
            rawId={`containers.${idPrefix}.${i}.raw`}
            value={c}
            onChange={(next) =>
              onChange(items.map((x, j) => (j === i ? next : x)))
            }
          />
        </div>
      ))}
      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={() => onChange([...items, { name: "", image: "" }])}
      >
        <Plus className="h-4 w-4" />
        {addLabel}
      </Button>
    </div>
  );
}

// ── section props ────────────────────────────────────────────────────────────

export interface ContainersMountsSectionProps {
  initContainers: MiniContainer[];
  setInitContainers: Dispatch<SetStateAction<MiniContainer[]>>;
  sidecarContainers: MiniContainer[];
  setSidecarContainers: Dispatch<SetStateAction<MiniContainer[]>>;
  envFrom: EnvFromSource[];
  setEnvFrom: Dispatch<SetStateAction<EnvFromSource[]>>;
  configMounts: ObjectMount[];
  setConfigMounts: Dispatch<SetStateAction<ObjectMount[]>>;
  secretMounts: ObjectMount[];
  setSecretMounts: Dispatch<SetStateAction<ObjectMount[]>>;
}

/**
 * "Containers & Mounts" config: cluster-wide init & sidecar containers, envFrom
 * sources, and ConfigMap / Secret volume mounts — applied to both roles.
 */
export function ContainersMountsSection({
  initContainers,
  setInitContainers,
  sidecarContainers,
  setSidecarContainers,
  envFrom,
  setEnvFrom,
  configMounts,
  setConfigMounts,
  secretMounts,
  setSecretMounts,
}: ContainersMountsSectionProps) {
  const setEnvFromRow = (i: number, next: EnvFromSource) =>
    setEnvFrom(envFrom.map((r, j) => (j === i ? next : r)));

  return (
    <div className="space-y-5">
      <ConfigSection
        title="Environment from sources"
        description="Bulk-import environment variables from a ConfigMap or Secret."
      >
        <div className="space-y-2">
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
      </ConfigSection>

      <ConfigSection
        title="Volume mounts"
        description="Mount ConfigMaps and Secrets into the Trino container."
      >
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
      </ConfigSection>

      <ConfigSection
        title="Init & sidecar containers"
        description="Extra containers added to both the coordinator and worker pods."
      >
        <div className="space-y-2">
          <Label>Init containers</Label>
          <p className="text-xs text-muted-foreground">
            Run to completion before the Trino container starts (both roles).
          </p>
          <MiniContainerList
            idPrefix="init"
            items={initContainers}
            onChange={setInitContainers}
            addLabel="Add init container"
          />
        </div>
        <div className="space-y-2">
          <Label>Sidecar containers</Label>
          <p className="text-xs text-muted-foreground">
            Extra containers that run alongside Trino (both roles).
          </p>
          <MiniContainerList
            idPrefix="sidecar"
            items={sidecarContainers}
            onChange={setSidecarContainers}
            addLabel="Add sidecar container"
          />
        </div>
      </ConfigSection>
    </div>
  );
}
