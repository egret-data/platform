import { useState } from "react";
import { Link, useLocation } from "react-router-dom";
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
import type { RuleEntry } from "@/routes/governance/ruleDraft";
import { useWorkspaceNames } from "@/routes/governance/openInData";
import { formatRuleKind, formatSelector, type WorkspaceNames } from "@/routes/governance/permFormat";
import { PrincipalNames } from "@/routes/governance/PrincipalNames";
import { refusalMessage, usePolicyPermissions } from "@/routes/governance/policyWrites";
import { KIND_ICON, type NodeKind } from "@/routes/governance/data/CatalogTree";
import {
  insideGroup,
  insideName,
  patternLevels,
  ruleSelects,
  scopeCoverage,
  tagNamesOn,
  type PatternLevel,
} from "@/routes/governance/data/coverage";
import { tagReach, type CatalogState, type TagsResult } from "@/routes/governance/data/objectData";
import type { ObjectPath } from "@/routes/governance/data/objectPath";
import { HandWrittenSection } from "@/routes/governance/handwritten/HandWrittenSection";
import { CatalogGate } from "@/routes/governance/data/tabs/TabStates";
import { ACTIONS_CELL, Section, TabError } from "@/routes/governance/tabParts";
import { tagUnresolved } from "@/routes/governance/ruleMatch";
import { TERM } from "@/routes/governance/glossary";

/**
 * What a viewer without `policy:read` reads instead of the rules. Also this
 * module's bundle marker (`scripts/assert-governance-data-lazy.mjs`).
 */
export const RULES_NEED_POLICY_READ = "Reading rules needs the policy:read permission, which this account does not have.";

/** M1: what a schema or catalog page says, since it never lists a tag rule (see {@link MasksFiltersTab}). */
export const TAG_RULES_ON_TABLES = "Rules on classifications are listed on each table's page.";

const LEVEL_LABEL = { catalog: "Catalog", schema: "Schema", table: "Table" } as const;

type Group = "here" | "inherited" | "inside" | "tag" | "pattern";

/** One listed rule: its group, and what the group's extra column says of it. */
type Row = { rule: RuleView; group: Group; extra: string; order: number };

const GROUPS: { id: Group; title: string; head?: string; mono?: boolean }[] = [
  { id: "here", title: "Here" },
  { id: "inherited", title: "Inherited", head: "From" },
  // Titled for the page's level by `insideGroup`.
  { id: "inside", title: "On columns", head: "Column", mono: true },
  { id: "tag", title: "By classification", head: TERM.classification },
  { id: "pattern", title: "Matching a pattern", head: "Pattern" },
];

/**
 * An object's Rules tab (spec §2.2; "Masks & filters" before the glossary): the mask and row-filter rules
 * that select the object, grouped by how they reach it: set here, inherited
 * from an ancestor (the level named), set on something inside it (a table's
 * columns; a schema's or a catalog's tables and columns, each named), by a
 * tag the object (or one of its columns) carries, or matching a pattern
 * (including a pattern below a schema or catalog, such as `table = "*"`).
 * A tag rule is listed on a table only: a schema or catalog page does not
 * read the tags of the tables inside it, and says so.
 *
 * F10: this lists the rules that select the object; it never says who sees
 * what. Only explain evaluates (an exemption, a clearance, a second rule).
 *
 * A path rule reaches the object as a grant does (`scopeCoverage`), and one
 * that names a level by pattern has its own group, as on the Permissions
 * tab. A tag rule reaches it through the tag names egret resolves for it
 * (`tagNamesOn` and `ruleSelects`, which document the rule; the engine
 * matches a tag's NAME, so a rule reaches every tag sharing its tag's name):
 * a table's tag reaches the table
 * and every column; a column's tag reaches that column only, so a mask by it
 * is listed with the column and a row filter by it (a table request) is not.
 * Only a table in an egret catalog has tags, so no other object lists a tag
 * rule, and none asks for tags (F9). A tag rule whose tag no longer resolves
 * selects nothing (the compiler refuses it); on a table such rules are named
 * in a note, never silently dropped.
 *
 * Each row shows the rule's exemptions, so a rule never reads as applying to
 * everyone; a `workspace_members` exemption reads by the workspace's name. Inside a group, masks come before row filters.
 *
 * The rules come from `GET /perm/rules` (`policy:read`), filtered here (spec
 * §2.3, "Not new"). Add rule, Edit and Delete each need `policy:update`, the
 * permission every rule write requires (`crates/egret-perm-compile/src/routes.rs`,
 * `create_rule`, `patch_rule`, `delete_rule` and `preview`). Add rule opens
 * `RuleDialog` from this object's entry point (spec C8.3): on a table, a mask
 * on the table's path with Column left empty (every column) and Row filter
 * one click away; on a catalog or schema, a mask on that path. A save or a delete
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
  return (
    <CatalogGate catalog={catalog}>
      {(c) => (
        <div className="space-y-6">
          <Rules path={path} kind={kind} catalog={c} tags={tags} />
          {/* F12 (#4b spec §6): the hand-written masking and row-filter policies that reach the object. */}
          <HandWrittenSection catalogId={c.id} schema={path.schema} table={path.object} show="rules" />
        </div>
      )}
    </CatalogGate>
  );
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
  const workspaceName = useWorkspaceNames(catalog);

  const [dialog, setDialog] = useState<RuleEntry | null>(null);
  const [deleting, setDeleting] = useState<RuleView | null>(null);
  const queryClient = useQueryClient();
  const deleteRule = usePermDeleteRule({
    mutation: {
      onSuccess: (_data, { id }) => {
        void queryClient.invalidateQueries({ queryKey: getPermListRulesQueryKey() });
        const name = rulesQ.data?.status === 200 ? rulesQ.data.data.find((r) => r.id === id)?.name : undefined;
        toast.success(name ? `${TERM.rule} ${name} deleted.` : `${TERM.rule} deleted.`);
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
    const unresolved = assignments === null ? [] : rulesQ.data.data.filter(tagUnresolved).map((r) => r.name).sort();
    body = (
      <div className="space-y-6">
        {reach.status === "error" ? <TabError what="classifications" onRetry={reach.retry} /> : null}
        {target.object === undefined ? (
          <p className="text-xs text-muted-foreground">{TAG_RULES_ON_TABLES}</p>
        ) : null}
        {unresolved.length > 0 ? (
          <Alert>
            <AlertDescription>
              {unresolved.length === 1
                ? `Rule ${unresolved[0]} uses a classification that no longer exists, so it applies to nothing.`
                : `Rules ${unresolved.join(", ")} use classifications that no longer exist, so they apply to nothing.`}
            </AlertDescription>
          </Alert>
        ) : null}
        {rows.length === 0 ? (
          reach.status === "error" ? null : <EmptyState icon={EyeOff} title={`No rule applies to this ${noun}.`} />
        ) : (
          GROUPS.map((g) => {
            const inGroup = rows
              .filter((r) => r.group === g.id)
              .sort(
                (a, b) =>
                  kindRank(a.rule) - kindRank(b.rule) ||
                  a.order - b.order ||
                  a.extra.localeCompare(b.extra) ||
                  a.rule.name.localeCompare(b.rule.name),
              );
            if (inGroup.length === 0) return null;
            const { title, head } = g.id === "inside" ? insideGroup(target) : g;
            return (
              <Section key={g.id} title={title}>
                <RuleTable
                  label={title}
                  rows={inGroup}
                  extra={head ? { head, mono: g.mono } : undefined}
                  canWrite={canWrite}
                  workspaceName={workspaceName}
                  onEdit={(rule) => setDialog({ kind: "edit", rule })}
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
          <Button size="sm" onClick={() => setDialog(newRuleEntry(target, catalog.id))}>
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
        entry={dialog ?? newRuleEntry(target, catalog.id)}
      />
      <ConfirmDeleteDialog
        open={deleting !== null}
        onOpenChange={(open) => {
          if (!open) setDeleting(null);
        }}
        title={deleting ? `Delete "${deleting.name}"?` : "Delete rule?"}
        description={deleteDescription(deleting)}
        confirmText={deleting?.name ?? "delete"}
        confirmLabel={TERM.delete}
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
    if (ruleSelects(r, p, catalogId, tagNamesOn(assignments, p)) === "tag") {
      return { rule: r, group: "tag", extra: "On the table", order: 0 };
    }
    // A mask by a tag on some of the table's columns reaches those columns.
    if (r.kind.type !== "mask") return null;
    const columns = new Map<string, string>();
    for (const a of assignments) {
      if (a.column === "" || columns.has(a.column.toLowerCase())) continue;
      if (ruleSelects(r, { ...p, column: a.column }, catalogId, tagNamesOn(assignments, { ...p, column: a.column })) === "tag") {
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
  if (reach === "child") return { rule: r, group: "inside", extra: insideName(r.selector, p), order: 0 };
  const order = Object.keys(LEVEL_LABEL).indexOf(reach.inheritedFrom);
  return { rule: r, group: "inherited", extra: LEVEL_LABEL[reach.inheritedFrom], order };
}

/** Masks first, then row filters. */
function kindRank(r: RuleView): number {
  return r.kind.type === "mask" ? 0 : 1;
}

/** `table “*”`. */
function patternText(p: PatternLevel): string {
  return `${p.level} “${p.value}”`;
}

/** The delete confirmation's text; a rule that reaches more than this object says how far. */
function deleteDescription(r: RuleView | null): string {
  const removed = "It is permanently removed and every profile it reaches recompiles.";
  if (r?.selector.type === "tag") {
    return `This rule applies to everything carrying the classification “${r.tag_name || r.selector.key}”, in every catalog, not one object. ${removed}`;
  }
  const patterns = r ? patternLevels(r.selector) : [];
  if (patterns.length === 0) return "This rule is permanently removed and every profile it reaches recompiles.";
  const reach = patterns.map((p) => `every ${p.level} matching “${p.value}”`).join(" and ");
  return `This rule selects ${reach}, not one object. ${removed}`;
}

/** A rule's page, opened from this Data page, so its way back returns here (`?from=data:`, spec C1). */
function ruleHref(ruleId: string, back: string): string {
  return `/governance/rules/${encodeURIComponent(ruleId)}?from=${encodeURIComponent(`data:${encodeURIComponent(back)}`)}`;
}

function RuleTable({
  label,
  rows,
  extra,
  canWrite,
  workspaceName,
  onEdit,
  onDelete,
}: {
  label: string;
  rows: Row[];
  extra?: { head: string; mono?: boolean };
  canWrite: boolean;
  workspaceName: WorkspaceNames;
  onEdit: (r: RuleView) => void;
  onDelete: (r: RuleView) => void;
}) {
  const location = useLocation();
  const back = location.pathname + location.search;
  return (
    <Table aria-label={label}>
      <TableHeader>
        <TableRow>
          <TableHead>{TERM.rule}</TableHead>
          <TableHead>{TERM.kind}</TableHead>
          {extra ? <TableHead>{extra.head}</TableHead> : null}
          <TableHead>{TERM.exempt}</TableHead>
          {canWrite ? (
            <TableHead className={ACTIONS_CELL}>
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
                <p className="text-sm font-medium break-words">
                  <Link to={ruleHref(rule.id, back)} className="text-primary hover:underline">
                    {rule.name}
                  </Link>
                </p>
                <p className="text-xs break-words text-muted-foreground">{formatSelector(rule)}</p>
              </div>
            </TableCell>
            <TableCell className="whitespace-normal break-words">{formatRuleKind(rule)}</TableCell>
            {extra ? <TableCell className={extra.mono ? "font-mono text-xs" : undefined}>{extraText}</TableCell> : null}
            <TableCell className="whitespace-normal break-words">
              {rule.exempt.include.length > 0 ? (
                <PrincipalNames list={rule.exempt.include} workspaceName={workspaceName} />
              ) : (
                <span className="text-muted-foreground">—</span>
              )}
            </TableCell>
            {canWrite ? (
              <TableCell className={`${ACTIONS_CELL} text-right`}>
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

/** The rule dialog's entry point for a new rule on `p` (spec C8.3): a table, or a catalog or schema. */
function newRuleEntry(p: ObjectPath, catalogId: string): RuleEntry {
  if (p.schema !== undefined && p.object !== undefined) {
    return { kind: "table", catalogId, schema: p.schema, table: p.object };
  }
  return { kind: "scope", catalogId, schema: p.schema };
}
