import { useState, type ReactNode } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Columns3, Loader2, Lock, Tag as TagIcon } from "lucide-react";
import { toast } from "sonner";
import { ApiError } from "@/api/client";
import {
  useControlPlaneCatalogAssignResourceTag,
  useControlPlaneCatalogUnassignResourceTag,
} from "@/api/generated/control-plane-catalog/control-plane-catalog";
import { useControlPlaneTagsListTags } from "@/api/generated/control-plane-tags/control-plane-tags";
import type { RuleView } from "@/api/generated/model";
import { usePermListRules } from "@/api/generated/perm/perm";
import { invalidateTagAssignment, type Tag } from "@/api/tags";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { tableTagMaskConflict, type TableTagMaskConflict } from "@/routes/governance/apiErrors";
import { useCanEditCatalogTags } from "@/routes/governance/ClassificationChip";
import { DEFAULT_COLOR } from "@/routes/governance/classifications/tagParts";
import { refusalMessage, usePolicyPermissions } from "@/routes/governance/policyWrites";
import { rulesOnTag } from "@/routes/governance/ruleMatch";
import { fullPath } from "@/routes/governance/data/objectData";
import { failedText } from "@/routes/governance/tabParts";
import { TERM } from "@/routes/governance/glossary";

/**
 * Why Add classification is disabled on an object outside egret's catalogs (F9: the tag
 * API is keyed by egret's warehouse, so only an egret catalog can carry a
 * tag). Also this module's bundle marker (`scripts/assert-governance-data-lazy.mjs`).
 */
export const TAGS_EGRET_ONLY = "Classifications can be attached to egret-managed catalogs only";

/** What the tag picker says when `GET /tags` is refused (it needs `tag:read`, which writing a tag does not imply). */
const TAG_LIST_NEEDS_TAG_READ = "Reading the classification list needs the tag:read permission.";

/** Spec C5: what a classification on a whole table does, shown when no mask is known to select it. */
const TABLE_LEVEL_NOTE =
  "On a whole table, this classification counts for clearance on every column. A rule that masks by classification needs it on columns, not on the table.";

/** The table or column a tag is added to. */
export type TagTarget = {
  catalog: string;
  catalogId: string;
  warehouseId: string;
  schema: string;
  table: string;
  /** Absent for the table itself. */
  column?: string;
  /** The ids of the tags already on this table or column, which are not offered again. */
  assigned: string[];
  /**
   * The table's columns as its source lists them, for "Attach to columns
   * instead" (spec C5); absent when they could not be listed or for a column.
   */
  columns?: string[];
};

/** The conflict sentence (spec C5): one mask, the classification and the table. */
function conflictSentence(mask: string, tag: string, schema: string, table: string): string {
  return `${mask} masks by ${tag}, so ${tag} on the whole table ${schema}.${table} would make it unreadable.`;
}

const columnsWord = (n: number) => `${n} ${n === 1 ? "column" : "columns"}`;

/**
 * Add one tag to a table or a column (spec §2.2, F9) through the existing
 * resource-tags write, `PUT /warehouses/{warehouse_id}/resource-tags`, gated
 * on `catalog_registry:update` (`crates/egret-bi/src/catalog_tags.rs`,
 * `WAREHOUSE_WRITE`). The picker offers the tag catalogue (`GET /tags`,
 * `tag:read`) less the tags already on the target; each option shows its
 * swatch, key, the first line of its description, "Clearance" when gated and,
 * with `policy:read`, how many rules select it (spec C4).
 *
 * **A whole table (spec C5).** A mask that selects by a classification
 * type-checks against every column, so the classification on a whole table
 * makes the table unreadable, and the server refuses the pair
 * (`table_tag_mask_conflict`). With `policy:read` the dialog knows before
 * submit: when a mask rule selects the picked classification (`rulesOnTag`),
 * it says so, Add is `aria-disabled` with that sentence as its reason, and
 * **Attach to columns instead** switches to a checkbox list of the table's
 * columns, one `PUT` per ticked column in the table's order, stopping at the
 * first failure with the ticks kept. Without `policy:read` (or with no such
 * mask) it shows a short note, and the server decides: its coded refusal is
 * shown as the same sentence, with the same way out.
 *
 * On success it refetches what an attach changes (`invalidateTagAssignment`),
 * closes and toasts. On failure the server's sentence shows beside the field
 * and the choice is kept: a 409 names the two rules the tag would make
 * overlap, a 400 the level a coordinate cannot be stored at.
 */
export function AddTagDialog({
  target,
  onOpenChange,
}: {
  /** `null` while closed. */
  target: TagTarget | null;
  onOpenChange: (open: boolean) => void;
}) {
  return (
    <Dialog open={target !== null} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        {target ? <AddTagForm target={target} onDone={() => onOpenChange(false)} /> : null}
      </DialogContent>
    </Dialog>
  );
}

function AddTagForm({ target, onDone }: { target: TagTarget; onDone: () => void }) {
  const [tagId, setTagId] = useState("");
  const [mode, setMode] = useState<"whole" | "columns">("whole");
  const [ticked, setTicked] = useState<string[]>([]);
  const [columnsError, setColumnsError] = useState<unknown>(null);
  const [columnsPending, setColumnsPending] = useState(false);
  const tagsQ = useControlPlaneTagsListTags({ query: { retry: false } });
  const canReadRules = usePolicyPermissions()("policy:read");
  const rulesQ = usePermListRules({ query: { enabled: canReadRules, retry: false } });
  const queryClient = useQueryClient();
  const assign = useControlPlaneCatalogAssignResourceTag<ApiError>();

  const path = fullPath({ catalog: target.catalog, schema: target.schema, object: target.table, column: target.column });
  const assigned = new Set(target.assigned);
  // The control-plane route answers `Tag` (with clearance); the generated `TagDto` is egret-bi's narrower schema (`api/tags.ts`).
  const catalogue = tagsQ.data?.status === 200 ? (tagsQ.data.data as unknown as Tag[]) : undefined;
  const offered = catalogue?.filter((t) => !assigned.has(t.id)).sort((a, b) => a.name.localeCompare(b.name));
  const rules: RuleView[] | undefined = canReadRules && rulesQ.data?.status === 200 ? rulesQ.data.data : undefined;
  const picked = offered?.find((t) => t.id === tagId);
  const onTable = target.column === undefined;
  const noun = onTable ? "table" : "column";

  // Known before submit (spec C5): a mask rule selects the picked classification and the target is a whole table.
  const masks = onTable && picked && rules ? rulesOnTag(rules, picked).filter((r) => r.kind.type === "mask") : [];
  const preConflict =
    masks.length > 0 && picked ? conflictSentence(masks[0].name, picked.name, target.schema, target.table) : null;
  // What the server said, when it refused the pair.
  const serverConflict = tableTagMaskConflict(assign.error);

  const succeed = (message: string) => {
    toast.success(message);
    onDone();
  };

  const attachWhole = () => {
    if (!picked || preConflict || assign.isPending) return;
    assign
      .mutateAsync({
        warehouseId: target.warehouseId,
        data: { schema: target.schema, table: target.table, column: target.column ?? "", tag_id: picked.id },
      })
      .then(
        () => {
          invalidateTagAssignment(queryClient, picked.id, target.warehouseId);
          succeed("Classification attached.");
        },
        () => {
          // Shown beside the field from `assign.error`; the choice is kept.
        },
      );
  };

  const attachColumns = async () => {
    if (!picked || columnsPending) return;
    // In the table's order, whatever order they were ticked in.
    const order = (target.columns ?? []).filter((c) => ticked.includes(c));
    if (order.length === 0) return;
    setColumnsPending(true);
    setColumnsError(null);
    let done = 0;
    try {
      for (const column of order) {
        await assign.mutateAsync({
          warehouseId: target.warehouseId,
          data: { schema: target.schema, table: target.table, column, tag_id: picked.id },
        });
        done++;
      }
    } catch (e) {
      setColumnsError(e);
    } finally {
      setColumnsPending(false);
      if (done > 0) invalidateTagAssignment(queryClient, picked.id, target.warehouseId);
    }
    if (done === order.length) succeed(`Classification attached to ${columnsWord(done)}.`);
    // A partial run keeps the remaining ticks: the attached ones are on their columns now.
    else if (done > 0) setTicked((prev) => prev.filter((c) => !order.slice(0, done).includes(c)));
  };

  let picker: ReactNode;
  if (tagsQ.isError) {
    const refused = tagsQ.error instanceof ApiError && tagsQ.error.status === 403;
    picker = (
      <Alert variant="destructive">
        <AlertDescription>{refused ? TAG_LIST_NEEDS_TAG_READ : failedText("classification list")}</AlertDescription>
      </Alert>
    );
  } else if (offered === undefined) {
    picker = <ListSkeleton rows={1} cols={1} />;
  } else if (offered.length === 0) {
    picker = (
      <p className="text-sm text-muted-foreground">
        {assigned.size > 0 && catalogue && catalogue.length > 0
          ? `Every classification is already on this ${noun}.`
          : "No classifications are defined yet."}
      </p>
    );
  } else {
    const wholeError = mode === "whole" && assign.isError && !serverConflict;
    picker = (
      <div className="space-y-1.5">
        <Label htmlFor="add-tag-tag">{TERM.classification}</Label>
        <Select
          value={tagId}
          onValueChange={(v) => {
            setTagId(v);
            setMode("whole");
            setColumnsError(null);
            assign.reset();
          }}
        >
          <SelectTrigger id="add-tag-tag" className="w-full" aria-describedby={wholeError ? "add-tag-error" : undefined}>
            <SelectValue placeholder="Choose a classification" />
          </SelectTrigger>
          <SelectContent>
            {offered.map((t) => (
              <SelectItem key={t.id} value={t.id} textValue={t.name} description={<OptionDetails tag={t} rules={rules} />}>
                <span
                  aria-hidden="true"
                  className="inline-block size-2.5 shrink-0 rounded-full border border-border"
                  style={{ backgroundColor: t.color || DEFAULT_COLOR }}
                />
                {t.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        {wholeError ? (
          <p id="add-tag-error" role="alert" className="text-sm text-destructive">
            {refusalMessage(assign.error, "catalog_registry:update", "The classification could not be attached. Try again.")}
          </p>
        ) : null}
      </div>
    );
  }

  const canColumns = onTable && (target.columns?.length ?? 0) > 0;
  const toColumns = () => {
    setMode("columns");
    assign.reset();
  };

  let tableNotes: ReactNode = null;
  if (onTable && picked && mode === "whole") {
    if (preConflict) {
      tableNotes = (
        <ConflictAlert title={`On the whole table, ${picked.name} would make ${target.schema}.${target.table} unreadable.`}>
          <p id="add-tag-conflict">{preConflict}</p>
          <ColumnsWayOut canColumns={canColumns} onColumns={toColumns} />
        </ConflictAlert>
      );
    } else if (serverConflict) {
      tableNotes = <ServerConflict conflict={serverConflict} canColumns={canColumns} onColumns={toColumns} onRemoved={() => assign.reset()} />;
    } else {
      tableNotes = <p className="text-xs text-muted-foreground">{TABLE_LEVEL_NOTE}</p>;
    }
  }

  const columnsForm =
    onTable && picked && mode === "columns" ? (
      <fieldset className="space-y-2">
        <legend className="text-sm font-medium">{`Columns of ${target.schema}.${target.table} to attach ${picked.name} to`}</legend>
        <ul className="max-h-48 space-y-1 overflow-y-auto">
          {(target.columns ?? []).map((c) => {
            const id = `add-tag-col-${c}`;
            return (
              <li key={c} className="flex items-center gap-2">
                <Checkbox
                  id={id}
                  checked={ticked.includes(c)}
                  onCheckedChange={(v) => setTicked((prev) => (v === true ? [...prev, c] : prev.filter((x) => x !== c)))}
                />
                <Label htmlFor={id} className="font-mono text-xs font-normal">
                  {c}
                </Label>
              </li>
            );
          })}
        </ul>
        {columnsError ? (
          <p role="alert" className="text-sm text-destructive">
            {tableTagMaskConflict(columnsError)?.error ??
              refusalMessage(columnsError, "catalog_registry:update", "The classification could not be attached. Try again.")}
          </p>
        ) : null}
        <Button type="button" variant="link" size="sm" className="h-auto px-0" onClick={() => setMode("whole")}>
          Attach to the whole table instead
        </Button>
      </fieldset>
    ) : null;

  // Why the submit does nothing, when it does nothing (FE-ACTION-4, spec N22: aria-disabled with the reason tied).
  let blockedBy: string | undefined;
  if (offered && offered.length > 0 && !tagId) blockedBy = "add-tag-needs";
  else if (mode === "whole" && preConflict) blockedBy = "add-tag-conflict";
  else if (mode === "columns" && ticked.length === 0) blockedBy = "add-tag-tick";
  const pending = assign.isPending || columnsPending;

  return (
    <form
      className="grid gap-4"
      onSubmit={(e) => {
        e.preventDefault();
        if (blockedBy || pending) return;
        if (mode === "columns") void attachColumns();
        else attachWhole();
      }}
    >
      <DialogHeader>
        <DialogTitle>{TERM.addClassification}</DialogTitle>
        <DialogDescription>
          <span className="font-mono text-xs break-all">{path}</span>
        </DialogDescription>
      </DialogHeader>
      {picker}
      {tableNotes}
      {columnsForm}
      <DialogFooter className="items-center">
        {blockedBy === "add-tag-needs" ? (
          <p id="add-tag-needs" className="text-xs text-muted-foreground">
            Choose a classification to attach.
          </p>
        ) : blockedBy === "add-tag-tick" ? (
          <p id="add-tag-tick" className="text-xs text-muted-foreground">
            Tick the columns to attach it to.
          </p>
        ) : null}
        <Button type="submit" aria-disabled={blockedBy || pending ? true : undefined} aria-describedby={blockedBy}>
          {pending ? <Loader2 aria-hidden className="animate-spin" /> : mode === "columns" ? <Columns3 aria-hidden /> : <TagIcon aria-hidden />}
          {mode === "columns" ? `Attach to ${columnsWord(ticked.length)}` : TERM.addClassification}
        </Button>
      </DialogFooter>
    </form>
  );
}

/** An option's secondary line: key, the description's first line, clearance and, with `policy:read`, its rules. */
function OptionDetails({ tag, rules }: { tag: Tag; rules: RuleView[] | undefined }) {
  const firstLine = tag.description.split("\n")[0]?.trim();
  const gated = (tag.required_roles?.length ?? 0) + (tag.required_groups?.length ?? 0) > 0;
  const ruleCount = rules ? rulesOnTag(rules, tag).length : 0;
  return (
    <>
      <span className="font-mono text-xs">{tag.key}</span>
      {firstLine ? <span className="truncate">{firstLine}</span> : null}
      {gated ? (
        <span className="inline-flex items-center gap-1">
          <Lock aria-hidden className="size-3" />
          {TERM.clearance}
        </span>
      ) : null}
      {ruleCount > 0 ? <span className="tabular-nums">{`${ruleCount} ${ruleCount === 1 ? "rule" : "rules"}`}</span> : null}
    </>
  );
}

function ConflictAlert({ title, children }: { title: string; children: ReactNode }) {
  return (
    <Alert>
      <AlertTitle className="whitespace-normal">{title}</AlertTitle>
      <AlertDescription className="space-y-2">{children}</AlertDescription>
    </Alert>
  );
}

function ColumnsWayOut({ canColumns, onColumns }: { canColumns: boolean; onColumns: () => void }) {
  if (!canColumns) {
    return <p className="text-xs">The table's columns could not be listed: attach it from each column's panel.</p>;
  }
  return (
    <Button type="button" variant="outline" size="sm" onClick={onColumns}>
      <Columns3 aria-hidden />
      Attach to columns instead
    </Button>
  );
}

/**
 * The server's `table_tag_mask_conflict` (spec C5), as the same short
 * sentence. When it names a table-level attachment already stored
 * (`warehouseId`), a viewer who may edit catalog tags can remove that one.
 */
function ServerConflict({
  conflict,
  canColumns,
  onColumns,
  onRemoved,
}: {
  conflict: TableTagMaskConflict;
  canColumns: boolean;
  onColumns: () => void;
  onRemoved: () => void;
}) {
  const canEdit = useCanEditCatalogTags();
  const queryClient = useQueryClient();
  const remove = useControlPlaneCatalogUnassignResourceTag<ApiError>();
  const warehouseId = conflict.warehouseId;
  return (
    <div role="alert" className="space-y-2 rounded-md border border-destructive/50 p-3 text-sm text-destructive">
      <p>
        {conflictSentence(conflict.mask, conflict.tag, conflict.schema, conflict.table)}
        {conflict.moreTables ? " Other tables carry it too." : ""}
      </p>
      <div className="flex flex-wrap gap-2">
        <ColumnsWayOut canColumns={canColumns} onColumns={onColumns} />
        {warehouseId && canEdit ? (
          <Button
            type="button"
            variant="outline"
            size="sm"
            aria-disabled={remove.isPending ? true : undefined}
            onClick={() => {
              if (remove.isPending) return;
              remove
                .mutateAsync({
                  warehouseId,
                  data: { schema: conflict.schema, table: conflict.table, column: "", tag_id: conflict.tagId },
                })
                .then(
                  () => {
                    invalidateTagAssignment(queryClient, conflict.tagId, warehouseId);
                    toast.success(`Removed ${conflict.tag} from ${conflict.schema}.${conflict.table}.`);
                    onRemoved();
                  },
                  () => {},
                );
            }}
          >
            Remove from table
          </Button>
        ) : null}
      </div>
      {remove.isError ? (
        <p>{refusalMessage(remove.error, "catalog_registry:update", "The classification could not be removed. Try again.")}</p>
      ) : null}
      <details className="text-xs text-muted-foreground">
        <summary>Details</summary>
        {conflict.error}
      </details>
    </div>
  );
}
