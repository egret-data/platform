import { useEffect, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, Loader2, Plus, Trash2, X } from "lucide-react";

import {
  type AccessItem,
  type MaskItem,
  type RowFilterItem,
  type ServiceDef,
  type ServiceDefResource,
  createPolicy,
  deletePolicy,
  getPolicy,
  getServiceDef,
  listPrincipals,
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
import { TagCombobox } from "@/components/ui/tag-combobox";
import { Textarea } from "@/components/ui/textarea";
import { MaskItemTable } from "@/components/policy/MaskItemTable";
import { PermissionTable } from "@/components/policy/PermissionTable";
import { ResourceLevelEditor } from "@/components/policy/ResourceLevelEditor";
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
    resourceMode: "table",
    trinouser: emptyLevel(),
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
function toFormState(kind: PolicyKind, service: string, body: Record<string, any>): any {
  const rawLevel = (raw: any) => ({
    values: raw?.values ?? [],
    isExcludes: !!raw?.isExcludes,
    isRecursive: !!raw?.isRecursive,
  });
  const lvl = (k: string) => rawLevel(body?.resources?.[k]);

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
      resources: { catalog: lvl("catalog"), schema: lvl("schema"), table: lvl("table") },
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
  const isImp = !!body?.resources?.trinouser;
  const additional = (body.additionalResources ?? []).map((r: any) => ({
    catalog: rawLevel(r?.catalog),
    schema: rawLevel(r?.schema),
    table: rawLevel(r?.table),
    column: rawLevel(r?.column),
  }));
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
    additional,
    resourceMode: isImp ? "impersonation" : "table",
    trinouser: lvl("trinouser"),
    items: items.length ? items : [emptyAccessItem()],
    allowExceptions: map(body.allowExceptions, "allow"),
    denyExceptions: map(body.denyExceptions, "deny"),
    isDenyAllElse: body.isDenyAllElse === true,
    description: String(body.description ?? ""),
    labels: body.policyLabels ?? [],
    isOverride: body.policyPriority === 1,
  };
}

interface ResourceBlockProps {
  value: any;
  onChange: (v: any) => void;
  service: string;
  levels: readonly ("catalog" | "schema" | "table" | "column")[];
  levelFlags: (lvl: string) => ServiceDefResource;
  missingRequired?: string[];
}

/**
 * One catalog/schema/table/column resource grid — the primary `resources`
 * block and each `additionalResources` entry both render this, so Ranger's
 * "extra independent resource hierarchy" repeater is just N of these plus a
 * remove button (see the Resource section below).
 */
function ResourceBlock({
  value,
  onChange,
  service,
  levels,
  levelFlags,
  missingRequired = [],
}: ResourceBlockProps) {
  return (
    <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
      {levels.map((lvl) => (
        <ResourceLevelEditor
          key={lvl}
          label={lvl[0].toUpperCase() + lvl.slice(1)}
          level={lvl}
          service={service}
          parents={{
            catalog: value.catalog.values[0],
            schema: value.schema.values[0],
            table: value.table.values[0],
          }}
          value={value[lvl]}
          onChange={(v) => onChange({ ...value, [lvl]: v })}
          excludesSupported={levelFlags(lvl).excludesSupported ?? true}
          recursiveSupported={levelFlags(lvl).recursiveSupported ?? false}
          mandatory={levelFlags(lvl).mandatory ?? false}
          invalid={missingRequired.includes(lvl)}
        />
      ))}
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
    enabled: kind === "access",
    retry: false,
  });
  const accessTypes = def.data?.accessTypes?.map((a) => a.name) ?? DEFAULT_ACCESS_TYPES;

  // Lookup of per-level resource flags (Exclude/Recursive support, mandatory)
  // from the fetched Trino service-def. Before the def loads (or for
  // masking/rowfilter kinds, which don't fetch it), fall back to Trino's
  // known shape: excludesSupported + mandatory on every level, no recursive
  // support anywhere — so the UI is sane even without the def in hand.
  const resFlags = (def.data?.resources ?? []).reduce(
    (m, r) => {
      m[r.name] = r;
      return m;
    },
    {} as Record<string, ServiceDefResource>,
  );
  const levelFlags = (lvl: string): ServiceDefResource =>
    resFlags[lvl] ?? { name: lvl, excludesSupported: true, recursiveSupported: false, mandatory: true };

  const profileUrl = `/control-plane/policies/${encodeURIComponent(service)}`;

  // In impersonation mode, "impersonate" is the only valid access type — force
  // it onto every item/exception at save time regardless of what (if
  // anything) was set on the form, so a stray empty `accesses: []` never
  // reaches Ranger.
  const buildSavePayload = () => {
    if (kind !== "access" || form.resourceMode !== "impersonation") return form;
    const forceImpersonate = (it: AccessItem) => ({ ...it, accesses: ["impersonate"] });
    return {
      ...form,
      items: form.items.map(forceImpersonate),
      allowExceptions: form.allowExceptions.map(forceImpersonate),
      denyExceptions: form.denyExceptions.map(forceImpersonate),
    };
  };

  const save = useMutation<{ id: number }, ApiError>({
    mutationFn: () => {
      const payload = buildSavePayload();
      return id ? updatePolicy(Number(id), payload) : createPolicy(payload);
    },
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

  const levels =
    kind === "rowfilter"
      ? (["catalog", "schema", "table"] as const)
      : (["catalog", "schema", "table", "column"] as const);

  const isImpersonation = kind === "access" && form.resourceMode === "impersonation";
  const isTableResourceMode = kind === "access" && form.resourceMode === "table";

  // Resource levels the current service-def marks mandatory but that are
  // still empty — blocks Save and drives the per-level "Required" state. In
  // impersonation mode the only resource is trinouser, so "missing" collapses
  // to a single synthetic entry when it's empty.
  const missingRequired: string[] = isImpersonation
    ? form.trinouser.values.length === 0
      ? ["trinouser"]
      : []
    : levels.filter((lvl) => levelFlags(lvl).mandatory && form.resources[lvl].values.length === 0);

  const kindTitle =
    kind === "masking" ? "Masking" : kind === "rowfilter" ? "Row filter" : "Permissions";

  // At-a-glance summary of the resource being configured, e.g. `iceberg.sales.*.*`
  // for table mode, or the trinouser value(s) for impersonation mode.
  const resourcePreview = isImpersonation
    ? form.trinouser.values.length > 0
      ? form.trinouser.values.join(", ")
      : "…"
    : levels.map((lvl) => form.resources[lvl].values[0] || "…").join(".");

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
  const resourceCount = isImpersonation
    ? form.trinouser.values.length > 0
      ? 1
      : 0
    : Object.values(form.resources).filter((l: any) => l.values.length > 0).length +
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
              disabled={save.isPending || !form.name.trim() || missingRequired.length > 0}
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
                  {kind === "access" && (
                    <div className="inline-flex w-fit rounded-md border p-0.5">
                      {(
                        [
                          { key: "table", label: "Table access" },
                          { key: "impersonation", label: "User impersonation" },
                        ] as const
                      ).map((opt) => (
                        <button
                          key={opt.key}
                          type="button"
                          onClick={() =>
                            setForm((f: any) => ({ ...f, resourceMode: opt.key }))
                          }
                          className={`rounded px-3 py-1.5 text-sm transition-colors ${
                            form.resourceMode === opt.key
                              ? "bg-muted font-medium"
                              : "text-muted-foreground hover:bg-muted/50"
                          }`}
                        >
                          {opt.label}
                        </button>
                      ))}
                    </div>
                  )}
                  <p className="font-mono text-xs text-muted-foreground">{resourcePreview}</p>
                  {isImpersonation ? (
                    <div className="max-w-sm space-y-1">
                      <label className="text-sm font-medium">Trino user</label>
                      <div
                        className={
                          missingRequired.includes("trinouser")
                            ? "rounded-md ring-1 ring-destructive"
                            : undefined
                        }
                      >
                        <TagCombobox
                          values={form.trinouser.values}
                          onChange={(vals) =>
                            setForm((f: any) => ({
                              ...f,
                              trinouser: { ...f.trinouser, values: vals },
                            }))
                          }
                          suggest={(q) =>
                            listPrincipals(q, "user").then((ps) =>
                              ps.map((p) => ({ value: p.name, label: p.label })),
                            )
                          }
                          queryKey={["principals", "user", "impersonation"]}
                          placeholder="username or * (Enter to add)"
                          creatable
                          ariaLabel="Trino user value"
                        />
                      </div>
                      {missingRequired.includes("trinouser") && (
                        <p className="text-xs text-destructive">Required</p>
                      )}
                      <p className="text-xs text-muted-foreground">
                        Who may be impersonated (a Trino username, or * for any).
                      </p>
                    </div>
                  ) : (
                    <>
                      <ResourceBlock
                        value={form.resources}
                        onChange={(r) => setForm((f: any) => ({ ...f, resources: r }))}
                        service={service}
                        levels={levels}
                        levelFlags={levelFlags}
                        missingRequired={missingRequired}
                      />
                      {missingRequired.length > 0 && (
                        <p className="text-xs text-muted-foreground">
                          Fill the required resource levels.
                        </p>
                      )}
                      {isTableResourceMode && (
                        <>
                          {form.additional.map((res: any, i: number) => (
                            <div key={i} className="mt-4 flex gap-3 border-t pt-4">
                              <div className="min-w-0 flex-1">
                                <ResourceBlock
                                  value={res}
                                  onChange={(r) =>
                                    setForm((f: any) => ({
                                      ...f,
                                      additional: f.additional.map((a: any, j: number) =>
                                        j === i ? r : a,
                                      ),
                                    }))
                                  }
                                  service={service}
                                  levels={levels}
                                  levelFlags={levelFlags}
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
                                additional: [...f.additional, emptyTableResources()],
                              }))
                            }
                          >
                            <Plus className="mr-1 h-4 w-4" /> Add resource
                          </Button>
                        </>
                      )}
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
                                impersonation={isImpersonation}
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
                                      impersonation={isImpersonation}
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
                                  impersonation={isImpersonation}
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
                                        impersonation={isImpersonation}
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
