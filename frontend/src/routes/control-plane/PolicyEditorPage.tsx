import { useEffect, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, Loader2, Plus, Trash2, X } from "lucide-react";

import {
  type AccessItem,
  type MaskItem,
  type RowFilterItem,
  type ServiceDef,
  MASK_TYPES,
  createPolicy,
  deletePolicy,
  getPolicy,
  getServiceDef,
  updatePolicy,
} from "@/api/policies";
import { ApiError } from "@/api/client";
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
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
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { PrincipalPicker } from "@/components/policy/PrincipalPicker";
import { ResourceLevelEditor } from "@/components/policy/ResourceLevelEditor";
import { ConfigSection } from "./config/ConfigSection";
import { FieldLabel } from "./config/FieldLabel";

type PolicyKind = "access" | "masking" | "rowfilter";
type ExceptionKey = "allowExceptions" | "denyExceptions";

const DEFAULT_ACCESS_TYPES = ["select", "insert", "delete", "update", "create", "drop"];

// Clusters the Trino access types into labeled groups so the access-item
// checkbox grid reads as sections instead of one flat wrapping list. Any
// service-def access type not covered here (and not "all", which gets its
// own full-width toggle) falls back into a trailing "Other" bucket — see
// `AccessItemRow`.
const ACCESS_TYPE_GROUPS: { label: string; types: string[] }[] = [
  { label: "Read", types: ["select", "show", "read_sysinfo"] },
  { label: "Write", types: ["insert", "update", "delete", "write_sysinfo"] },
  { label: "DDL", types: ["create", "drop", "alter"] },
  { label: "Grant", types: ["grant", "revoke"] },
  { label: "Other", types: ["use", "impersonate", "execute"] },
];
const ALL_GROUPED_ACCESS_TYPES = new Set(ACCESS_TYPE_GROUPS.flatMap((g) => g.types));

const emptyLevel = () => ({ values: [] as string[], isExcludes: false, isRecursive: false });
const emptyAccessItem = (effect: "allow" | "deny" = "allow"): AccessItem => ({
  users: [],
  groups: [],
  accesses: [],
  effect,
});
const emptyMaskItem = (): MaskItem => ({
  users: [],
  groups: [],
  maskType: "MASK_HASH",
  valueExpr: undefined,
});
const emptyRowFilterItem = (): RowFilterItem => ({ users: [], groups: [], filterExpr: "" });

/**
 * Builds a fresh form for a brand-new policy of the given kind. Mirrors the
 * three dialogs' `blankInput` helpers (`AccessPolicyDialog`,
 * `MaskingPolicyDialog`, `RowFilterPolicyDialog`), keyed by `kind` instead of
 * one function per component.
 */
function blankInput(kind: PolicyKind, service: string): any {
  if (kind === "masking") {
    return {
      kind: "masking",
      name: "",
      service,
      isEnabled: true,
      resources: {
        catalog: emptyLevel(),
        schema: emptyLevel(),
        table: emptyLevel(),
        column: emptyLevel(),
      },
      items: [emptyMaskItem()],
      description: "",
      labels: [],
    };
  }
  if (kind === "rowfilter") {
    return {
      kind: "rowfilter",
      name: "",
      service,
      isEnabled: true,
      resources: { catalog: emptyLevel(), schema: emptyLevel(), table: emptyLevel() },
      items: [emptyRowFilterItem()],
      description: "",
      labels: [],
    };
  }
  return {
    kind: "access",
    name: "",
    service,
    isEnabled: true,
    resources: {
      catalog: emptyLevel(),
      schema: emptyLevel(),
      table: emptyLevel(),
      column: emptyLevel(),
    },
    items: [emptyAccessItem()],
    allowExceptions: [],
    denyExceptions: [],
    isDenyAllElse: false,
    description: "",
    labels: [],
  };
}

/**
 * Reverse-maps a raw Ranger policy body (as returned by `getPolicy`) back into
 * the editor's form shape. Inverse of the backend's `*_to_ranger_json`
 * serializers — ported from `Policies.tsx`'s `toInput`/`maskingToInput`/
 * `rowfilterToInput`, unified behind `kind` instead of three call sites.
 */
function toFormState(kind: PolicyKind, service: string, body: Record<string, any>): any {
  const lvl = (k: string) => ({
    values: body?.resources?.[k]?.values ?? [],
    isExcludes: !!body?.resources?.[k]?.isExcludes,
    isRecursive: !!body?.resources?.[k]?.isRecursive,
  });

  if (kind === "masking") {
    const items = (body.dataMaskPolicyItems ?? []).map((it: any) => ({
      users: it.users ?? [],
      groups: it.groups ?? [],
      maskType: it.dataMaskInfo?.dataMaskType ?? "MASK_HASH",
      valueExpr: it.dataMaskInfo?.valueExpr,
    }));
    return {
      kind: "masking",
      name: String(body.name ?? ""),
      service,
      isEnabled: body.isEnabled !== false,
      resources: {
        catalog: lvl("catalog"),
        schema: lvl("schema"),
        table: lvl("table"),
        column: lvl("column"),
      },
      items: items.length ? items : [emptyMaskItem()],
      description: String(body.description ?? ""),
      labels: body.labels ?? [],
    };
  }

  if (kind === "rowfilter") {
    const items = (body.rowFilterPolicyItems ?? []).map((it: any) => ({
      users: it.users ?? [],
      groups: it.groups ?? [],
      filterExpr: it.rowFilterInfo?.filterExpr ?? "",
    }));
    return {
      kind: "rowfilter",
      name: String(body.name ?? ""),
      service,
      isEnabled: body.isEnabled !== false,
      resources: { catalog: lvl("catalog"), schema: lvl("schema"), table: lvl("table") },
      items: items.length ? items : [emptyRowFilterItem()],
      description: String(body.description ?? ""),
      labels: body.labels ?? [],
    };
  }

  const map = (arr: any[], effect: "allow" | "deny") =>
    (arr ?? []).map((it: any) => ({
      users: it.users ?? [],
      groups: it.groups ?? [],
      accesses: (it.accesses ?? []).map((a: any) => a.type),
      effect,
    }));
  const items = [...map(body.policyItems, "allow"), ...map(body.denyPolicyItems, "deny")];
  return {
    kind: "access",
    name: String(body.name ?? ""),
    service,
    isEnabled: body.isEnabled !== false,
    resources: {
      catalog: lvl("catalog"),
      schema: lvl("schema"),
      table: lvl("table"),
      column: lvl("column"),
    },
    items: items.length ? items : [emptyAccessItem()],
    allowExceptions: map(body.allowExceptions, "allow"),
    denyExceptions: map(body.denyExceptions, "deny"),
    isDenyAllElse: body.isDenyAllElse === true,
    description: String(body.description ?? ""),
    labels: body.policyLabels ?? [],
  };
}

interface AccessItemRowProps {
  item: AccessItem;
  accessTypes: string[];
  removeLabel: string;
  onChange: (next: Partial<AccessItem>) => void;
  onRemove: () => void;
}

function AccessItemRow({
  item,
  accessTypes,
  removeLabel,
  onChange,
  onRemove,
}: AccessItemRowProps) {
  const toggleAccess = (a: string, c: boolean) =>
    onChange({
      accesses: c ? [...item.accesses, a] : item.accesses.filter((x) => x !== a),
    });

  // Intersect each group's types with what the service def actually exposes
  // (skip empty groups), then bucket any def types not covered by any group
  // into a trailing "Other" section. "all" (grant-everything) is rendered
  // separately below, not as part of any group.
  const groups = ACCESS_TYPE_GROUPS.map((g) => ({
    label: g.label,
    types: g.types.filter((t) => accessTypes.includes(t)),
  })).filter((g) => g.types.length > 0);

  const ungrouped = accessTypes.filter(
    (t) => t !== "all" && !ALL_GROUPED_ACCESS_TYPES.has(t),
  );
  if (ungrouped.length > 0) {
    const other = groups.find((g) => g.label === "Other");
    if (other) other.types = [...other.types, ...ungrouped];
    else groups.push({ label: "Other", types: ungrouped });
  }

  const hasAll = accessTypes.includes("all");

  return (
    <div className="space-y-2 rounded-md border p-3">
      <div className="flex items-start justify-between gap-2">
        <div className="flex-1">
          <PrincipalPicker
            users={item.users}
            groups={item.groups}
            onChange={(p) => onChange(p)}
          />
        </div>
        <button type="button" aria-label={removeLabel} onClick={onRemove}>
          <Trash2 className="h-4 w-4 text-muted-foreground" />
        </button>
      </div>
      <div className="space-y-2">
        {groups.map((g) => (
          <div key={g.label}>
            <span className="text-xs text-muted-foreground">{g.label}</span>
            <div className="flex flex-wrap gap-3">
              {g.types.map((a) => (
                <label key={a} className="flex items-center gap-1.5 text-sm">
                  <Checkbox
                    aria-label={a}
                    checked={item.accesses.includes(a)}
                    onCheckedChange={(c) => toggleAccess(a, c === true)}
                  />
                  {a}
                </label>
              ))}
            </div>
          </div>
        ))}
        {hasAll && (
          <label className="flex w-full items-center gap-1.5 text-sm">
            <Checkbox
              aria-label="all"
              checked={item.accesses.includes("all")}
              onCheckedChange={(c) => toggleAccess("all", c === true)}
            />
            All (grant everything)
          </label>
        )}
      </div>
    </div>
  );
}

/**
 * Control Plane → full-page policy editor. Replaces the three modal dialogs
 * (`AccessPolicyDialog`/`MaskingPolicyDialog`/`RowFilterPolicyDialog`) with a
 * single Engine-style page handling all three policy kinds — create + edit.
 *
 * Route params: `service`, `kind` ("access"|"masking"|"rowfilter"), and an
 * optional `id` (present = editing an existing policy).
 */
export default function PolicyEditorPage() {
  const params = useParams<{ service: string; kind: string; id?: string }>();
  const service = params.service ?? "";
  const kind = (params.kind as PolicyKind) ?? "access";
  const id = params.id;
  const navigate = useNavigate();
  const queryClient = useQueryClient();

  const [form, setForm] = useState<any>(() => blankInput(kind, service));
  const [tab, setTab] = useState<"allow" | "deny">("allow");
  const [labelInput, setLabelInput] = useState("");
  const [deleteOpen, setDeleteOpen] = useState(false);

  // Outer left-nav section (Resource / Permissions|Masking|Row filter /
  // Details) — mirrors EngineDetail's `ConfigurationTab` vertical nav.
  const [section, setSection] = useState("resource");

  const policyQuery = useQuery<Record<string, unknown>, ApiError>({
    queryKey: ["control-plane", "policy", id],
    queryFn: () => getPolicy(Number(id)),
    enabled: !!id,
    retry: false,
  });

  useEffect(() => {
    if (id && policyQuery.data) setForm(toFormState(kind, service, policyQuery.data));
  }, [id, policyQuery.data, kind, service]);

  useEffect(() => {
    if (form.isDenyAllElse && tab === "deny") setTab("allow");
  }, [form.isDenyAllElse, tab]);

  const def = useQuery<ServiceDef>({
    queryKey: ["service-def", service],
    queryFn: () => getServiceDef(service),
    enabled: kind === "access",
    retry: false,
  });
  const accessTypes = def.data?.accessTypes?.map((a) => a.name) ?? DEFAULT_ACCESS_TYPES;

  const profileUrl = `/control-plane/policies/${encodeURIComponent(service)}`;

  const save = useMutation<{ id: number }, ApiError>({
    mutationFn: () => (id ? updatePolicy(Number(id), form) : createPolicy(form)),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["control-plane", "policies"] });
      navigate(profileUrl);
    },
  });

  const del = useMutation<void, ApiError>({
    mutationFn: () => deletePolicy(Number(id)),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["control-plane", "policies"] });
      navigate(profileUrl);
    },
  });

  // ── Items (shared shape across all three kinds — masking/rowfilter items
  // just carry fewer fields than AccessItem) ─────────────────────────────────
  const setItem = (i: number, next: Record<string, unknown>) =>
    setForm((f: any) => ({
      ...f,
      items: f.items.map((it: any, j: number) => (j === i ? { ...it, ...next } : it)),
    }));
  const addItem = (effect?: "allow" | "deny") =>
    setForm((f: any) => ({
      ...f,
      items: [
        ...f.items,
        kind === "masking"
          ? emptyMaskItem()
          : kind === "rowfilter"
            ? emptyRowFilterItem()
            : emptyAccessItem(effect ?? "allow"),
      ],
    }));
  const removeItem = (i: number) =>
    setForm((f: any) => ({ ...f, items: f.items.filter((_: unknown, j: number) => j !== i) }));

  // ── Exceptions (access only) ───────────────────────────────────────────────
  const setException = (key: ExceptionKey, i: number, next: Partial<AccessItem>) =>
    setForm((f: any) => ({
      ...f,
      [key]: f[key].map((it: AccessItem, j: number) => (j === i ? { ...it, ...next } : it)),
    }));
  const addException = (key: ExceptionKey, effect: "allow" | "deny") =>
    setForm((f: any) => ({ ...f, [key]: [...f[key], emptyAccessItem(effect)] }));
  const removeException = (key: ExceptionKey, i: number) =>
    setForm((f: any) => ({
      ...f,
      [key]: f[key].filter((_: unknown, j: number) => j !== i),
    }));

  const addLabel = () => {
    const t = labelInput.trim();
    if (t && !form.labels.includes(t)) setForm((f: any) => ({ ...f, labels: [...f.labels, t] }));
    setLabelInput("");
  };
  const removeLabel = (l: string) =>
    setForm((f: any) => ({ ...f, labels: f.labels.filter((x: string) => x !== l) }));

  const allowItems: { it: AccessItem; i: number }[] =
    kind === "access"
      ? form.items
          .map((it: AccessItem, i: number) => ({ it, i }))
          .filter((x: { it: AccessItem }) => x.it.effect === "allow")
      : [];
  const denyItems: { it: AccessItem; i: number }[] =
    kind === "access"
      ? form.items
          .map((it: AccessItem, i: number) => ({ it, i }))
          .filter((x: { it: AccessItem }) => x.it.effect === "deny")
      : [];

  const levels =
    kind === "rowfilter"
      ? (["catalog", "schema", "table"] as const)
      : (["catalog", "schema", "table", "column"] as const);

  const kindTitle =
    kind === "masking" ? "Masking" : kind === "rowfilter" ? "Row filter" : "Permissions";

  // At-a-glance summary of the resource path being configured, e.g.
  // `iceberg.sales.*.*` — joins each level's first value (or a placeholder).
  const resourcePreview = levels
    .map((lvl) => form.resources[lvl].values[0] || "…")
    .join(".");

  // Left-nav sections (vertical layout — mirrors EngineDetail's
  // `ConfigurationTab`). The middle section's key is always "rules"; only its
  // label changes per policy kind.
  const NAV_ITEMS: { key: string; label: string }[] = [
    { key: "resource", label: "Resource" },
    { key: "rules", label: kindTitle },
    { key: "details", label: "Details" },
  ];

  // Non-default field counts per section, driving the nav badges (same idea
  // as engine config's `sectionCounts`).
  const resourceCount = Object.values(form.resources).filter(
    (l: any) => l.values.length > 0,
  ).length;
  const rulesCount =
    form.items.length +
    (kind === "access" ? form.allowExceptions.length + form.denyExceptions.length : 0);
  const detailsCount = form.labels.length + (form.description ? 1 : 0);
  const countFor = (key: string): number =>
    key === "resource" ? resourceCount : key === "rules" ? rulesCount : detailsCount;

  return (
    <div className="space-y-5">
      <Link
        to={profileUrl}
        className="inline-flex items-center gap-1 text-sm text-muted-foreground transition-colors hover:text-foreground"
      >
        <ArrowLeft className="h-4 w-4" />
        {service || "Policies"}
      </Link>

      <Card className="overflow-hidden">
        <div className="flex flex-wrap items-center justify-between gap-3 border-b bg-muted/30 p-5">
          <div className="flex min-w-0 flex-wrap items-center gap-3">
            <div className="space-y-1.5">
              <Label htmlFor="policy-name">Policy name</Label>
              <Input
                id="policy-name"
                aria-label="Policy name"
                value={form.name}
                placeholder="Policy name"
                onChange={(e) => setForm((f: any) => ({ ...f, name: e.target.value }))}
                className="h-9 max-w-sm text-base font-semibold"
              />
            </div>
            <Badge variant="secondary">{kind}</Badge>
            <div className="flex items-center gap-2">
              <Switch
                aria-label="Enabled"
                checked={form.isEnabled}
                onCheckedChange={(c) => setForm((f: any) => ({ ...f, isEnabled: c }))}
              />
              <span className="text-sm text-muted-foreground">Enabled</span>
            </div>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            {id && (
              <Button
                type="button"
                variant="ghost"
                size="sm"
                className="text-muted-foreground"
                onClick={() => setDeleteOpen(true)}
              >
                <Trash2 className="h-4 w-4" />
                Delete
              </Button>
            )}
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => navigate(profileUrl)}
            >
              Cancel
            </Button>
            <Button
              type="button"
              size="sm"
              onClick={() => save.mutate()}
              disabled={save.isPending || !form.name.trim()}
            >
              {save.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
              Save
            </Button>
          </div>
        </div>
      </Card>

      {save.isError && (
        <Alert variant="destructive">
          <AlertDescription>{save.error?.message ?? "Failed to save policy."}</AlertDescription>
        </Alert>
      )}

      <Card>
        <CardContent className="pt-6">
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

            <div className="min-w-0 flex-1">
              {section === "resource" && (
                <ConfigSection
                  title="Resource"
                  description="The Trino resource this policy applies to. Use * for wildcards."
                >
                  <p className="font-mono text-xs text-muted-foreground">{resourcePreview}</p>
                  <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
                    {levels.map((lvl) => (
                      <ResourceLevelEditor
                        key={lvl}
                        label={lvl[0].toUpperCase() + lvl.slice(1)}
                        level={lvl}
                        service={service}
                        parents={{
                          catalog: form.resources.catalog.values[0],
                          schema: form.resources.schema.values[0],
                          table: form.resources.table.values[0],
                        }}
                        value={form.resources[lvl]}
                        onChange={(v) =>
                          setForm((f: any) => ({ ...f, resources: { ...f.resources, [lvl]: v } }))
                        }
                      />
                    ))}
                  </div>
                </ConfigSection>
              )}

              {section === "rules" && (
                <>
                  {kind === "access" && (
                    <ConfigSection title={kindTitle}>
                      <div className="space-y-4">
                        <div className="flex items-center justify-between gap-4">
                          <FieldLabel
                            label="Deny everything else not explicitly allowed"
                            htmlFor="deny-all-else"
                          />
                          <Switch
                            id="deny-all-else"
                            aria-label="Deny all else"
                            checked={form.isDenyAllElse}
                            onCheckedChange={(c) => setForm((f: any) => ({ ...f, isDenyAllElse: c }))}
                          />
                        </div>

                        <Tabs value={tab} onValueChange={(v) => setTab(v as "allow" | "deny")}>
                          <TabsList>
                            <TabsTrigger value="allow">Allow</TabsTrigger>
                            {!form.isDenyAllElse && <TabsTrigger value="deny">Deny</TabsTrigger>}
                          </TabsList>

                          <TabsContent value="allow" className="space-y-3">
                            <div className="flex items-center justify-between">
                              <span className="text-sm font-medium">Allow items</span>
                              <Button
                                type="button"
                                variant="outline"
                                size="sm"
                                onClick={() => addItem("allow")}
                              >
                                <Plus className="mr-1 h-4 w-4" /> Add item
                              </Button>
                            </div>
                            {allowItems.map(({ it, i }) => (
                              <AccessItemRow
                                key={i}
                                item={it}
                                accessTypes={accessTypes}
                                removeLabel={`remove allow item ${i}`}
                                onChange={(next) => setItem(i, next)}
                                onRemove={() => removeItem(i)}
                              />
                            ))}
                            <Accordion type="single" collapsible>
                              <AccordionItem value="exceptions">
                                <AccordionTrigger>Exceptions</AccordionTrigger>
                                <AccordionContent className="space-y-3">
                                  <div className="flex items-center justify-between">
                                    <span className="text-xs text-muted-foreground">
                                      Allow exceptions (deny overrides)
                                    </span>
                                    <Button
                                      type="button"
                                      variant="outline"
                                      size="sm"
                                      onClick={() => addException("allowExceptions", "allow")}
                                    >
                                      <Plus className="mr-1 h-4 w-4" /> Add exception
                                    </Button>
                                  </div>
                                  {form.allowExceptions.map((it: AccessItem, i: number) => (
                                    <AccessItemRow
                                      key={i}
                                      item={it}
                                      accessTypes={accessTypes}
                                      removeLabel={`remove allow exception ${i}`}
                                      onChange={(next) => setException("allowExceptions", i, next)}
                                      onRemove={() => removeException("allowExceptions", i)}
                                    />
                                  ))}
                                </AccordionContent>
                              </AccordionItem>
                            </Accordion>
                          </TabsContent>

                          {!form.isDenyAllElse && (
                            <TabsContent value="deny" className="space-y-3">
                              <div className="flex items-center justify-between">
                                <span className="text-sm font-medium">Deny items</span>
                                <Button
                                  type="button"
                                  variant="outline"
                                  size="sm"
                                  onClick={() => addItem("deny")}
                                >
                                  <Plus className="mr-1 h-4 w-4" /> Add item
                                </Button>
                              </div>
                              {denyItems.map(({ it, i }) => (
                                <AccessItemRow
                                  key={i}
                                  item={it}
                                  accessTypes={accessTypes}
                                  removeLabel={`remove deny item ${i}`}
                                  onChange={(next) => setItem(i, next)}
                                  onRemove={() => removeItem(i)}
                                />
                              ))}
                              <Accordion type="single" collapsible>
                                <AccordionItem value="exceptions">
                                  <AccordionTrigger>Exceptions</AccordionTrigger>
                                  <AccordionContent className="space-y-3">
                                    <div className="flex items-center justify-between">
                                      <span className="text-xs text-muted-foreground">
                                        Deny exceptions (allow overrides)
                                      </span>
                                      <Button
                                        type="button"
                                        variant="outline"
                                        size="sm"
                                        onClick={() => addException("denyExceptions", "deny")}
                                      >
                                        <Plus className="mr-1 h-4 w-4" /> Add exception
                                      </Button>
                                    </div>
                                    {form.denyExceptions.map((it: AccessItem, i: number) => (
                                      <AccessItemRow
                                        key={i}
                                        item={it}
                                        accessTypes={accessTypes}
                                        removeLabel={`remove deny exception ${i}`}
                                        onChange={(next) => setException("denyExceptions", i, next)}
                                        onRemove={() => removeException("denyExceptions", i)}
                                      />
                                    ))}
                                  </AccordionContent>
                                </AccordionItem>
                              </Accordion>
                            </TabsContent>
                          )}
                        </Tabs>
                      </div>
                    </ConfigSection>
                  )}

                  {kind === "masking" && (
                    <ConfigSection title={kindTitle}>
                      <div className="space-y-3">
                        <div className="flex items-center justify-between">
                          <span className="text-sm font-medium">Policy items</span>
                          <Button type="button" variant="outline" size="sm" onClick={() => addItem()}>
                            <Plus className="mr-1 h-4 w-4" /> Add item
                          </Button>
                        </div>
                        {form.items.map((it: MaskItem, i: number) => (
                          <div key={i} className="space-y-2 rounded-md border p-3">
                            <div className="flex items-start justify-between gap-2">
                              <div className="flex-1 space-y-1.5">
                                <span className="text-xs text-muted-foreground">Principals</span>
                                <PrincipalPicker
                                  users={it.users}
                                  groups={it.groups}
                                  onChange={(p) => setItem(i, p)}
                                />
                              </div>
                              {form.items.length > 1 && (
                                <button
                                  type="button"
                                  aria-label={`remove item ${i}`}
                                  onClick={() => removeItem(i)}
                                >
                                  <Trash2 className="h-4 w-4 text-muted-foreground" />
                                </button>
                              )}
                            </div>
                            <div className="space-y-1.5">
                              <FieldLabel label="Mask type" />
                              <Select
                                value={it.maskType}
                                onValueChange={(v) => setItem(i, { maskType: v })}
                              >
                                <SelectTrigger aria-label={`mask type ${i}`} className="w-56">
                                  <SelectValue />
                                </SelectTrigger>
                                <SelectContent>
                                  {MASK_TYPES.map((m) => (
                                    <SelectItem key={m.name} value={m.name}>
                                      {m.label}
                                    </SelectItem>
                                  ))}
                                </SelectContent>
                              </Select>
                            </div>
                            {it.maskType === "CUSTOM" && (
                              <div className="space-y-1.5">
                                <FieldLabel label="Custom expression" />
                                <Input
                                  aria-label={`value expr ${i}`}
                                  placeholder="cast(... {col} ...) as {type}"
                                  value={it.valueExpr ?? ""}
                                  onChange={(e) => setItem(i, { valueExpr: e.target.value })}
                                />
                              </div>
                            )}
                          </div>
                        ))}
                      </div>
                    </ConfigSection>
                  )}

                  {kind === "rowfilter" && (
                    <ConfigSection title={kindTitle}>
                      <div className="space-y-3">
                        <div className="flex items-center justify-between">
                          <span className="text-sm font-medium">Policy items</span>
                          <Button type="button" variant="outline" size="sm" onClick={() => addItem()}>
                            <Plus className="mr-1 h-4 w-4" /> Add item
                          </Button>
                        </div>
                        {form.items.map((it: RowFilterItem, i: number) => (
                          <div key={i} className="space-y-2 rounded-md border p-3">
                            <div className="flex items-start justify-between gap-2">
                              <div className="flex-1 space-y-1.5">
                                <span className="text-xs text-muted-foreground">Principals</span>
                                <PrincipalPicker
                                  users={it.users}
                                  groups={it.groups}
                                  onChange={(p) => setItem(i, p)}
                                />
                              </div>
                              {form.items.length > 1 && (
                                <button
                                  type="button"
                                  aria-label={`remove item ${i}`}
                                  onClick={() => removeItem(i)}
                                >
                                  <Trash2 className="h-4 w-4 text-muted-foreground" />
                                </button>
                              )}
                            </div>
                            <div className="space-y-1.5">
                              <FieldLabel label="Filter expression" />
                              <Input
                                aria-label={`filter ${i}`}
                                placeholder="e.g. country = 'US'"
                                value={it.filterExpr}
                                onChange={(e) => setItem(i, { filterExpr: e.target.value })}
                              />
                            </div>
                          </div>
                        ))}
                      </div>
                    </ConfigSection>
                  )}
                </>
              )}

              {section === "details" && (
                <ConfigSection title="Details">
                  <div className="space-y-4">
                    <div className="space-y-1.5">
                      <FieldLabel label="Description" htmlFor="policy-description" />
                      <Textarea
                        id="policy-description"
                        rows={3}
                        value={form.description}
                        onChange={(e) =>
                          setForm((f: any) => ({ ...f, description: e.target.value }))
                        }
                      />
                    </div>
                    <div className="space-y-1.5">
                      <FieldLabel label="Labels" htmlFor="policy-labels" />
                      {form.labels.length > 0 && (
                        <div className="flex flex-wrap gap-1">
                          {form.labels.map((l: string) => (
                            <Badge key={l} variant="secondary" className="gap-1">
                              {l}
                              <button
                                type="button"
                                aria-label={`remove label ${l}`}
                                onClick={() => removeLabel(l)}
                              >
                                <X className="h-3 w-3" />
                              </button>
                            </Badge>
                          ))}
                        </div>
                      )}
                      <Input
                        id="policy-labels"
                        placeholder="Add label…"
                        value={labelInput}
                        onChange={(e) => setLabelInput(e.target.value)}
                        onKeyDown={(e) => {
                          if (e.key === "Enter" || e.key === ",") {
                            e.preventDefault();
                            addLabel();
                          }
                        }}
                      />
                    </div>
                  </div>
                </ConfigSection>
              )}
            </div>
          </div>
        </CardContent>
      </Card>

      <ConfirmDeleteDialog
        open={deleteOpen}
        onOpenChange={setDeleteOpen}
        title={`Delete "${form.name}"?`}
        description="This policy will be permanently removed from Ranger."
        confirmText="delete"
        onConfirm={() => del.mutate()}
      />
    </div>
  );
}
