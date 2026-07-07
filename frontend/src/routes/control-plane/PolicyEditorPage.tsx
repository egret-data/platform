import { useEffect, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, Loader2, Plus, Trash2, X } from "lucide-react";

import {
  type AccessItem,
  type MaskItem,
  type ResourceLevel,
  type RowFilterItem,
  type ServiceDef,
  type ServiceDefResource,
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
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { MaskItemTable } from "@/components/policy/MaskItemTable";
import { PermissionTable } from "@/components/policy/PermissionTable";
import { ResourceSelector } from "@/components/policy/ResourceSelector";
import { RowFilterItemTable } from "@/components/policy/RowFilterItemTable";
import { ConfigSection } from "./config/ConfigSection";
import { FieldLabel } from "./config/FieldLabel";

type PolicyKind = "access" | "masking" | "rowfilter";

const DEFAULT_ACCESS_TYPES = ["select", "insert", "delete", "update", "create", "drop"];

const emptyLevel = () => ({ values: [] as string[], isExcludes: false, isRecursive: false });
const wildcardLevel = () => ({
  values: ["*"] as string[],
  isExcludes: false,
  isRecursive: false,
});
const emptyAccessItem = (effect: "allow" | "deny" = "allow"): AccessItem => ({
  users: [],
  groups: [],
  roles: [],
  accesses: [],
  effect,
  delegateAdmin: false,
});
const emptyMaskItem = (): MaskItem => ({
  users: [],
  groups: [],
  maskType: "MASK_HASH",
  valueExpr: undefined,
});
const emptyRowFilterItem = (): RowFilterItem => ({ users: [], groups: [], filterExpr: "" });
const emptyTableResources = () => ({
  catalog: emptyLevel(),
  schema: wildcardLevel(),
  table: wildcardLevel(),
  column: wildcardLevel(),
});

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
      resources: emptyTableResources(),
      items: [emptyMaskItem()],
      description: "",
      labels: [],
      isOverride: false,
    };
  }
  if (kind === "rowfilter") {
    return {
      kind: "rowfilter",
      name: "",
      service,
      isEnabled: true,
      resources: { catalog: emptyLevel(), schema: wildcardLevel(), table: wildcardLevel() },
      items: [emptyRowFilterItem()],
      description: "",
      labels: [],
      isOverride: false,
    };
  }
  return {
    kind: "access",
    name: "",
    service,
    isEnabled: true,
    resources: emptyTableResources(),
    additional: [],
    items: [emptyAccessItem()],
    allowExceptions: [],
    denyExceptions: [],
    isDenyAllElse: false,
    description: "",
    labels: [],
    isOverride: false,
  };
}

/**
 * Reverse-maps a raw Ranger policy body (as returned by `getPolicy`) back into
 * the editor's form shape. Inverse of the backend's `*_to_ranger_json`
 * serializers — ported from `Policies.tsx`'s `toInput`/`maskingToInput`/
 * `rowfilterToInput`, unified behind `kind` instead of three call sites.
 */
function rawResourcesToMap(obj: Record<string, any> | undefined | null): Record<string, ResourceLevel> {
  return Object.fromEntries(
    Object.entries(obj ?? {}).map(([k, v]) => [
      k,
      { values: v?.values ?? [], isExcludes: !!v?.isExcludes, isRecursive: !!v?.isRecursive },
    ]),
  );
}

function toFormState(kind: PolicyKind, service: string, body: Record<string, any>): any {
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
      resources: rawResourcesToMap(body.resources),
      items: items.length ? items : [emptyMaskItem()],
      description: String(body.description ?? ""),
      labels: body.labels ?? [],
      isOverride: body.policyPriority === 1,
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
      resources: rawResourcesToMap(body.resources),
      items: items.length ? items : [emptyRowFilterItem()],
      description: String(body.description ?? ""),
      labels: body.labels ?? [],
      isOverride: body.policyPriority === 1,
    };
  }

  const map = (arr: any[], effect: "allow" | "deny") =>
    (arr ?? []).map((it: any) => ({
      users: it.users ?? [],
      groups: it.groups ?? [],
      roles: it.roles ?? [],
      accesses: (it.accesses ?? []).map((a: any) => a.type),
      delegateAdmin: !!it.delegateAdmin,
      effect,
    }));
  const items = [...map(body.policyItems, "allow"), ...map(body.denyPolicyItems, "deny")];
  const additional = (body.additionalResources ?? []).map(rawResourcesToMap);
  return {
    kind: "access",
    name: String(body.name ?? ""),
    service,
    isEnabled: body.isEnabled !== false,
    resources: rawResourcesToMap(body.resources),
    additional,
    items: items.length ? items : [emptyAccessItem()],
    allowExceptions: map(body.allowExceptions, "allow"),
    denyExceptions: map(body.denyExceptions, "deny"),
    isDenyAllElse: body.isDenyAllElse === true,
    description: String(body.description ?? ""),
    labels: body.policyLabels ?? [],
    isOverride: body.policyPriority === 1,
  };
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

  const def = useQuery<ServiceDef>({
    queryKey: ["service-def", service],
    queryFn: () => getServiceDef(service),
    retry: false,
  });

  // Applicable resource-type defs for the current policy kind: access uses
  // the top-level hierarchy, masking/rowfilter use their own (Ranger keeps
  // separate resource defs per policy kind — e.g. masking's may omit levels
  // access allows, or vice versa).
  const resDefs: ServiceDefResource[] =
    kind === "masking"
      ? (def.data?.dataMaskDef?.resources ?? [])
      : kind === "rowfilter"
        ? (def.data?.rowFilterDef?.resources ?? [])
        : (def.data?.resources ?? []);
  const resDefByName = (name: string): ServiceDefResource | undefined =>
    resDefs.find((r) => r.name === name);

  // The selected chain's leaf resource type (reported by ResourceSelector),
  // used to restrict which access types are offered — e.g. a `trinouser`
  // leaf only allows "impersonate", a `role` leaf only its own restrictions.
  const [leaf, setLeaf] = useState<string | null>(null);
  const leafDef = leaf ? resDefByName(leaf) : undefined;
  const accessTypes = leafDef?.accessTypeRestrictions?.length
    ? leafDef.accessTypeRestrictions
    : (def.data?.accessTypes?.map((a) => a.name) ?? DEFAULT_ACCESS_TYPES);

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

  const addLabel = () => {
    const t = labelInput.trim();
    if (t && !form.labels.includes(t)) setForm((f: any) => ({ ...f, labels: [...f.labels, t] }));
    setLabelInput("");
  };
  const removeLabel = (l: string) =>
    setForm((f: any) => ({ ...f, labels: f.labels.filter((x: string) => x !== l) }));

  // Any resource entry the def marks mandatory but that's still empty —
  // blocks Save. (Per-field "Required" hint is rendered by ResourceSelector
  // itself via ResourceLevelEditor's `invalid` prop.)
  const missingRequired: boolean = Object.entries(form.resources as Record<string, ResourceLevel>).some(
    ([name, lvl]) => resDefByName(name)?.mandatory && lvl.values.length === 0,
  );

  const kindTitle =
    kind === "masking" ? "Masking" : kind === "rowfilter" ? "Row filter" : "Permissions";

  const TABLE_LEVELS = ["catalog", "schema", "table", "column"];

  // At-a-glance summary of the resource being configured, e.g. `iceberg.sales.*.*`
  // for a catalog/schema/table/column chain, or `role: *` / `trinouser: *`
  // for a single non-table resource.
  const resourcePreview = (() => {
    const keys = Object.keys(form.resources);
    if (keys.length === 0) return "…";
    if (keys.every((k) => TABLE_LEVELS.includes(k))) {
      return TABLE_LEVELS.filter((l) => keys.includes(l))
        .map((l) => form.resources[l]?.values[0] || "…")
        .join(".");
    }
    if (keys.length === 1) {
      const k = keys[0];
      return `${k}: ${form.resources[k].values[0] || "…"}`;
    }
    return keys.map((k) => form.resources[k].values[0] || "…").join(".");
  })();

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
  const resourceCount =
    Object.values(form.resources).filter((l: any) => l.values.length > 0).length +
    (kind === "access" ? form.additional.length : 0);
  const rulesCount =
    form.items.length +
    (kind === "access" ? form.allowExceptions.length + form.denyExceptions.length : 0);
  const detailsCount = form.labels.length + (form.description ? 1 : 0);
  const countFor = (key: string): number =>
    key === "resource" ? resourceCount : key === "rules" ? rulesCount : detailsCount;

  // ── Access items (Permissions tab): `form.items` is the single wire array
  // with a per-item `effect`; allow/deny views below are derived filters
  // whose onChange rebuilds `form.items` from the updated subset plus the
  // untouched other side, keeping the `items` serializer wire intact.
  const allowItems: AccessItem[] = kind === "access" ? form.items.filter((it: AccessItem) => it.effect === "allow") : [];
  const denyItems: AccessItem[] = kind === "access" ? form.items.filter((it: AccessItem) => it.effect === "deny") : [];
  const setAllowItems = (next: AccessItem[]) =>
    setForm((f: any) => ({
      ...f,
      items: [...next, ...f.items.filter((it: AccessItem) => it.effect === "deny")],
    }));
  const setDenyItems = (next: AccessItem[]) =>
    setForm((f: any) => ({
      ...f,
      items: [...f.items.filter((it: AccessItem) => it.effect === "allow"), ...next],
    }));

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
              disabled={save.isPending || !form.name.trim() || missingRequired}
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
                  <ResourceSelector
                    resourceDefs={resDefs}
                    service={service}
                    value={form.resources}
                    onChange={(r) => setForm((f: any) => ({ ...f, resources: r }))}
                    onLeafChange={setLeaf}
                  />
                  {missingRequired && (
                    <p className="text-xs text-muted-foreground">
                      Fill the required resource levels.
                    </p>
                  )}
                  {kind === "access" && (
                    <>
                      {form.additional.map((res: Record<string, ResourceLevel>, i: number) => (
                        <div key={i} className="mt-4 flex gap-3 border-t pt-4">
                          <div className="min-w-0 flex-1">
                            <ResourceSelector
                              resourceDefs={resDefs}
                              service={service}
                              value={res}
                              onChange={(r) =>
                                setForm((f: any) => ({
                                  ...f,
                                  additional: f.additional.map((a: any, j: number) =>
                                    j === i ? r : a,
                                  ),
                                }))
                              }
                            />
                          </div>
                          <Button
                            type="button"
                            variant="ghost"
                            size="iconSm"
                            title="Remove resource"
                            aria-label={`remove resource ${i}`}
                            onClick={() =>
                              setForm((f: any) => ({
                                ...f,
                                additional: f.additional.filter(
                                  (_: unknown, j: number) => j !== i,
                                ),
                              }))
                            }
                          >
                            <Trash2 className="h-3.5 w-3.5 text-muted-foreground" />
                          </Button>
                        </div>
                      ))}
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        className="mt-4"
                        onClick={() =>
                          setForm((f: any) => ({
                            ...f,
                            additional: [...f.additional, {}],
                          }))
                        }
                      >
                        <Plus className="mr-1 h-4 w-4" /> Add resource
                      </Button>
                    </>
                  )}
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

                        <Accordion type="multiple" defaultValue={["allow"]}>
                          <AccordionItem value="allow">
                            <AccordionTrigger>Allow conditions</AccordionTrigger>
                            <AccordionContent className="space-y-4">
                              <PermissionTable
                                items={allowItems}
                                onChange={setAllowItems}
                                accessTypes={accessTypes}
                                effect="allow"
                              />
                              <Accordion type="single" collapsible>
                                <AccordionItem value="allow-exceptions">
                                  <AccordionTrigger className="text-sm">
                                    Exclude from allow
                                  </AccordionTrigger>
                                  <AccordionContent>
                                    <PermissionTable
                                      items={form.allowExceptions}
                                      onChange={(next) =>
                                        setForm((f: any) => ({ ...f, allowExceptions: next }))
                                      }
                                      accessTypes={accessTypes}
                                      effect="allow"
                                    />
                                  </AccordionContent>
                                </AccordionItem>
                              </Accordion>
                            </AccordionContent>
                          </AccordionItem>

                          {!form.isDenyAllElse && (
                            <AccordionItem value="deny">
                              <AccordionTrigger>Deny conditions</AccordionTrigger>
                              <AccordionContent className="space-y-4">
                                <PermissionTable
                                  items={denyItems}
                                  onChange={setDenyItems}
                                  accessTypes={accessTypes}
                                  effect="deny"
                                />
                                <Accordion type="single" collapsible>
                                  <AccordionItem value="deny-exceptions">
                                    <AccordionTrigger className="text-sm">
                                      Exclude from deny
                                    </AccordionTrigger>
                                    <AccordionContent>
                                      <PermissionTable
                                        items={form.denyExceptions}
                                        onChange={(next) =>
                                          setForm((f: any) => ({ ...f, denyExceptions: next }))
                                        }
                                        accessTypes={accessTypes}
                                        effect="deny"
                                      />
                                    </AccordionContent>
                                  </AccordionItem>
                                </Accordion>
                              </AccordionContent>
                            </AccordionItem>
                          )}
                        </Accordion>
                      </div>
                    </ConfigSection>
                  )}

                  {kind === "masking" && (
                    <ConfigSection title={kindTitle}>
                      <MaskItemTable
                        items={form.items}
                        onChange={(items) => setForm((f: any) => ({ ...f, items }))}
                      />
                    </ConfigSection>
                  )}

                  {kind === "rowfilter" && (
                    <ConfigSection title={kindTitle}>
                      <RowFilterItemTable
                        items={form.items}
                        onChange={(items) => setForm((f: any) => ({ ...f, items }))}
                      />
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
                      <div className="flex items-center justify-between gap-4">
                        <FieldLabel label="Override" htmlFor="policy-override" />
                        <Switch
                          id="policy-override"
                          aria-label="Override"
                          checked={form.isOverride}
                          onCheckedChange={(c) => setForm((f: any) => ({ ...f, isOverride: c }))}
                        />
                      </div>
                      <p className="text-xs text-muted-foreground">
                        This policy&apos;s allow/deny wins over conflicting normal policies.
                      </p>
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
