import { useEffect, useState } from "react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Loader2, Plus, Trash2, TriangleAlert, X } from "lucide-react";
import { getPolicyAdminListPoliciesQueryKey } from "@/api/generated/policy-admin/policy-admin";

import {
  type AccessItem,
  type CreatedPolicyId,
  type MaskItem,
  type ResourceLevel,
  type RowFilterItem,
  type ServiceDef,
  type ServiceDefResource,
  compilerOwnedConflict,
  createPolicy,
  deletePolicy,
  getPolicy,
  getServiceDef,
  updatePolicy,
} from "@/api/policies";
import { ApiError } from "@/api/client";
import { useTags } from "@/api/tags";
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { refusalMessage, usePolicyPermissions } from "@/routes/governance/policyWrites";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { MaskItemTable } from "@/components/policy/MaskItemTable";
import { PermissionTable } from "@/components/policy/PermissionTable";
import { ResourceSelector } from "@/components/policy/ResourceSelector";
import { grantsSelect, levelGaps, levelHasValue, levelsToFill } from "@/components/policy/resourceLevels";
import { RowFilterItemTable } from "@/components/policy/RowFilterItemTable";
import { ConfigSection } from "@/components/form/ConfigSection";
import { FieldLabel } from "@/routes/control-plane/config/FieldLabel";
import {
  editorUrl,
  engineUrl,
  profileUrl as profileUrlOf,
  type EditorSubject,
} from "@/routes/governance/engines/engineUrl";
import { BackLink } from "@/components/layout/DetailHeader";

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
// Table-level chain (catalog/schema/table) — the common stopping point for
// access and row-filter policies. Only catalog starts empty (the one
// Required field); the user can extend to column (access) or drop back to
// schema/catalog via ResourceSelector's "— none —" affordance.
const emptyTableResources = () => ({
  catalog: emptyLevel(),
  schema: wildcardLevel(),
  table: wildcardLevel(),
});
// Column-level chain — masking policies operate on a column, so include it.
const emptyColumnResources = () => ({
  ...emptyTableResources(),
  column: wildcardLevel(),
});

// Build a default resource chain (root empty, deeper levels wildcard) from the
// service-def hierarchy so a fresh StarRocks policy uses catalog/database/…
// and a Trino one uses catalog/schema/…. Access and masking chains extend to
// `column`; row-filter stops at `table` (matching the previous Trino
// defaults: `emptyColumnResources` vs `emptyTableResources`).
// Exported for unit testing (pure helper).
export function defaultResourcesFromDef(
  resDefs: ServiceDefResource[],
  kind: PolicyKind,
): Record<string, ResourceLevel> {
  const byName = new Map(resDefs.map((r) => [r.name, r]));
  const childrenOf = (n: string) => resDefs.filter((r) => r.parent === n);
  const stopAt = kind === "rowfilter" ? "table" : "column";
  let node = resDefs.find((r) => r.name === "catalog" && !r.parent)
    ?? resDefs.find((r) => !r.parent && childrenOf(r.name).length > 0);
  const out: Record<string, ResourceLevel> = {};
  let first = true;
  while (node) {
    out[node.name] = first ? emptyLevel() : wildcardLevel();
    first = false;
    if (node.name === stopAt) break;
    const kids = childrenOf(node.name);
    // Follow the linear data hierarchy: prefer database/schema, then table, then column.
    const next =
      kids.find((k) => k.name === "database" || k.name === "schema") ??
      kids.find((k) => k.name === "table") ??
      kids.find((k) => k.name === "column");
    if (!next) break;
    node = byName.get(next.name);
  }
  return out;
}

// The masking/row-filter resource subsets (`dataMaskDef.resources`/
// `rowFilterDef.resources`) are flat name-only refs with no parent/level, so
// ResourceSelector — which cascades by walking `parent` links — would render
// only the root. Map each subset entry onto its full `resources` counterpart
// (by name) so the subset regains parent/level/flags and cascades correctly,
// while still limiting the selectable types to the kind's allowed subset. A
// name with no full match is kept as-is (no invented hierarchy). Exported for
// unit testing (pure helper).
export function enrichResourceDefs(
  subset: ServiceDefResource[],
  full: ServiceDefResource[],
): ServiceDefResource[] {
  const byName = new Map(full.map((r) => [r.name, r]));
  return subset.map((r) => byName.get(r.name) ?? r);
}

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
      resources: emptyColumnResources(),
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
      resources: emptyTableResources(),
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
    // Show the full catalog→schema→table→column chain by default; the user can
    // drop column (or table/schema) via the "— none —" option if not needed.
    resources: emptyColumnResources(),
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
 * Engine & policy → Advanced → full-page policy editor (spec §6.6, EP2): one
 * page for the three policy kinds, create and edit. It edits a policy of
 * `service`, the profile of the page it hangs under (`subject`: an engine's
 * page or an unbound profile's), and goes back to that page's Advanced tab.
 *
 * Route params: `kind` ("access"|"masking"|"rowfilter", Q2) and an optional
 * `policyId` (present = editing an existing policy).
 */
export default function PolicyEditorPage({ service, subject }: { service: string; subject: EditorSubject }) {
  const params = useParams<{ kind: string; policyId?: string }>();
  const kind = (params.kind as PolicyKind) ?? "access";
  const id = params.policyId;
  const [sp] = useSearchParams();
  const navigate = useNavigate();
  const queryClient = useQueryClient();

  const [form, setForm] = useState<any>(() => blankInput(kind, service));
  const [labelInput, setLabelInput] = useState("");
  const [deleteOpen, setDeleteOpen] = useState(false);

  // Outer left-nav section (Resource / Permissions|Masking|Row filter /
  // Details) — mirrors EngineDetail's `ConfigurationTab` vertical nav.
  const [section, setSection] = useState("resource");

  // Policy scope: "resource" (the existing catalog/schema/table/column
  // picker) vs "tag" (a single classification tag — the BE resolves the tag
  // service from the chosen trino profile and dispatches on `kind:"tag-*"`).
  // New policies default to "resource"; `?scope=tag` (from the profile's Tags
  // tab "New" action) opens straight in tag mode.
  const [scope, setScope] = useState<"resource" | "tag">(
    sp.get("scope") === "tag" ? "tag" : "resource",
  );
  const [tag, setTag] = useState("");
  const tagsQuery = useTags();
  const tagOptions = tagsQuery.data ?? [];

  const policyQuery = useQuery<Record<string, unknown>, ApiError>({
    queryKey: ["control-plane", "policy", id],
    queryFn: () => getPolicy(Number(id)),
    enabled: !!id,
    retry: false,
  });

  useEffect(() => {
    if (id && policyQuery.data) {
      const state = toFormState(kind, service, policyQuery.data);
      setForm(state);
      // A loaded policy whose resource is a single `tag` is a tag policy — open
      // it in By-tag mode so the Tags-tab edit round-trips instead of showing an
      // empty resource picker.
      const tagVal = (state.resources as Record<string, ResourceLevel>)?.tag?.values?.[0];
      if (tagVal) {
        setScope("tag");
        setTag(tagVal);
      }
    }
  }, [id, policyQuery.data, kind, service]);

  const engineOf = (d: ServiceDef | undefined) => (typeof d?.name === "string" ? d.name : undefined);

  const def = useQuery<ServiceDef>({
    queryKey: ["service-def", service],
    queryFn: () => getServiceDef(service),
    retry: false,
  });

  // Applicable resource-type defs for the current policy kind: access uses
  // the top-level hierarchy, masking/rowfilter use their own (Ranger keeps
  // separate resource defs per policy kind — e.g. masking's may omit levels
  // access allows, or vice versa).
  const fullResDefs = def.data?.resources ?? [];
  const resDefs: ServiceDefResource[] =
    kind === "masking"
      ? enrichResourceDefs(def.data?.dataMaskDef?.resources ?? [], fullResDefs)
      : kind === "rowfilter"
        ? enrichResourceDefs(def.data?.rowFilterDef?.resources ?? [], fullResDefs)
        : fullResDefs;
  const resDefByName = (name: string): ServiceDefResource | undefined =>
    resDefs.find((r) => r.name === name);

  // On create, once the def is known, seed the resource chain from the def so
  // the blank form matches the profile's engine (StarRocks: catalog/database/…;
  // Trino: catalog/schema/…) instead of the hardcoded Trino skeleton. Seed from
  // the top-level `resources` (which carry parent/level) rather than `resDefs`:
  // the masking/row-filter subsets (`dataMaskDef.resources`/`rowFilterDef.
  // resources`) are flat name-only refs with no parent links, so walking them
  // would stop at the root — the full hierarchy, capped per kind, is the chain.
  useEffect(() => {
    if (!id && def.data) {
      const seeded = defaultResourcesFromDef(def.data.resources ?? [], kind);
      if (Object.keys(seeded).length) {
        setForm((f: any) => ({ ...f, resources: seeded }));
      }
    }
  }, [id, def.data, kind]);

  // The selected chain's leaf resource type (reported by ResourceSelector),
  // used to restrict which access types are offered — e.g. a `trinouser`
  // leaf only allows "impersonate", a `role` leaf only its own restrictions.
  const [leaf, setLeaf] = useState<string | null>(null);
  const leafDef = leaf ? resDefByName(leaf) : undefined;
  const accessTypes = leafDef?.accessTypeRestrictions?.length
    ? leafDef.accessTypeRestrictions
    : (def.data?.accessTypes?.map((a) => a.name) ?? DEFAULT_ACCESS_TYPES);

  const profileUrl = "engineId" in subject ? engineUrl(subject.engineId, "advanced") : profileUrlOf(subject.profile, "advanced");

  // In Tag mode the wire `kind` becomes "tag-<kind>" and a `tag` field
  // carries the selected tag name; `service` stays the trino profile (the BE
  // resolves the tag service from it). Resource-only fields (`resources`/
  // `additional`) are dropped — the BE overwrites/ignores them anyway, since
  // a tag policy targets exactly one tag, not a resource hierarchy. In
  // Resource mode the payload is exactly `form`, unchanged.
  const buildPayload = (): any => {
    if (scope !== "tag") return form;
    const tagKind = kind === "masking" ? "tag-masking" : kind === "rowfilter" ? "tag-rowfilter" : "tag-access";
    const { resources: _resources, additional: _additional, ...rest } = form;
    return { ...rest, kind: tagKind, tag };
  };

  // SP4b-2 review I-1, the third page of the same surface: this editor is
  // reachable by URL for anyone who can READ a policy, so `Save` and `Delete`
  // were offered to an account whose write would 403. Disabled-with-a-reason
  // rather than hidden, because the form itself is a legitimate read for a
  // `policy:read` holder — hiding the button would leave the page looking
  // broken instead of explaining it. Creating needs `policy:create`, editing
  // an existing policy `policy:update` (two different routes).
  const can = usePolicyPermissions();
  const savePermission = id ? "policy:update" : "policy:create";
  const canSave = can(savePermission);
  const canDelete = can("policy:delete");

  // What the server did with the levels left empty (SP4d-4 B1). A plain
  // `{ id }` save navigates away; a save that filled a level (or overtook
  // another policy) stays, because the response IS the point (FE-ACTION-2):
  // the operator must read what the save changed before leaving.
  const [saved, setSaved] = useState<CreatedPolicyId | null>(null);
  const [confirmWiden, setConfirmWiden] = useState(false);

  // The Policies and Advanced tabs read the generated list key; the legacy key
  // is kept for any other reader of `api/policies`.
  const invalidatePolicyLists = () => {
    void queryClient.invalidateQueries({ queryKey: ["control-plane", "policies"] });
    void queryClient.invalidateQueries({ queryKey: getPolicyAdminListPoliciesQueryKey() });
  };

  const save = useMutation<CreatedPolicyId, ApiError>({
    mutationFn: () => (id ? updatePolicy(Number(id), buildPayload()) : createPolicy(buildPayload())),
    onSuccess: (res) => {
      invalidatePolicyLists();
      if (res?.filledLevels?.length || res?.warnings?.length) {
        setSaved(res);
        return;
      }
      navigate(profileUrl);
    },
  });

  const del = useMutation<void, ApiError>({
    mutationFn: () => deletePolicy(Number(id)),
    onSuccess: () => {
      invalidatePolicyLists();
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
  // itself via ResourceLevelEditor's `invalid` prop.) In Tag mode, a tag
  // must be selected instead.
  // Only a ROOT level (catalog, role…) blocks: the server saves an empty
  // deeper level as `*`, and the selector says so under the field.
  const isRoot = (name: string) => !resDefByName(name)?.parent;
  const missingRequired: boolean =
    scope === "tag"
      ? !tag
      : Object.entries(form.resources as Record<string, ResourceLevel>).some(
          ([name, lvl]) => resDefByName(name)?.mandatory && isRoot(name) && !levelHasValue(lvl),
        );

  // Resource maps of the primary resource and every additional one.
  const resourceMaps: Record<string, ResourceLevel>[] =
    scope === "tag" ? [] : [form.resources, ...(kind === "access" ? form.additional : [])];
  // A named level under an absent one: the server refuses it.
  const gaps = resourceMaps.flatMap((m) => levelGaps(resDefs, m));
  // Levels the server will save as `*`.
  const fills = Array.from(new Set(resourceMaps.flatMap((m) => levelsToFill(resDefs, m))));
  // Access types in force, for the "stops above column" warning.
  const grantedAccessTypes: string[] | undefined =
    kind === "access"
      ? Array.from(
          new Set(
            form.items
              .filter((it: AccessItem) => it.effect === "allow")
              .flatMap((it: AccessItem) => it.accesses),
          ),
        )
      : undefined;
  // Saving an ALREADY-STORED policy that the server will widen: an unrelated
  // edit must not silently turn a narrower row into a broader one.
  const widensStored = !!id && fills.length > 0;
  const widensColumnSelect =
    fills.includes("column") && kind === "access" && grantsSelect(grantedAccessTypes);
  const saveBlockedReason = !canSave
    ? `Saving needs the ${savePermission} permission.`
    : !form.name.trim()
      ? "Enter a policy name"
      : missingRequired
        ? scope === "tag"
          ? "Select a tag"
          : "Name the required resource level (the first one) before saving"
        : gaps.length > 0
          ? `${gaps[0].level} is set but ${gaps[0].missing} is not: set ${gaps[0].missing} or clear ${gaps[0].level}`
          : undefined;

  const kindTitle =
    kind === "masking" ? "Masking" : kind === "rowfilter" ? "Row filter" : "Permissions";

  const TABLE_LEVELS = ["catalog", "schema", "table", "column"];

  // At-a-glance summary of the resource being configured, e.g. `iceberg.sales.*.*`
  // for a catalog/schema/table/column chain, or `role: *` / `trinouser: *`
  // for a single non-table resource. In Tag mode it's just `tag: <name>`,
  // matching the BE's `summarize_policy` output for tag policies.
  // Empty levels fall back to their own name (e.g. an unfilled catalog shows
  // "catalog") so a fresh policy reads "catalog.schema.table.column" rather than
  // a cryptic "….*.*", and self-documents which level to fill.
  const resourcePreview = (() => {
    if (scope === "tag") return tag ? `tag: ${tag}` : "tag: …";
    const keys = Object.keys(form.resources);
    if (keys.length === 0) return "catalog.schema.table.column";
    if (keys.every((k) => TABLE_LEVELS.includes(k))) {
      return TABLE_LEVELS.filter((l) => keys.includes(l))
        .map((l) => form.resources[l]?.values[0] || l)
        .join(".");
    }
    if (keys.length === 1) {
      const k = keys[0];
      return `${k}: ${form.resources[k].values[0] || "…"}`;
    }
    return keys.map((k) => form.resources[k].values[0] || k).join(".");
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
    scope === "tag"
      ? tag
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

  const backLink = (
    <div>
      <BackLink to={profileUrl}>{service || "Policies"}</BackLink>
    </div>
  );

  // DATA-SAFETY GUARD: when editing an existing policy, the form is seeded from
  // `policyQuery.data` only after it resolves. Never render the editor (and its
  // Save button) over the still-blank form — an early Save would overwrite the
  // real policy with empty values. Also block on the service-def, which the
  // resource selector + required-field validation depend on.
  const loadingDeps = (!!id && policyQuery.isLoading) || def.isLoading;
  const loadError = (!!id && policyQuery.isError) || (def.isError as boolean);
  if (loadingDeps) {
    return (
      <div className="space-y-5">
        {backLink}
        <ListSkeleton rows={6} cols={2} />
      </div>
    );
  }
  if (loadError) {
    const msg =
      (!!id && policyQuery.error?.message) ||
      "Couldn't load this policy's definition.";
    return (
      <div className="space-y-5">
        {backLink}
        <Alert variant="destructive">
          <AlertDescription>
            {msg} It wasn't loaded, so there's nothing safe to edit here — go back
            and try again.
          </AlertDescription>
        </Alert>
      </div>
    );
  }

  if (saved) {
    return (
      <div className="space-y-5">
        {backLink}
        <SavedResult
          result={saved}
          service={service}
          subject={subject}
          verb={id ? "updated" : "created"}
          profileUrl={profileUrl}
        />
      </div>
    );
  }

  return (
    <div className="space-y-5">
      {backLink}

      <div className="flex flex-wrap items-end justify-between gap-3">
        <div className="flex min-w-0 flex-wrap items-end gap-3">
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
            {!form.name.trim() && (
              <p className="text-xs text-muted-foreground">Required to save.</p>
            )}
          </div>
          <Badge variant="secondary" className="mb-2.5">
            {kind}
          </Badge>
          <div className="mb-2 flex items-center gap-2">
            <Switch
              aria-label="Enabled"
              checked={form.isEnabled}
              onCheckedChange={(c) => setForm((f: any) => ({ ...f, isEnabled: c }))}
            />
            <span className="text-sm text-muted-foreground">Enabled</span>
          </div>
        </div>
        <div className="mb-0.5 flex shrink-0 items-center gap-2">
          {id && canDelete && (
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
            onClick={() => (widensStored ? setConfirmWiden(true) : save.mutate())}
            title={saveBlockedReason}
            disabled={
              !canSave ||
              save.isPending ||
              !form.name.trim() ||
              missingRequired ||
              gaps.length > 0 ||
              (!!id && !policyQuery.data) ||
              !def.data
            }
          >
            {save.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
            Save
          </Button>
        </div>
      </div>

      {!canSave && (
        <Alert>
          <AlertDescription>
            This policy is read-only for this account: saving needs the{" "}
            <code>{savePermission}</code> permission. Everything below still shows what the
            policy contains.
          </AlertDescription>
        </Alert>
      )}

      {save.isError && (
        <Alert variant="destructive">
          <AlertDescription>
            {(() => {
              // D10: a write the compiler owns comes back 409 with an
              // explanation ("edit the grant/rule instead") — surface that,
              // not a bare error. Any other failure keeps the old fallback.
              const conflict = compilerOwnedConflict(save.error);
              return conflict ? (
                <>
                  <span className="font-medium">This policy is owned by the permission model.</span>{" "}
                  {conflict}
                </>
              ) : (
                // A 403's body is the bare word `forbidden`; name the
                // permission instead (review I-1).
                refusalMessage(save.error, savePermission, "Failed to save policy.")
              );
            })()}
          </AlertDescription>
        </Alert>
      )}

      <div className="rounded-lg border p-4 md:p-6">
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
                  title={scope === "tag" ? "Tag" : "Resource"}
                  description={
                    scope === "tag"
                      ? "The classification tag this policy applies to. Ranger resolves the tag service from this profile automatically."
                      : "The resource this policy applies to. Use * for wildcards."
                  }
                >
                  <div className="flex items-center gap-2" role="group" aria-label="Policy scope">
                    <Button
                      type="button"
                      size="sm"
                      variant={scope === "resource" ? "secondary" : "outline"}
                      aria-pressed={scope === "resource"}
                      onClick={() => setScope("resource")}
                    >
                      By resource
                    </Button>
                    <Button
                      type="button"
                      size="sm"
                      variant={scope === "tag" ? "secondary" : "outline"}
                      aria-pressed={scope === "tag"}
                      onClick={() => setScope("tag")}
                    >
                      By tag
                    </Button>
                  </div>

                  <p className="font-mono text-xs text-muted-foreground">{resourcePreview}</p>

                  {scope === "tag" ? (
                    <div className="space-y-1.5">
                      <FieldLabel label="Tag" htmlFor="policy-tag" />
                      <Select value={tag} onValueChange={setTag}>
                        <SelectTrigger
                          id="policy-tag"
                          aria-label="Tag"
                          size="sm"
                          className="h-9 w-64"
                        >
                          <SelectValue placeholder="Select a tag…" />
                        </SelectTrigger>
                        <SelectContent>
                          {tagOptions.map((t) => (
                            <SelectItem key={t.id} value={t.name}>
                              {t.name}
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                      {missingRequired && (
                        <p className="text-xs text-muted-foreground">Select a tag to continue.</p>
                      )}
                    </div>
                  ) : (
                    <>
                      <ResourceSelector
                        resourceDefs={resDefs}
                        service={service}
                        engine={engineOf(def.data)}
                        value={form.resources}
                        onChange={(r) => setForm((f: any) => ({ ...f, resources: r }))}
                        onLeafChange={setLeaf}
                        grantedAccessTypes={grantedAccessTypes}
                      />
                      {missingRequired && (
                        <p className="text-xs text-muted-foreground">
                          Name the required resource level (the first one) before saving.
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
                                  engine={engineOf(def.data)}
                                  value={res}
                                  onChange={(r) =>
                                    setForm((f: any) => ({
                                      ...f,
                                      additional: f.additional.map((a: any, j: number) =>
                                        j === i ? r : a,
                                      ),
                                    }))
                                  }
                                  grantedAccessTypes={grantedAccessTypes}
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
                        maskTypes={def.data?.dataMaskDef?.maskTypes}
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
      </div>

      <AlertDialog open={confirmWiden} onOpenChange={setConfirmWiden}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Saving will widen this stored policy</AlertDialogTitle>
            <AlertDialogDescription asChild>
              <div className="space-y-2 text-sm">
                <p>
                  This policy is already stored without{" "}
                  {fills.map((l, i) => (
                    <span key={l}>
                      {i > 0 && (i === fills.length - 1 ? " and " : ", ")}
                      <code>{l}</code>
                    </span>
                  ))}{" "}
                  set. Saving it, even with an unrelated change, stores{" "}
                  {fills.length === 1 ? "that level" : "those levels"} as <code>*</code> (every value),
                  so the policy will grant on more than it does today.
                </p>
                {widensColumnSelect && (
                  <p>Column SELECT becomes granted: the stored row grants no column reads today.</p>
                )}
                <p>
                  The broader policy may also be evaluated earlier than other policies in the same
                  priority band. The result of the save lists any policy it now overtakes.
                </p>
              </div>
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Keep the policy as stored</AlertDialogCancel>
            <AlertDialogAction onClick={() => save.mutate()}>Widen and save</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

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

const WARNING_KIND_EDITOR: Record<string, string> = {
  access: "access",
  mask: "masking",
  row_filter: "rowfilter",
};

/**
 * What a save did when it did more than store the form (SP4d-4 B1): levels
 * left empty were stored as `*`, and the policy may now be evaluated ahead of
 * another one in its priority band. Shown in place of the form (the response
 * is the point, so the page does not navigate away unread).
 */
function SavedResult({
  result,
  service,
  subject,
  verb,
  profileUrl,
}: {
  result: CreatedPolicyId;
  service: string;
  subject: EditorSubject;
  verb: "created" | "updated";
  profileUrl: string;
}) {
  const levels = result.filledLevels ?? [];
  const warnings = result.warnings ?? [];
  return (
    <div className="space-y-4">
      <Alert>
        <TriangleAlert className="h-4 w-4" />
        <div>
          <AlertTitle>
            Policy {verb}
            {levels.length > 0 && (
              <>
                {" "}
                with {levels.length === 1 ? "a level" : "levels"} stored as <code>*</code>
              </>
            )}
          </AlertTitle>
          <AlertDescription className="space-y-2">
            {levels.length > 0 && (
              <p>
                Saved as <code>*</code> (every value): {levels.join(", ")}.
              </p>
            )}
            {result.note && <p>{result.note}</p>}
          </AlertDescription>
        </div>
      </Alert>
      {warnings.length > 0 && (
        <Alert>
          <TriangleAlert className="h-4 w-4" />
          <div>
            <AlertTitle>
              {warnings.length === 1
                ? "This save changed the order of 1 other policy"
                : `This save changed the order of ${warnings.length} other policies`}
            </AlertTitle>
            <AlertDescription>
              <ul className="space-y-2">
                {warnings.map((w) => (
                  <li key={`${w.policyKind}-${w.policyId ?? w.policy}`}>
                    {w.policyId != null ? (
                      <Link
                        className="font-medium text-primary hover:underline"
                        to={editorUrl(subject, WARNING_KIND_EDITOR[w.policyKind] ?? "access", w.policyId)}
                      >
                        {w.policy}
                      </Link>
                    ) : (
                      <span className="font-medium">{w.policy}</span>
                    )}
                    : {w.sentence}
                  </li>
                ))}
              </ul>
            </AlertDescription>
          </div>
        </Alert>
      )}
      <Button asChild size="sm">
        <Link to={profileUrl}>Back to {service}</Link>
      </Button>
    </div>
  );
}
