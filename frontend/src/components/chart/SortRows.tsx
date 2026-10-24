import { useEffect } from "react";
import { ArrowDown, ArrowUp, ChevronDown, ChevronUp, Trash2 } from "lucide-react";
import type { ChartOrderBy } from "@/api/charts";
import { Label } from "@/components/ui/label";
import {
  Select, SelectTrigger, SelectValue, SelectContent, SelectItem,
} from "@/components/ui/select";

import { FIELD_LABEL as LABEL_CLASS, ICON_BUTTON } from "@/components/chart/settings/FieldLabel";
import { displayName, fieldTitle, type FieldLabels } from "@/components/chart/config/field-names";

/**
 * The order a chart's rows come back in — the studio's first AUTHORED query clause (filters still
 * arrive only from drilling). Sort is a query concern, so this edits `SharedQueryState.orderby`
 * beside `filters` and `rowLimit` rather than any viz's settings, which is how Metabase models it
 * too: a notebook step, not a visualization option.
 *
 * Ported from Metabase's `SortStep` (querying/notebook/components/SortStep):
 *
 * - several clauses, and **the array's order is the sort precedence**;
 * - a new clause is ASCENDING (`Lib.orderBy(..., "asc")`);
 * - a clause can change its own field, and the picker hides fields already sorted on except the
 *   clause's own — Metabase's filter is literally `isSelected || !isAlreadyUsed`;
 * - one click flips the direction, and one removes the clause.
 *
 * Deliberate differences, both of them narrowings:
 *
 * - **Arrow buttons reorder, not drag-and-drop.** Metabase's `ClauseStep` wires `onReorder` to a
 *   drag handle. `@dnd-kit` is already a dependency here and `SeriesListSection` uses it, so this
 *   is cheap to upgrade later; two chevrons keep the diff reviewable and are the idiom
 *   `ConditionalFormattingSection` already established for an ordered list in this panel.
 * - **The field picker is a flat `Select`, not a grouped column popover.** Metabase groups
 *   `orderableColumns` by table because a query can join several; an egret chart reads one dataset
 *   and the options are the handful of fields its own spec selects, so there is nothing to group.
 *
 * `fields` must come from `sortableFields(spec)` — see there for why the dataset's column list is
 * the wrong source.
 */
export function SortRows({ fields, labels, value, onChange }: {
  fields: string[];
  /** Display names, as the field pickers show them (KI-050). A clause still stores the machine name. */
  labels?: FieldLabels;
  value: ChartOrderBy[];
  onChange: (next: ChartOrderBy[]) => void;
}) {
  // A sort names a field of the SPEC, and the spec moves under it: change a bar chart's x-axis and
  // the clause saved on the old one names a column the query no longer groups by. `orderby_sql`
  // resolves it against the dataset anyway (its third arm) and emits an ORDER BY that appears in no
  // GROUP BY, so the engine — not egret — is what fails. Drop it instead. Skipped while `fields` is
  // empty, which is the "nothing configured yet" state rather than "everything went away".
  useEffect(() => {
    if (fields.length === 0) return;
    const kept = value.filter((o) => fields.includes(o.field));
    if (kept.length !== value.length) onChange(kept);
  });

  if (fields.length === 0) return null;

  const used = new Set(value.map((o) => o.field));
  const unused = fields.filter((f) => !used.has(f));
  const replace = (i: number, clause: ChartOrderBy) =>
    onChange(value.map((o, j) => (j === i ? clause : o)));
  const move = (i: number, dir: -1 | 1) => {
    const j = i + dir;
    if (j < 0 || j >= value.length) return;
    const next = [...value];
    [next[i], next[j]] = [next[j], next[i]];
    onChange(next);
  };

  return (
    <div className="space-y-1">
      <Label className={LABEL_CLASS}>Sort</Label>
      <div className="space-y-1.5">
        {value.map((clause, i) => (
          <div key={i} className="flex items-center gap-1">
            <Select value={clause.field} onValueChange={(v) => { if (v) replace(i, { ...clause, field: v }); }}>
              <SelectTrigger aria-label={`sort ${i + 1} field`} className="h-8 flex-1 text-sm">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {fields.filter((f) => f === clause.field || !used.has(f))
                  .map((f) => <SelectItem key={f} value={f} title={fieldTitle(f, labels)}>{displayName(f, labels)}</SelectItem>)}
              </SelectContent>
            </Select>
            <div className="flex shrink-0 items-center text-muted-foreground">
              {/* The name carries the CURRENT direction, so the arrow is readable by something
                  other than a sighted user — and so a test can see the flip in the DOM. */}
              <button type="button" className={ICON_BUTTON}
                aria-label={`sort ${i + 1} direction: ${clause.desc ? "descending" : "ascending"}`}
                onClick={() => replace(i, { ...clause, desc: !clause.desc })}>
                {clause.desc ? <ArrowDown className="h-3.5 w-3.5" /> : <ArrowUp className="h-3.5 w-3.5" />}
              </button>
              <button type="button" className={ICON_BUTTON} aria-label={`move sort ${i + 1} up`}
                disabled={i === 0} onClick={() => move(i, -1)}>
                <ChevronUp className="h-3.5 w-3.5" />
              </button>
              <button type="button" className={ICON_BUTTON} aria-label={`move sort ${i + 1} down`}
                disabled={i === value.length - 1} onClick={() => move(i, 1)}>
                <ChevronDown className="h-3.5 w-3.5" />
              </button>
              <button type="button" className={`${ICON_BUTTON} hover:text-destructive`}
                aria-label={`remove sort ${i + 1}`}
                onClick={() => onChange(value.filter((_, j) => j !== i))}>
                <Trash2 className="h-3.5 w-3.5" />
              </button>
            </div>
          </div>
        ))}
        {/* Gone once every field is sorted on — there is nothing left it could add. */}
        {unused.length > 0 && (
          <Select value="" onValueChange={(v) => { if (v) onChange([...value, { field: v, desc: false }]); }}>
            <SelectTrigger aria-label="add a sort" className="h-8 w-full text-sm">
              <SelectValue placeholder="Add a sort…" />
            </SelectTrigger>
            <SelectContent>
              {unused.map((f) => <SelectItem key={f} value={f} title={fieldTitle(f, labels)}>{displayName(f, labels)}</SelectItem>)}
            </SelectContent>
          </Select>
        )}
      </div>
    </div>
  );
}
