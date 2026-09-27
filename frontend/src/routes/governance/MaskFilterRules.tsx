import { useState } from "react";
import { EyeOff, Pencil, Plus, Trash2 } from "lucide-react";
import { toast } from "sonner";
import { useQueryClient } from "@tanstack/react-query";

import {
  getPermListRulesQueryKey,
  usePermDeleteRule,
  usePermListRules,
} from "@/api/generated/perm/perm";
import { useControlPlaneCatalogRegistryListRegistryCatalogs } from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";
import type { Rule } from "@/api/generated/model";
import { ApiError } from "@/api/client";
import { useTags } from "@/api/tags";
import { useAuth } from "@/auth/useAuth";
import { EmptyState } from "@/components/explore/EmptyState";
import { FACET_ALL, FacetSelect } from "@/components/explore/FacetSelect";
import { FilterableTable, type Column } from "@/components/explore/FilterableTable";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { formatPrincipals, formatRuleKind, formatSelector } from "./permFormat";
import { RuleDialog } from "./RuleDialog";

/**
 * Governance → Mask & filter rules (SP4b-2 Task 4).
 *
 * Steward-facing: named `perm.rules` that mask a column or filter rows
 * wherever a classification tag or a path matches. A tag-selected rule is
 * catalog-unscoped by design (SP4a), which is exactly why every write here
 * goes through the same preview-before-commit gate `RuleDialog` shares with
 * `GrantDialog`.
 */
export default function MaskFilterRules() {
  const { me } = useAuth();
  const isAdmin = !!me && me.platform_roles.includes("platform_admin");
  const canWrite = isAdmin || !!me?.permissions.includes("policy:update");

  const rulesQ = usePermListRules({ query: { retry: false } });
  const rows: Rule[] = rulesQ.data?.status === 200 ? rulesQ.data.data : [];

  const catalogsQ = useControlPlaneCatalogRegistryListRegistryCatalogs(undefined, {
    query: { retry: false },
  });
  const catalogs = catalogsQ.data?.status === 200 ? catalogsQ.data.data : [];
  const catalogName = (id: string) => catalogs.find((c) => c.id === id)?.name ?? id;

  const tags = useTags();
  const tagName = (key: string) => tags.data?.find((t) => t.key === key)?.name ?? key;

  const [kindFilter, setKindFilter] = useState(FACET_ALL);
  const filtered = rows.filter((r) => kindFilter === FACET_ALL || r.kind.type === kindFilter);

  const [dialogOpen, setDialogOpen] = useState(false);
  const [editing, setEditing] = useState<Rule | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<Rule | null>(null);

  const queryClient = useQueryClient();
  const deleteRule = usePermDeleteRule<ApiError>({
    mutation: {
      onSuccess: () => void queryClient.invalidateQueries({ queryKey: getPermListRulesQueryKey() }),
      onError: (e) => toast.error(e.message || "Failed to delete the rule."),
    },
  });

  function openCreate() {
    setEditing(null);
    setDialogOpen(true);
  }
  function openEdit(r: Rule) {
    setEditing(r);
    setDialogOpen(true);
  }

  const columns: Column<Rule>[] = [
    {
      key: "name",
      label: "Name",
      sortable: true,
      className: "font-medium",
      value: (r) => r.name,
      cell: (r) => r.name,
    },
    {
      key: "kind",
      label: "Kind",
      sortable: true,
      value: (r) => formatRuleKind(r),
      cell: (r) => (
        <Badge variant={r.kind.type === "mask" ? "secondary" : "outline"}>
          {formatRuleKind(r)}
        </Badge>
      ),
    },
    {
      key: "selects",
      label: "Selects",
      value: (r) => formatSelector(r.selector, catalogName, tagName),
      cell: (r) => formatSelector(r.selector, catalogName, tagName),
    },
    {
      key: "exempt",
      label: "Exemptions",
      value: (r) => formatPrincipals(r.exempt.include),
      cell: (r) =>
        r.exempt.include.length > 0 ? (
          formatPrincipals(r.exempt.include)
        ) : (
          <span className="text-muted-foreground">—</span>
        ),
    },
    {
      key: "actions",
      label: "",
      className: "w-24 text-right",
      cell: (r) =>
        canWrite ? (
          <div className="flex justify-end gap-1">
            <Button
              variant="ghost"
              size="icon"
              aria-label={`Edit ${r.name}`}
              onClick={() => openEdit(r)}
            >
              <Pencil className="h-4 w-4" />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              className="text-muted-foreground hover:text-destructive"
              aria-label={`Delete ${r.name}`}
              onClick={() => setDeleteTarget(r)}
            >
              <Trash2 className="h-4 w-4" />
            </Button>
          </div>
        ) : null,
    },
  ];

  const maskCount = rows.filter((r) => r.kind.type === "mask").length;
  const rowFilterCount = rows.filter((r) => r.kind.type === "row_filter").length;
  const tagSelectedCount = rows.filter((r) => r.selector.type === "tag").length;

  const forbidden = rulesQ.error instanceof ApiError && rulesQ.error.status === 403;

  const newRuleButton = canWrite ? (
    <Button size="sm" onClick={openCreate}>
      <Plus className="mr-1 h-4 w-4" />
      New rule
    </Button>
  ) : null;

  return (
    <div className="space-y-5">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h1 className="text-2xl font-semibold leading-tight">Mask &amp; filter rules</h1>
          <p className="text-sm text-muted-foreground">
            Named rules that mask columns or filter rows wherever a classification or a path
            matches.
          </p>
        </div>
        {newRuleButton}
      </div>

      <div className="space-y-4">
        {rulesQ.isLoading ? (
          <ListSkeleton rows={6} cols={4} />
        ) : forbidden ? (
          <Alert variant="destructive">
            <AlertDescription>
              You need the <code>policy:read</code> permission.
            </AlertDescription>
          </Alert>
        ) : rulesQ.isError ? (
          <Alert variant="destructive">
            <AlertDescription>Failed to load rules. Please try again.</AlertDescription>
          </Alert>
        ) : (
          <>
            <SummaryStrip
              items={[
                { label: "rules", singular: "rule", value: rows.length },
                { label: "masks", value: maskCount },
                { label: "row filters", value: rowFilterCount },
                { label: "tag-selected", value: tagSelectedCount },
              ]}
            />
            <FilterableTable
              rows={filtered}
              columns={columns}
              getId={(r) => r.id}
              searchPlaceholder="Search rules…"
              initialSort={{ key: "name", dir: "asc" }}
              pageSize={25}
              bordered
              filters={
                <FacetSelect
                  label="Kind"
                  value={kindFilter}
                  onChange={setKindFilter}
                  options={[
                    { value: "mask", label: "Mask" },
                    { value: "row_filter", label: "Row filter" },
                  ]}
                />
              }
              emptyText={
                <EmptyState
                  icon={EyeOff}
                  variant="dashed"
                  title="No rules yet"
                  description={
                    canWrite
                      ? "Create a rule to mask a column or filter rows by classification or path."
                      : "Creating a rule needs the policy:update permission, which this account does not have."
                  }
                  action={newRuleButton ?? undefined}
                />
              }
            />
          </>
        )}
      </div>

      <RuleDialog
        open={dialogOpen}
        onOpenChange={setDialogOpen}
        existing={editing}
        catalogName={catalogName}
      />

      <ConfirmDeleteDialog
        open={deleteTarget !== null}
        onOpenChange={(o) => {
          if (!o) setDeleteTarget(null);
        }}
        title={deleteTarget ? `Delete "${deleteTarget.name}"?` : "Delete rule?"}
        description="This rule is permanently removed and every profile it reaches recompiles."
        confirmText="delete"
        onConfirm={() => {
          if (deleteTarget) deleteRule.mutate({ id: deleteTarget.id });
          setDeleteTarget(null);
        }}
      />
    </div>
  );
}
