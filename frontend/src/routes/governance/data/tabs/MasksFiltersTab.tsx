import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { EyeOff, Pencil, Plus, Trash2 } from "lucide-react";
import { toast } from "sonner";
import { getPermListRulesQueryKey, usePermDeleteRule, usePermListRules } from "@/api/generated/perm/perm";
import type { GovernanceCatalog, ResourceTagDto, RuleView } from "@/api/generated/model";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { RuleDialog } from "@/routes/governance/RuleDialog";
import { formatRuleKind, formatSelector } from "@/routes/governance/permFormat";
import { refusalMessage, usePolicyPermissions } from "@/routes/governance/policyWrites";
import { KIND_ICON, type NodeKind } from "@/routes/governance/data/CatalogTree";
import { patternLevels, ruleSelects, scopeCoverage, tagKeysOn, type PatternLevel } from "@/routes/governance/data/coverage";
import { scopeOf, tagReach, type CatalogState, type TagsResult } from "@/routes/governance/data/objectData";
import type { ObjectPath } from "@/routes/governance/data/objectPath";
import { CatalogGate, Section, TabError } from "@/routes/governance/data/tabs/TabStates";

/**
 * What a viewer without `policy:read` reads instead of the rules. Also this
 * module's bundle marker (`scripts/assert-governance-data-lazy.mjs`).
 */
export const RULES_NEED_POLICY_READ = "Reading rules needs the policy:read permission, which this account does not have.";

const LEVEL_LABEL = { catalog: "Catalog", schema: "Schema", table: "Table" } as const;

type Group = "here" | "inherited" | "columns" | "tag" | "pattern";

/** One listed rule: its group, and what the group's extra column says of it. */
type Row = { rule: RuleView; group: Group; extra: string; order: number };

const GROUPS: { id: Group; title: string; head?: string; mono?: boolean }[] = [
  { id: "here", title: "Here" },
  { id: "inherited", title: "Inherited", head: "From" },
  { id: "columns", title: "On columns", head: "Column", mono: true },
  { id: "tag", title: "By tag", head: "Tagged" },
  { id: "pattern", title: "Matching a pattern", head: "Pattern" },
];

/**
 * An object's Masks & filters tab (spec §2.2): the mask and row-filter rules
 * that select the object, grouped by how they reach it: set here, inherited
 * from an ancestor (the level named), set on one of a table's columns, by a
 * tag the object (or one of its columns) carries, or matching a pattern.
 *
 * F10: this lists the rules that select the object; it never says who sees
 * what. Only explain evaluates (an exemption, a clearance, a second rule).
 *
 * A path rule reaches the object as a grant does (`scopeCoverage`), and one
 * that names a level by pattern has its own group, as on the Permissions
 * tab. A tag rule reaches it through the tags egret resolves for it
 * (`tagKeysOn`, which documents the rule): a table's tag reaches the table
 * and every column; a column's tag reaches that column only, so a mask by it
 * is listed with the column and a row filter by it (a table request) is not.
 * Only a table in an egret catalog has tags, so no other object lists a tag
 * rule, and none asks for tags (F9).
 *
 * The rules come from `GET /perm/rules` (`policy:read`), filtered here (spec
 * §2.3, "Not new"). Add rule, Edit and Delete each need `policy:update`, the
 * permission every rule write requires (`crates/egret-perm-compile/src/routes.rs`,
 * `create_rule`, `patch_rule`, `delete_rule` and `preview`). Add rule opens
 * `RuleDialog` with a path selector naming this object. A save or a delete
 * invalidates the rules list, so this tab refetches.
 */
export function MasksFiltersTab({
  path,
  kind,
  catalog,
  tags,
}: {
  path: ObjectPath;
  /** `null` while an object's kind is not known. */
  kind: NodeKind | null;
  catalog: CatalogState;
  tags: TagsResult;
}) {
  return <CatalogGate catalog={catalog}>{(c) => <Rules path={path} kind={kind} catalog={c} tags={tags} />}</CatalogGate>;
}

function Rules({
  path,
  kind,
  catalog,
  tags,
}: {
  path: ObjectPath;
  kind: NodeKind | null;
  catalog: GovernanceCatalog;
  tags: TagsResult;
}) {
  const can = usePolicyPermissions();
  const canRead = can("policy:read");
  const canWrite = can("policy:update");
  const rulesQ = usePermListRules({ query: { enabled: canRead, retry: false } });

  const [dialog, setDialog] = useState<{ existing: RuleView | null } | null>(null);
  const [deleting, setDeleting] = useState<RuleView | null>(null);
  const queryClient = useQueryClient();
  const deleteRule = usePermDeleteRule({
    mutation: {
      onSuccess: () => {
        void queryClient.invalidateQueries({ queryKey: getPermListRulesQueryKey() });
        toast.success("Rule deleted.");
      },
      onError: (e) => toast.error(refusalMessage(e, "policy:update", "The rule could not be deleted.")),
    },
  });

  if (!canRead) {
    return (
      <Alert>
        <AlertDescription>{RULES_NEED_POLICY_READ}</AlertDescription>
      </Alert>
    );
  }

  // The object itself: the column panel's `?column=` is not this tab's subject.
  const target: ObjectPath = { catalog: path.catalog, schema: path.schema, object: path.object };
  const reach = tagReach(target, catalog, tags);

  let body;
  if (rulesQ.isError) {
    body = <TabError what="rules" onRetry={() => void rulesQ.refetch()} />;
  } else if (rulesQ.data?.status !== 200 || reach.status === "loading") {
    body = <ListSkeleton rows={4} cols={3} />;
  } else {
    const assignments = reach.status === "ok" ? reach.assignments : null;
    const rows = rulesQ.data.data.flatMap((r) => {
      const row = rowOf(r, target, catalog.id, assignments);
      return row ? [row] : [];
    });
    const noun = kind ? KIND_ICON[kind].label.toLowerCase() : "object";
    body = (
      <div className="space-y-6">
        {reach.status === "error" ? <TabError what="tags" onRetry={reach.retry} /> : null}
        {rows.length === 0 ? (
          reach.status === "error" ? null : <EmptyState icon={EyeOff} title={`No mask or row filter selects this ${noun}`} />
        ) : (
          GROUPS.map((g) => {
            const inGroup = rows
              .filter((r) => r.group === g.id)
              .sort((a, b) => a.order - b.order || a.extra.localeCompare(b.extra) || a.rule.name.localeCompare(b.rule.name));
            if (inGroup.length === 0) return null;
            return (
              <Section key={g.id} title={g.title}>
                <RuleTable
                  label={g.title}
                  rows={inGroup}
                  extra={g.head ? { head: g.head, mono: g.mono } : undefined}
                  canWrite={canWrite}
                  onEdit={(rule) => setDialog({ existing: rule })}
                  onDelete={setDeleting}
                />
              </Section>
            );
          })
        )}
      </div>
    );
  }

  return (
    <div className="space-y-4">
      {canWrite ? (
        <div className="flex justify-end">
          <Button size="sm" onClick={() => setDialog({ existing: null })}>
            <Plus aria-hidden />
            Add rule
          </Button>
        </div>
      ) : null}
      {body}

      <RuleDialog
        open={dialog !== null}
        onOpenChange={(open) => {
          if (!open) setDialog(null);
        }}
        existing={dialog?.existing ?? null}
        initialSelector={{ type: "path", ...scopeOf(target, catalog.id) }}
      />
      <ConfirmDeleteDialog
        open={deleting !== null}
        onOpenChange={(open) => {
          if (!open) setDeleting(null);
        }}
        title={deleting ? `Delete "${deleting.name}"?` : "Delete rule?"}
        description={deleteDescription(deleting)}
        onConfirm={() => {
          if (deleting) deleteRule.mutate({ id: deleting.id });
          setDeleting(null);
        }}
      />
    </div>
  );
}

/**
 * How rule `r` reaches `p` (an object, never a column), or `null`.
 * `assignments` is `p`'s table's tags, or `null` when none can be known.
 */
function rowOf(r: RuleView, p: ObjectPath, catalogId: string, assignments: ResourceTagDto[] | null): Row | null {
  if (r.selector.type === "tag") {
    if (assignments === null) return null;
    if (ruleSelects(r, p, catalogId, tagKeysOn(assignments, p)) === "tag") {
      return { rule: r, group: "tag", extra: "On the table", order: 0 };
    }
    // A mask by a tag on some of the table's columns reaches those columns.
    if (r.kind.type !== "mask") return null;
    const columns = new Map<string, string>();
    for (const a of assignments) {
      if (a.column === "" || columns.has(a.column.toLowerCase())) continue;
      if (ruleSelects(r, { ...p, column: a.column }, catalogId, tagKeysOn(assignments, { ...p, column: a.column })) === "tag") {
        columns.set(a.column.toLowerCase(), a.column);
      }
    }
    if (columns.size === 0) return null;
    const names = [...columns.values()].sort((a, b) => a.localeCompare(b));
    return { rule: r, group: "tag", extra: `${names.length === 1 ? "On column" : "On columns"} ${names.join(", ")}`, order: 1 };
  }

  const reach = scopeCoverage(r.selector, p, catalogId);
  if (reach === null) return null;
  const patterns = patternLevels(r.selector);
  if (patterns.length > 0) return { rule: r, group: "pattern", extra: patterns.map(patternText).join(", "), order: 0 };
  if (reach === "here") return { rule: r, group: "here", extra: "", order: 0 };
  if (reach === "child") return { rule: r, group: "columns", extra: r.selector.column ?? "", order: 0 };
  const order = Object.keys(LEVEL_LABEL).indexOf(reach.inheritedFrom);
  return { rule: r, group: "inherited", extra: LEVEL_LABEL[reach.inheritedFrom], order };
}

/** `table “*”`. */
function patternText(p: PatternLevel): string {
  return `${p.level} “${p.value}”`;
}

/** The delete confirmation's text; a rule that reaches more than this object says how far. */
function deleteDescription(r: RuleView | null): string {
  const removed = "It is permanently removed and every profile it reaches recompiles.";
  if (r?.selector.type === "tag") {
    return `This rule selects everything tagged “${r.tag_name || r.selector.key}”, in every catalog, not one object. ${removed}`;
  }
  const patterns = r ? patternLevels(r.selector) : [];
  if (patterns.length === 0) return "This rule is permanently removed and every profile it reaches recompiles.";
  const reach = patterns.map((p) => `every ${p.level} matching “${p.value}”`).join(" and ");
  return `This rule selects ${reach}, not one object. ${removed}`;
}

function RuleTable({
  label,
  rows,
  extra,
  canWrite,
  onEdit,
  onDelete,
}: {
  label: string;
  rows: Row[];
  extra?: { head: string; mono?: boolean };
  canWrite: boolean;
  onEdit: (r: RuleView) => void;
  onDelete: (r: RuleView) => void;
}) {
  return (
    <Table aria-label={label}>
      <TableHeader>
        <TableRow>
          <TableHead>Rule</TableHead>
          <TableHead>Kind</TableHead>
          {extra ? <TableHead>{extra.head}</TableHead> : null}
          {canWrite ? (
            <TableHead className="w-24">
              <span className="sr-only">Actions</span>
            </TableHead>
          ) : null}
        </TableRow>
      </TableHeader>
      <TableBody>
        {rows.map(({ rule, extra: extraText }) => (
          <TableRow key={rule.id}>
            <TableCell className="whitespace-normal">
              <div className="space-y-0.5">
                <p className="text-sm font-medium break-words">{rule.name}</p>
                <p className="text-xs break-words text-muted-foreground">{formatSelector(rule)}</p>
              </div>
            </TableCell>
            <TableCell className="whitespace-normal break-words">{formatRuleKind(rule)}</TableCell>
            {extra ? <TableCell className={extra.mono ? "font-mono text-xs" : undefined}>{extraText}</TableCell> : null}
            {canWrite ? (
              <TableCell className="text-right">
                <div className="flex justify-end gap-1">
                  <Button variant="ghost" size="icon" aria-label={`Edit ${rule.name}`} onClick={() => onEdit(rule)}>
                    <Pencil aria-hidden className="size-4" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    className="text-muted-foreground hover:text-destructive"
                    aria-label={`Delete ${rule.name}`}
                    onClick={() => onDelete(rule)}
                  >
                    <Trash2 aria-hidden className="size-4" />
                  </Button>
                </div>
              </TableCell>
            ) : null}
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
