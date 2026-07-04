import { useEffect, useRef, useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import { Plus, Trash2 } from "lucide-react";

import type {
  DeploymentStrategy,
  EnvFromSource,
  MiniContainer,
  ObjectMount,
  SecurityContext,
  VolumeMount,
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
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import {
  type Kv,
  KeyValueEditor,
  kvToRecord,
  recordToKv,
} from "./EngineResourceGroups";
import type {
  ContainerPortForm,
  NodeMatchForm,
  RoleForm,
  TolerationForm,
  TopologySpreadForm,
} from "./useEngineConfigForm";

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

  // Keep a live ref to `onError` so the unmount cleanup can clear this field's
  // aggregated error (e.g. when a mini-container row is removed) without
  // re-running on every render.
  const onErrorRef = useRef(onError);
  onErrorRef.current = onError;
  useEffect(() => () => onErrorRef.current(null), []);

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
 * present) wins verbatim server-side. The raw textarea feeds `onRawError` so an
 * invalid override participates in the section-wide validity aggregation.
 */
function MiniContainerEditor({
  idPrefix,
  value,
  onChange,
  onRawError,
}: {
  idPrefix: string;
  value: MiniContainer;
  onChange: (next: MiniContainer) => void;
  onRawError: (err: string | null) => void;
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

      <RawJsonOverride
        id={`${idPrefix}-raw`}
        label="Raw override (wins verbatim)"
        value={value.rawOverride}
        onCommit={(parsed) => set("rawOverride", parsed)}
        onError={onRawError}
      />
    </div>
  );
}

/** A list of `MiniContainerEditor`s with add/remove (init or sidecar). */
function MiniContainerList({
  idPrefix,
  items,
  onChange,
  onRawError,
  addLabel,
}: {
  idPrefix: string;
  items: MiniContainer[];
  onChange: Dispatch<SetStateAction<MiniContainer[]>>;
  onRawError: (key: string) => (err: string | null) => void;
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
            value={c}
            onChange={(next) =>
              onChange(items.map((x, j) => (j === i ? next : x)))
            }
            onRawError={onRawError(`${idPrefix}-${i}`)}
          />
        </div>
      ))}
      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={() =>
          onChange([...items, { name: "", image: "" }])
        }
      >
        <Plus className="h-4 w-4" />
        {addLabel}
      </Button>
    </div>
  );
}

// ── per-role placement editor ────────────────────────────────────────────────

const TOLERATION_EFFECTS = ["NoSchedule", "PreferNoSchedule", "NoExecute"];
const WHEN_UNSATISFIABLE = ["DoNotSchedule", "ScheduleAnyway"];

/**
 * The per-role placement fields (nodeSelector / tolerations / topologySpread /
 * affinity / termination grace / extra ports) editing one role's `RoleForm`
 * slice. The affinity raw-JSON override is text-in-form; its validity is
 * checked centrally by the section (so both roles are covered regardless of the
 * selected tab).
 */
function RolePlacementEditor({
  idPrefix,
  form,
  onChange,
}: {
  idPrefix: string;
  form: RoleForm;
  onChange: Dispatch<SetStateAction<RoleForm>>;
}) {
  const setTolerations = (rows: TolerationForm[]) =>
    onChange((prev) => ({ ...prev, tolerations: rows }));
  const setTopology = (rows: TopologySpreadForm[]) =>
    onChange((prev) => ({ ...prev, topologySpread: rows }));
  const setPorts = (rows: ContainerPortForm[]) =>
    onChange((prev) => ({ ...prev, additionalExposedPorts: rows }));
  const setNodeMatch = (rows: NodeMatchForm[]) =>
    onChange((prev) => ({
      ...prev,
      affinity: { ...prev.affinity, nodeMatch: rows },
    }));

  return (
    <div className="space-y-6">
      {/* nodeSelector */}
      <div className="space-y-2">
        <Label>Node selector</Label>
        <KeyValueEditor
          rows={form.nodeSelector}
          onChange={(rows) => onChange((prev) => ({ ...prev, nodeSelector: rows }))}
          keyPlaceholder="disktype"
          valuePlaceholder="ssd"
          addLabel="Add label"
        />
      </div>

      {/* tolerations */}
      <div className="space-y-2">
        <Label>Tolerations</Label>
        {form.tolerations.map((t, i) => (
          <div key={i} className="flex items-start gap-2">
            <div className="grid flex-1 grid-cols-2 gap-2 sm:grid-cols-5">
              <Input
                value={t.key}
                placeholder="key"
                onChange={(e) =>
                  setTolerations(
                    form.tolerations.map((r, j) =>
                      j === i ? { ...r, key: e.target.value } : r,
                    ),
                  )
                }
              />
              <Input
                value={t.operator}
                placeholder="Equal / Exists"
                onChange={(e) =>
                  setTolerations(
                    form.tolerations.map((r, j) =>
                      j === i ? { ...r, operator: e.target.value } : r,
                    ),
                  )
                }
              />
              <Input
                value={t.value}
                placeholder="value"
                onChange={(e) =>
                  setTolerations(
                    form.tolerations.map((r, j) =>
                      j === i ? { ...r, value: e.target.value } : r,
                    ),
                  )
                }
              />
              <Select
                value={t.effect || SELECT_NONE}
                onValueChange={(v) =>
                  setTolerations(
                    form.tolerations.map((r, j) =>
                      j === i ? { ...r, effect: v === SELECT_NONE ? "" : v } : r,
                    ),
                  )
                }
              >
                <SelectTrigger className="w-full">
                  <SelectValue placeholder="effect" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value={SELECT_NONE}>(any effect)</SelectItem>
                  {TOLERATION_EFFECTS.map((e) => (
                    <SelectItem key={e} value={e}>
                      {e}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <Input
                type="number"
                value={t.tolerationSeconds}
                placeholder="tolerationSeconds"
                onChange={(e) =>
                  setTolerations(
                    form.tolerations.map((r, j) =>
                      j === i ? { ...r, tolerationSeconds: e.target.value } : r,
                    ),
                  )
                }
              />
            </div>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="shrink-0 text-muted-foreground"
              onClick={() =>
                setTolerations(form.tolerations.filter((_, j) => j !== i))
              }
              aria-label="Remove toleration"
            >
              <Trash2 className="h-4 w-4" />
            </Button>
          </div>
        ))}
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() =>
            setTolerations([
              ...form.tolerations,
              { key: "", operator: "", value: "", effect: "", tolerationSeconds: "" },
            ])
          }
        >
          <Plus className="h-4 w-4" />
          Add toleration
        </Button>
      </div>

      {/* topology spread */}
      <div className="space-y-2">
        <Label>Topology spread constraints</Label>
        {form.topologySpread.map((t, i) => (
          <div key={i} className="space-y-2 rounded-md border p-3">
            <div className="flex items-start gap-2">
              <div className="grid flex-1 grid-cols-1 gap-2 sm:grid-cols-3">
                <Input
                  type="number"
                  value={t.maxSkew}
                  placeholder="maxSkew"
                  onChange={(e) =>
                    setTopology(
                      form.topologySpread.map((r, j) =>
                        j === i ? { ...r, maxSkew: e.target.value } : r,
                      ),
                    )
                  }
                />
                <Input
                  value={t.topologyKey}
                  placeholder="kubernetes.io/hostname"
                  onChange={(e) =>
                    setTopology(
                      form.topologySpread.map((r, j) =>
                        j === i ? { ...r, topologyKey: e.target.value } : r,
                      ),
                    )
                  }
                />
                <Select
                  value={t.whenUnsatisfiable || SELECT_NONE}
                  onValueChange={(v) =>
                    setTopology(
                      form.topologySpread.map((r, j) =>
                        j === i
                          ? {
                              ...r,
                              whenUnsatisfiable: v === SELECT_NONE ? "" : v,
                            }
                          : r,
                      ),
                    )
                  }
                >
                  <SelectTrigger className="w-full">
                    <SelectValue placeholder="whenUnsatisfiable" />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value={SELECT_NONE}>(unset)</SelectItem>
                    {WHEN_UNSATISFIABLE.map((w) => (
                      <SelectItem key={w} value={w}>
                        {w}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              <Button
                type="button"
                variant="ghost"
                size="icon"
                className="shrink-0 text-muted-foreground"
                onClick={() =>
                  setTopology(form.topologySpread.filter((_, j) => j !== i))
                }
                aria-label="Remove constraint"
              >
                <Trash2 className="h-4 w-4" />
              </Button>
            </div>
            <div className="space-y-1.5">
              <Label className="text-xs">matchLabels</Label>
              <KeyValueEditor
                rows={t.matchLabels}
                onChange={(rows) =>
                  setTopology(
                    form.topologySpread.map((r, j) =>
                      j === i ? { ...r, matchLabels: rows } : r,
                    ),
                  )
                }
                keyPlaceholder="app"
                valuePlaceholder="trino"
                addLabel="Add label"
              />
            </div>
          </div>
        ))}
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() =>
            setTopology([
              ...form.topologySpread,
              {
                maxSkew: "",
                topologyKey: "",
                whenUnsatisfiable: "",
                matchLabels: [],
              },
            ])
          }
        >
          <Plus className="h-4 w-4" />
          Add constraint
        </Button>
      </div>

      {/* affinity */}
      <div className="space-y-3 rounded-md border p-3">
        <Label className="text-xs font-medium">Affinity</Label>
        <div className="space-y-2">
          <Label className="text-xs">Node-affinity match expressions</Label>
          {form.affinity.nodeMatch.map((n, i) => (
            <div key={i} className="flex items-start gap-2">
              <div className="grid flex-1 grid-cols-1 gap-2 sm:grid-cols-3">
                <Input
                  value={n.key}
                  placeholder="key"
                  onChange={(e) =>
                    setNodeMatch(
                      form.affinity.nodeMatch.map((r, j) =>
                        j === i ? { ...r, key: e.target.value } : r,
                      ),
                    )
                  }
                />
                <Input
                  value={n.operator}
                  placeholder="In / NotIn / Exists"
                  onChange={(e) =>
                    setNodeMatch(
                      form.affinity.nodeMatch.map((r, j) =>
                        j === i ? { ...r, operator: e.target.value } : r,
                      ),
                    )
                  }
                />
                <Input
                  value={n.values}
                  placeholder="values (comma-separated)"
                  onChange={(e) =>
                    setNodeMatch(
                      form.affinity.nodeMatch.map((r, j) =>
                        j === i ? { ...r, values: e.target.value } : r,
                      ),
                    )
                  }
                />
              </div>
              <Button
                type="button"
                variant="ghost"
                size="icon"
                className="shrink-0 text-muted-foreground"
                onClick={() =>
                  setNodeMatch(form.affinity.nodeMatch.filter((_, j) => j !== i))
                }
                aria-label="Remove requirement"
              >
                <Trash2 className="h-4 w-4" />
              </Button>
            </div>
          ))}
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() =>
              setNodeMatch([
                ...form.affinity.nodeMatch,
                { key: "", operator: "", values: "" },
              ])
            }
          >
            <Plus className="h-4 w-4" />
            Add requirement
          </Button>
        </div>
        <div className="space-y-1.5">
          <Label htmlFor={`${idPrefix}-antiaff`} className="text-xs">
            Pod anti-affinity topology key
          </Label>
          <Input
            id={`${idPrefix}-antiaff`}
            value={form.affinity.podAntiAffinityTopologyKey}
            placeholder="kubernetes.io/hostname"
            onChange={(e) =>
              onChange((prev) => ({
                ...prev,
                affinity: {
                  ...prev.affinity,
                  podAntiAffinityTopologyKey: e.target.value,
                },
              }))
            }
          />
        </div>
        <div className="space-y-1.5">
          <Label htmlFor={`${idPrefix}-affraw`} className="text-xs">
            Raw override (wins verbatim)
          </Label>
          <Textarea
            id={`${idPrefix}-affraw`}
            value={form.affinity.rawOverride}
            rows={4}
            className="font-mono text-xs"
            placeholder='{"nodeAffinity": {}}'
            onChange={(e) =>
              onChange((prev) => ({
                ...prev,
                affinity: { ...prev.affinity, rawOverride: e.target.value },
              }))
            }
          />
          {validateRawJson(form.affinity.rawOverride) && (
            <p className="text-xs text-destructive">
              Invalid JSON: {validateRawJson(form.affinity.rawOverride)}
            </p>
          )}
        </div>
      </div>

      {/* termination grace + extra ports */}
      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
        <div className="space-y-1.5">
          <Label htmlFor={`${idPrefix}-grace`}>terminationGraceSeconds</Label>
          <Input
            id={`${idPrefix}-grace`}
            type="number"
            value={form.terminationGraceSeconds}
            placeholder="60"
            onChange={(e) =>
              onChange((prev) => ({
                ...prev,
                terminationGraceSeconds: e.target.value,
              }))
            }
          />
        </div>
      </div>

      <div className="space-y-2">
        <Label>Additional exposed ports</Label>
        {form.additionalExposedPorts.map((p, i) => (
          <div key={i} className="flex items-start gap-2">
            <div className="grid flex-1 grid-cols-2 gap-2">
              <Input
                value={p.name}
                placeholder="jmx"
                onChange={(e) =>
                  setPorts(
                    form.additionalExposedPorts.map((r, j) =>
                      j === i ? { ...r, name: e.target.value } : r,
                    ),
                  )
                }
              />
              <Input
                type="number"
                value={p.containerPort}
                placeholder="9080"
                onChange={(e) =>
                  setPorts(
                    form.additionalExposedPorts.map((r, j) =>
                      j === i ? { ...r, containerPort: e.target.value } : r,
                    ),
                  )
                }
              />
            </div>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="shrink-0 text-muted-foreground"
              onClick={() =>
                setPorts(form.additionalExposedPorts.filter((_, j) => j !== i))
              }
              aria-label="Remove port"
            >
              <Trash2 className="h-4 w-4" />
            </Button>
          </div>
        ))}
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() =>
            setPorts([...form.additionalExposedPorts, { name: "", containerPort: "" }])
          }
        >
          <Plus className="h-4 w-4" />
          Add port
        </Button>
      </div>
    </div>
  );
}

// ── section props ────────────────────────────────────────────────────────────

export interface PodSchedulingSectionProps {
  // Per-role placement (coordinator + worker `RoleForm` slices from the hook).
  coordinator: RoleForm;
  setCoordinator: Dispatch<SetStateAction<RoleForm>>;
  worker: RoleForm;
  setWorker: Dispatch<SetStateAction<RoleForm>>;
  // Cluster-wide init / sidecar containers.
  initContainers: MiniContainer[];
  setInitContainers: Dispatch<SetStateAction<MiniContainer[]>>;
  sidecarContainers: MiniContainer[];
  setSidecarContainers: Dispatch<SetStateAction<MiniContainer[]>>;
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
    coordinator,
    setCoordinator,
    worker,
    setWorker,
    initContainers,
    setInitContainers,
    sidecarContainers,
    setSidecarContainers,
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

  // Per-role affinity raw-JSON is stored as text in the form; validate BOTH
  // roles centrally so an invalid override blocks Save regardless of which role
  // tab is currently visible.
  useEffect(() => {
    const coordErr = validateRawJson(coordinator.affinity.rawOverride);
    const workerErr = validateRawJson(worker.affinity.rawOverride);
    setRawErrors((prev) =>
      prev.coordAffinity === coordErr && prev.workerAffinity === workerErr
        ? prev
        : { ...prev, coordAffinity: coordErr, workerAffinity: workerErr },
    );
  }, [coordinator.affinity.rawOverride, worker.affinity.rawOverride]);

  const [role, setRole] = useState<"coordinator" | "worker">("coordinator");

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

      {/* Init / sidecar containers */}
      <div className="space-y-2">
        <Label>Init containers</Label>
        <p className="text-xs text-muted-foreground">
          Run to completion before the Trino container starts (both roles).
        </p>
        <MiniContainerList
          idPrefix="init"
          items={initContainers}
          onChange={setInitContainers}
          onRawError={setRawError}
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
          onRawError={setRawError}
          addLabel="Add sidecar container"
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

      {/* Per-role placement (coordinator / worker) */}
      <div className="space-y-3 border-t pt-6">
        <div className="space-y-1">
          <Label className="text-base font-medium">Per-role placement</Label>
          <p className="text-xs text-muted-foreground">
            Node selector, tolerations, topology spread & affinity applied to the
            selected role's Deployment.
          </p>
        </div>
        <Tabs value={role} onValueChange={(v) => setRole(v as typeof role)}>
          <TabsList>
            <TabsTrigger value="coordinator">Coordinator</TabsTrigger>
            <TabsTrigger value="worker">Worker</TabsTrigger>
          </TabsList>
          <TabsContent value="coordinator" className="pt-4">
            <RolePlacementEditor
              idPrefix="ps-coord"
              form={coordinator}
              onChange={setCoordinator}
            />
          </TabsContent>
          <TabsContent value="worker" className="pt-4">
            <RolePlacementEditor
              idPrefix="ps-worker"
              form={worker}
              onChange={setWorker}
            />
          </TabsContent>
        </Tabs>
      </div>
    </div>
  );
}
