import { useId, useState, type ReactNode } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { FileText, Lock, Pencil, SearchX, Trash2 } from "lucide-react";
import type { PolicySummary } from "@/api/generated/model";
import {
  getPolicyAdminListPoliciesQueryKey,
  usePolicyAdminDeletePolicyHandler,
  usePolicyAdminSetPolicyEnabled,
} from "@/api/generated/policy-admin/policy-admin";
import { EmptyState } from "@/components/explore/EmptyState";
import { FilterableTable, type Column } from "@/components/explore/FilterableTable";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { IconButton } from "@/components/ui/icon-button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { PolicyWarningsCallout } from "@/routes/governance/PolicyWarningsCallout";
import { refusalMessage, usePolicyPermissions } from "@/routes/governance/policyWrites";
import { TabError } from "@/routes/governance/tabParts";
import { compiledSource, compiledSourceLabel, type CompiledSource } from "@/routes/governance/engines/compiledSource";
import { editorUrl, engineUrl, type EditorSubject } from "@/routes/governance/engines/engineUrl";
import { tagUrl } from "@/routes/governance/classifications/tagUrl";
import type { TabContext } from "@/routes/governance/engines/PageFrame";
import type { PolicySource } from "@/routes/governance/engines/profilePolicies";

export const NO_POLICY = "This profile has no policy.";
export const NO_MATCH = "No policy matches these filters.";
export const COMPILED_SENTENCE = "Compiled by egret; change its source";
export const SEEDED_SENTENCE = "Managed by egret (engine delegation, admin access or query limits).";
export const HAND_WRITTEN_SENTENCE = "Not managed by egret";

const SOURCE_LABEL: Record<PolicySource, string> = {
  compiler: "egret-compiled",
  infra: "egret-seeded",
  operator: "manual",
};
const SOURCES: PolicySource[] = ["compiler", "infra", "operator"];

/** The type filter: a policy type of the profile, or `tag` for its tag service's rows. */
type TypeFilter = "access" | "masking" | "rowfilter" | "tag";
const TYPE_LABEL: Record<TypeFilter, string> = {
  access: "Access",
  masking: "Masking",
  rowfilter: "Row filter",
  tag: "Tag",
};
const ALL = "all";

/** Where "Open source" goes, or null when the source has no page this reader can open. */
function sourceHref(src: CompiledSource | null, subject: EditorSubject, tagRead: boolean): string | null {
  if (!src) return null;
  switch (src.kind) {
    case "grant":
      return `/governance/grants/${encodeURIComponent(src.id)}`;
    case "rule":
      return `/governance/rules/${encodeURIComponent(src.id)}`;
    case "clearance":
      return tagRead ? tagUrl(src.id, "clearance") : null;
    case "engine_grant":
      return "engineId" in subject ? engineUrl(subject.engineId, "engine-permissions", { grant: src.id }) : null;
    default:
      return null;
  }
}

const sourceOf = (r: PolicySummary): PolicySource =>
  r.managed_by === "compiler" || r.managed_by === "infra" ? r.managed_by : "operator";

/**
 * The Policies tab (spec §6.2, EP5): every policy of the profile and of its tag
 * service, filtered by source, type and a search, all three in the URL
 * (FE-PAGE-2). Ownership is `managed_by`; a compiled row is read-only and opens
 * the egret object it was compiled from; a seeded row is read-only; a
 * hand-written row is enabled, disabled, deleted and edited here, each control
 * shown only with the permission its route needs.
 */
export function PoliciesTab({ ctx }: { ctx: TabContext }) {
  const can = usePolicyPermissions();
  const queryClient = useQueryClient();
  const [search, setSearch] = useSearchParams();
  const [deleting, setDeleting] = useState<PolicySummary | null>(null);
  const [togglingId, setTogglingId] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const ids = { source: useId(), type: useId(), q: useId() };

  const profile = ctx.profile;
  const tagService = profile?.tagService ?? null;
  const policies = ctx.policies;

  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey: getPolicyAdminListPoliciesQueryKey() });
    // The warnings callout and the old pages' lists sit under this prefix.
    void queryClient.invalidateQueries({ queryKey: ["control-plane", "policies"] });
  };
  const toggle = usePolicyAdminSetPolicyEnabled({
    mutation: {
      onMutate: ({ id }) => setTogglingId(id),
      onSettled: () => setTogglingId(null),
      onSuccess: (_d, { data }) => {
        setError(null);
        invalidate();
        toast.success(data.enabled ? "Policy enabled." : "Policy disabled.");
      },
      onError: (e) => {
        setError(refusalMessage(e, "policy:update", "The policy could not be changed."));
        invalidate();
      },
    },
  });
  const del = usePolicyAdminDeletePolicyHandler({
    mutation: {
      onSuccess: () => {
        setError(null);
        invalidate();
        toast.success("Policy deleted.");
      },
      onError: (e) => setError(refusalMessage(e, "policy:delete", "The policy could not be deleted.")),
    },
  });

  if (!profile) {
    return <EmptyState icon={FileText} title="This engine downloads no policy profile, so it has no policy." />;
  }

  const sourceParam = search.get("source");
  const source = SOURCES.find((s) => s === sourceParam) ?? null;
  const typeParam = search.get("type");
  const types: TypeFilter[] = tagService ? ["access", "masking", "rowfilter", "tag"] : ["access", "masking", "rowfilter"];
  const type = types.find((t) => t === typeParam) ?? null;
  const q = search.get("q") ?? "";

  const setParam = (key: string, value: string | null, replace = false) =>
    setSearch(
      (prev) => {
        const next = new URLSearchParams(prev);
        if (value) next.set(key, value);
        else next.delete(key);
        return next;
      },
      { replace },
    );
  const clearFilters = () =>
    setSearch((prev) => {
      const next = new URLSearchParams(prev);
      for (const k of ["source", "type", "q"]) next.delete(k);
      return next;
    });

  const fromTag = (r: PolicySummary) => !!tagService && r.service === tagService;
  const needle = q.trim().toLowerCase();
  const rows = policies.rows.filter((r) => {
    if (source && sourceOf(r) !== source) return false;
    if (type === "tag" && !fromTag(r)) return false;
    if (type && type !== "tag" && (fromTag(r) || r.policy_type !== type)) return false;
    if (needle) {
      const hay = [r.name, r.resource_path, ...r.principals].join(" ").toLowerCase();
      if (!hay.includes(needle)) return false;
    }
    return true;
  });

  const counts = { compiler: 0, infra: 0, operator: 0 };
  for (const r of policies.rows) counts[sourceOf(r)] += 1;
  const total = policies.rows.length;

  // Every edit goes through the page's profile with the row's own type, so a
  // tag-service policy opens in the By-tag editor that wrote it.
  const editHref = (r: PolicySummary) => editorUrl(ctx.subject, r.policy_type, r.id);

  const columns: Column<PolicySummary>[] = [
    {
      key: "name",
      label: "Name",
      sortable: true,
      value: (r) => r.name,
      cell: (r) =>
        sourceOf(r) === "operator" ? (
          <Link to={editHref(r)} className="break-all font-medium text-primary hover:underline focus-visible:ring-2 focus-visible:ring-ring">
            {r.name}
          </Link>
        ) : (
          <span className="inline-flex items-center gap-1 break-all font-medium">
            {sourceOf(r) === "compiler" ? <Lock className="h-3 w-3 shrink-0" aria-hidden /> : null}
            {r.name}
          </span>
        ),
    },
    {
      key: "source",
      label: "Source",
      sortable: true,
      value: (r) => SOURCE_LABEL[sourceOf(r)],
      cell: (r) => <SourceCell row={r} subject={ctx.subject} tagRead={can("tag:read")} />,
    },
    {
      key: "type",
      label: "Type",
      sortable: true,
      value: (r) => r.policy_type,
      cell: (r) => (
        <Badge variant="outline" className="font-normal">
          {TYPE_LABEL[r.policy_type as TypeFilter] ?? r.policy_type}
          {fromTag(r) ? " (by tag)" : ""}
        </Badge>
      ),
    },
    {
      key: "resource",
      label: "Resource",
      value: (r) => r.resource_path,
      cell: (r) => <span className="break-all font-mono text-xs">{r.resource_path}</span>,
    },
    {
      key: "principals",
      label: "Principals",
      value: (r) => r.principals.join(", "),
      cell: (r) => <span className="break-words">{r.principals.join(", ") || "None"}</span>,
    },
    {
      key: "enabled",
      label: "Enabled",
      sortable: true,
      value: (r) => (r.is_enabled ? 1 : 0),
      cell: (r) =>
        sourceOf(r) === "operator" && can("policy:update") ? (
          <Switch
            aria-label={`Enable ${r.name}`}
            checked={r.is_enabled}
            disabled={toggle.isPending && togglingId === r.id}
            onCheckedChange={(enabled) => toggle.mutate({ id: r.id, data: { enabled } })}
          />
        ) : (
          <span>{r.is_enabled ? "Enabled" : "Disabled"}</span>
        ),
    },
    {
      key: "actions",
      label: "Actions",
      className: "text-right",
      cell: (r) =>
        sourceOf(r) === "operator" ? (
          <span className="inline-flex items-center gap-1">
            {can("policy:update") ? (
              <Button variant="ghost" size="sm" asChild>
                <Link to={editHref(r)} aria-label={`Edit ${r.name}`}>
                  <Pencil className="h-4 w-4" aria-hidden />
                  Edit
                </Link>
              </Button>
            ) : null}
            {can("policy:delete") ? (
              <IconButton aria-label={`Delete ${r.name}`} onClick={() => setDeleting(r)}>
                <Trash2 className="h-4 w-4" aria-hidden />
              </IconButton>
            ) : null}
          </span>
        ) : null,
    },
  ];

  let body: ReactNode;
  if (policies.isError) body = <TabError what="policies" onRetry={policies.refetch} />;
  else if (policies.isLoading) body = <ListSkeleton rows={5} cols={6} />;
  else if (total === 0) body = <EmptyState icon={FileText} title={NO_POLICY} />;
  else if (rows.length === 0)
    body = (
      <EmptyState
        icon={SearchX}
        title={NO_MATCH}
        action={
          <Button variant="outline" size="sm" onClick={clearFilters}>
            Clear filters
          </Button>
        }
      />
    );
  else
    body = (
      <FilterableTable
        rows={rows}
        columns={columns}
        getId={(r) => `${r.service}:${r.id}`}
        initialSort={{ key: "name", dir: "asc" }}
        pageSize={25}
        bordered
        hideSearch
        emptyText={NO_MATCH}
      />
    );

  return (
    <div className="space-y-4">
      <PolicyWarningsCallout
        service={profile.name}
        serviceId={profile.dto?.id}
        serviceType={profile.type}
        editorHref={(kind, id) => editorUrl(ctx.subject, kind, id)}
      />
      {total > 0 && !policies.isError ? (
        <p className="text-sm text-muted-foreground tabular-nums">
          {`${total} ${total === 1 ? "policy" : "policies"}: ${counts.compiler} egret-compiled · ${counts.infra} egret-seeded · ${counts.operator} manual`}
        </p>
      ) : null}
      <div className="flex flex-wrap items-end gap-3">
        <div className="space-y-1">
          <Label htmlFor={ids.q}>Search policies</Label>
          <Input
            id={ids.q}
            type="search"
            className="w-64 max-w-full"
            value={q}
            placeholder="Name, resource or principal"
            onChange={(e) => setParam("q", e.target.value || null, true)}
          />
        </div>
        <div className="space-y-1">
          <Label htmlFor={ids.source}>Source</Label>
          <Select value={source ?? ALL} onValueChange={(v) => setParam("source", v === ALL ? null : v)}>
            <SelectTrigger id={ids.source} className="w-44">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={ALL}>Every source</SelectItem>
              {SOURCES.map((s) => (
                <SelectItem key={s} value={s}>
                  {SOURCE_LABEL[s]}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div className="space-y-1">
          <Label htmlFor={ids.type}>Type</Label>
          <Select value={type ?? ALL} onValueChange={(v) => setParam("type", v === ALL ? null : v)}>
            <SelectTrigger id={ids.type} className="w-40">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={ALL}>Every type</SelectItem>
              {types.map((t) => (
                <SelectItem key={t} value={t}>
                  {TYPE_LABEL[t]}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      </div>
      {error ? (
        <Alert variant="destructive">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      ) : null}
      {body}
      <ConfirmDeleteDialog
        open={deleting !== null}
        onOpenChange={(open) => {
          if (!open) setDeleting(null);
        }}
        title={deleting ? `Delete "${deleting.name}"?` : "Delete policy?"}
        description="The manual policy is removed from the profile; the engine drops it at its next plugin download."
        onConfirm={() => {
          if (deleting) del.mutate({ id: deleting.id });
          setDeleting(null);
        }}
      />
    </div>
  );
}

function SourceCell({ row, subject, tagRead }: { row: PolicySummary; subject: EditorSubject; tagRead: boolean }) {
  const source = sourceOf(row);
  if (source === "infra") {
    return (
      <div className="space-y-0.5">
        <span className="font-medium">{SOURCE_LABEL.infra}</span>
        <p className="text-xs text-muted-foreground">{SEEDED_SENTENCE}</p>
      </div>
    );
  }
  if (source === "operator") {
    return (
      <div className="space-y-0.5">
        <span className="font-medium">{SOURCE_LABEL.operator}</span>
        <p className="text-xs text-muted-foreground">{HAND_WRITTEN_SENTENCE}</p>
      </div>
    );
  }
  const src = compiledSource(row.name);
  const href = sourceHref(src, subject, tagRead);
  const word = compiledSourceLabel(src);
  return (
    <div className="space-y-0.5">
      <span className="font-medium">{SOURCE_LABEL.compiler}</span>
      <p className="text-xs text-muted-foreground">{COMPILED_SENTENCE}</p>
      <p className="text-xs">
        <span>{word}</span>
        {href ? (
          <>
            {" · "}
            <Link to={href} className="text-primary underline focus-visible:ring-2 focus-visible:ring-ring">
              Open source
              <span className="sr-only">{` of ${row.name}`}</span>
            </Link>
          </>
        ) : null}
      </p>
    </div>
  );
}
