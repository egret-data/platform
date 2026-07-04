import { useState } from "react";
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
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { KeyValueEditor } from "../EngineResourceGroups";
import type {
  ContainerPortForm,
  NodeMatchForm,
  RoleForm,
  TolerationForm,
  TopologySpreadForm,
} from "../useEngineConfigForm";
import { ConfigSection } from "./ConfigSection";
import { RawJsonField } from "./RawJsonField";
import { SELECT_NONE } from "./podEditors";

// ── raw-JSON text ⇄ parsed bridge ─────────────────────────────────────────────
// The role form stores the affinity raw override as JSON *text*; the shared
// `RawJsonField` works in *parsed* values, so convert at the boundary (the built
// payload stays byte-identical).

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

// ── per-role placement editor ────────────────────────────────────────────────

const TOLERATION_EFFECTS = ["NoSchedule", "PreferNoSchedule", "NoExecute"];
const WHEN_UNSATISFIABLE = ["DoNotSchedule", "ScheduleAnyway"];

/**
 * The per-role placement fields (nodeSelector / tolerations / topologySpread /
 * affinity / termination grace / extra ports) editing one role's `RoleForm`
 * slice. The affinity raw-JSON override is text-in-form; it is bridged to the
 * shared `RawJsonField` (which reports its own validity to the surrounding
 * provider). Both roles stay mounted so an invalid override in either role
 * blocks Save regardless of the selected tab.
 */
function RolePlacementEditor({
  idPrefix,
  rawId,
  form,
  onChange,
}: {
  idPrefix: string;
  rawId: string;
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
        <RawJsonField
          id={rawId}
          label="Raw override (wins verbatim)"
          value={textToRaw(form.affinity.rawOverride)}
          onChange={(v) =>
            onChange((prev) => ({
              ...prev,
              affinity: { ...prev.affinity, rawOverride: rawToText(v) },
            }))
          }
          placeholder='{"nodeAffinity": {}}'
        />
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

export interface PlacementSectionProps {
  coordinator: RoleForm;
  setCoordinator: Dispatch<SetStateAction<RoleForm>>;
  worker: RoleForm;
  setWorker: Dispatch<SetStateAction<RoleForm>>;
}

/**
 * "Placement" config: per-role (coordinator / worker) node selector, tolerations,
 * topology spread, affinity, termination grace, and extra ports. A tab toggles
 * which role is visible, but both role editors stay mounted so an invalid
 * affinity raw override in either role blocks Save.
 */
export function PlacementSection({
  coordinator,
  setCoordinator,
  worker,
  setWorker,
}: PlacementSectionProps) {
  const [role, setRole] = useState<"coordinator" | "worker">("coordinator");

  return (
    <ConfigSection
      title="Per-role placement"
      description="Node selector, tolerations, topology spread & affinity applied to the selected role's Deployment."
    >
      <Tabs value={role} onValueChange={(v) => setRole(v as typeof role)}>
        <TabsList>
          <TabsTrigger value="coordinator">Coordinator</TabsTrigger>
          <TabsTrigger value="worker">Worker</TabsTrigger>
        </TabsList>
      </Tabs>
      {/* Both roles stay mounted (only the active one is shown) so both
          affinity raw-override fields register their validity. */}
      <div className={role === "coordinator" ? "pt-4" : "hidden"}>
        <RolePlacementEditor
          idPrefix="ps-coord"
          rawId="placement.coordinator.affinity.raw"
          form={coordinator}
          onChange={setCoordinator}
        />
      </div>
      <div className={role === "worker" ? "pt-4" : "hidden"}>
        <RolePlacementEditor
          idPrefix="ps-worker"
          rawId="placement.worker.affinity.raw"
          form={worker}
          onChange={setWorker}
        />
      </div>
    </ConfigSection>
  );
}
