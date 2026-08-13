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
import { Switch } from "@/components/ui/switch";
import {
  type Kv,
  KeyValueEditor,
  kvToRecord,
  recordToKv,
} from "../EngineResourceGroups";
import type { RoleForm } from "../useEngineConfigForm";
import { ConfigSection } from "@/components/form/ConfigSection";
import { ConfigTable, ConfigTableRow } from "./ConfigTable";
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

/**
 * A list of `MiniContainerEditor`s with add/remove (init or sidecar).
 *
 * Extracted for reuse by the StarRocks Settings tab (exported below as
 * `ContainerListEditor`). `idPrefix` / `addLabel` are optional so a standalone
 * caller need only supply `rows` + `onChange`; the Trino callers keep passing
 * them explicitly and render identically.
 */
function MiniContainerList({
  idPrefix = "container",
  rows,
  onChange,
  addLabel = "Add container",
}: {
  idPrefix?: string;
  rows: MiniContainer[];
  onChange: (rows: MiniContainer[]) => void;
  addLabel?: string;
}) {
  return (
    <div className="space-y-3">
      {rows.map((c, i) => (
        <div key={i} className="space-y-2">
          <div className="flex items-center justify-end">
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="text-muted-foreground"
              onClick={() => onChange(rows.filter((_, j) => j !== i))}
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
              onChange(rows.map((x, j) => (j === i ? next : x)))
            }
          />
        </div>
      ))}
      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={() => onChange([...rows, { name: "", image: "" }])}
      >
        <Plus className="h-4 w-4" />
        {addLabel}
      </Button>
    </div>
  );
}

/**
 * Standalone container-list editor for reuse (e.g. the StarRocks Settings tab).
 * `ContainerForm` is the `MiniContainer` shape (name/image, command/args string
 * lists, env `Record<string,string>`, volume mounts, raw-JSON override).
 */
export { MiniContainerList as ContainerListEditor };
export type { MiniContainer as ContainerForm } from "@/api/controlPlane";

// ── envFrom (cluster-wide) ───────────────────────────────────────────────────

/** Cluster-wide `envFrom` sources (chart top-level `envFrom`). Applied to all
 *  pods, so it lives in the Pod tab (not per role). */
export function EnvFromSection({
  envFrom,
  setEnvFrom,
}: {
  envFrom: EnvFromSource[];
  setEnvFrom: Dispatch<SetStateAction<EnvFromSource[]>>;
}) {
  const setEnvFromRow = (i: number, next: EnvFromSource) =>
    setEnvFrom(envFrom.map((r, j) => (j === i ? next : r)));

  return (
    <ConfigSection
      title="Environment from sources"
      description="Bulk-import environment variables from a ConfigMap or Secret (all pods)."
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
  );
}

// ── per-role pod shape (containers + mounts + shareProcessNamespace) ──────────

/**
 * Per-role init/sidecar containers, ConfigMap/Secret volume mounts, and the
 * shareProcessNamespace toggle — chart `coordinator.*` / `worker.*`. Edits a
 * {@link RoleForm} in place via `onChange`.
 */
export function RolePodShapeSection({
  role,
  value,
  onChange,
}: {
  role: "coordinator" | "worker";
  value: RoleForm;
  onChange: (next: RoleForm) => void;
}) {
  // Adapt a plain field update to the SetStateAction the list editors expect.
  const setter =
    <K extends "initContainers" | "sidecarContainers" | "configMounts" | "secretMounts">(
      key: K,
    ): Dispatch<SetStateAction<RoleForm[K]>> =>
    (u) =>
      onChange({
        ...value,
        [key]: typeof u === "function" ? (u as (p: RoleForm[K]) => RoleForm[K])(value[key]) : u,
      });

  return (
    <div className="space-y-5">
      <ConfigSection
        title="Init & sidecar containers"
        description={`Extra containers added to the ${role} pods.`}
      >
        <div className="space-y-2">
          <Label>Init containers</Label>
          <p className="text-xs text-muted-foreground">
            Run to completion before the Trino container starts.
          </p>
          <MiniContainerList
            idPrefix={`${role}-init`}
            rows={value.initContainers}
            onChange={setter("initContainers")}
            addLabel="Add init container"
          />
        </div>
        <div className="space-y-2">
          <Label>Sidecar containers</Label>
          <p className="text-xs text-muted-foreground">
            Extra containers that run alongside Trino.
          </p>
          <MiniContainerList
            idPrefix={`${role}-sidecar`}
            rows={value.sidecarContainers}
            onChange={setter("sidecarContainers")}
            addLabel="Add sidecar container"
          />
        </div>
      </ConfigSection>

      <ConfigSection
        title="Volume mounts"
        description={`Mount ConfigMaps and Secrets into the ${role} container.`}
      >
        <div className="space-y-2">
          <Label>ConfigMap mounts</Label>
          <ObjectMountRows
            rows={value.configMounts}
            onChange={setter("configMounts")}
            namePlaceholder="configMap name"
          />
        </div>
        <div className="space-y-2">
          <Label>Secret mounts</Label>
          <ObjectMountRows
            rows={value.secretMounts}
            onChange={setter("secretMounts")}
            namePlaceholder="secret name"
          />
        </div>
      </ConfigSection>

      <ConfigSection title="Pod options">
        <ConfigTable>
          <ConfigTableRow
            label="shareProcessNamespace"
            mono
            htmlFor={`${role}-sharepid`}
            details="Let containers in the pod share a single PID namespace."
          >
            <Switch
              id={`${role}-sharepid`}
              checked={value.shareProcessNamespace}
              onCheckedChange={(v) => onChange({ ...value, shareProcessNamespace: v })}
            />
          </ConfigTableRow>
        </ConfigTable>
      </ConfigSection>
    </div>
  );
}
