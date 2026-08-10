import { useState } from "react";

import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import {
  FIELD_FILTER_THRESHOLD, displayName, fieldTitle, matchesField, type FieldLabels,
} from "@/components/chart/config/field-names";

/**
 * A checkbox list — the shape both of these viz types configure with.
 *
 * WHY THIS IS STILL A CHECKBOX LIST AND NOT THE PIVOT'S DRAG ZONES. The two sit one screen apart on
 * the same tab and both answer "which fields does this chart use", which is a fair thing to call
 * inconsistent — but Metabase draws the same two shapes for the same two jobs, and for a reason
 * that survives being copied. Its notebook Data step is a checkbox list with a tri-state "Select
 * all" (`querying/notebook/components/FieldPicker/FieldPicker.tsx`); its pivot settings are drag
 * zones. A table's columns are a SET chosen out of many, where the common act is "not that one";
 * a pivot's rows/columns/values are three small ORDERED lists where the common act is "this one,
 * here". Ticking twenty boxes to build a set is right; dragging twenty chips to build one is not.
 *
 * So the gap that mattered was never the widget — it was that this one had none of the widget's
 * affordances. It had no count, no select-all, no filter, no bounded height (ten rows on a
 * ten-column dataset, fifty on a fifty-column one), and each row's only target was a 16×16
 * checkbox with a label beside it — measured live at 16×16 and 11×20 for the column named "id",
 * against the 24×24 minimum in WCAG 2.2 SC 2.5.8, in a row 255px wide of which 235px did nothing.
 * Metabase's own row is a full-width `<label>` at `min-height: 2.25rem` with a hover background,
 * which is what this is now.
 *
 * MAKING IT REORDERABLE IS NOT PART OF THIS, AND IS NOW SETTLED RATHER THAN DEFERRED. #441 left
 * this open on the grounds that a second order needed a rule; the rule is that THIS LIST NEVER
 * REORDERS, and the reason is stronger than "two places to store it". The Data tab's list is the
 * QUERY'S PROJECTION and the Display tab's is the order the table DRAWS in — `TableColumnsSection`,
 * backed by `tableState.columnOrder`, which #410 made the list the table actually renders from.
 * A drag added here would therefore be a control with NO VISIBLE EFFECT: whatever order it wrote,
 * the Display tab's `columnOrder` would still decide what appears on screen. A control that looks
 * like it works and does nothing is worse than an absent one.
 *
 * The two alternatives, both rejected. Making Display FOLLOW Data would discard an author's
 * explicit column order every time they ticked a box one tab over — the same silent-loss failure
 * `reconcileTableState` exists to prevent. Making Data follow Display cannot be built: this is a
 * checkbox list with no drag to follow it with. And a single shared order is what #410 ALREADY
 * SHIPPED, on the Display tab; re-siting it here would undo that PR's projection-versus-presentation
 * distinction, which is the one thing its body asks the next reader not to do.
 *
 * WHAT WAS ACTUALLY LEFT was that this list and the pivot's zones NAMED THE SAME COLUMN DIFFERENTLY
 * — see `field-names.ts`. That is the real residue of "it looks ugly next to the field zones", and
 * it is a naming fix, not a widget change.
 */
export function CheckList({ idPrefix, empty, options, selected, onToggle, onSetAll, labels }: {
  idPrefix: string;
  empty: string;
  options: string[];
  selected: string[];
  onToggle: (name: string) => void;
  /** Replaces the whole selection — the same `string[]` the toggles produce, never a new shape. */
  onSetAll: (next: string[]) => void;
  /** Machine name -> display name. Identity stays raw; this only changes what a row SAYS. */
  labels?: FieldLabels;
}) {
  const [query, setQuery] = useState("");
  const shown = options.filter((o) => matchesField(o, query, labels));
  const chosen = options.filter((o) => selected.includes(o));
  const allOn = options.length > 0 && chosen.length === options.length;

  if (options.length === 0) return <p className="text-xs italic text-muted-foreground">{empty}</p>;

  return (
    <div className="space-y-1 pt-1">
      {options.length > FIELD_FILTER_THRESHOLD && (
        <Input
          type="search"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={`Filter ${options.length} fields…`}
          aria-label="Filter fields"
          className="w-full text-sm"
        />
      )}
      {/* Select-all is INDETERMINATE at a partial selection rather than showing an empty box that
          would tick everything — that is Metabase's `indeterminate={!isAll && !isNone}` and it is
          the only way the control can state the list's state as well as offer an action. It also
          carries the count, which is the one fact a scrolled list hides. */}
      <label className="flex min-h-8 cursor-pointer items-center gap-2 rounded-md border-b px-1 hover:bg-muted/50">
        <Checkbox
          checked={allOn ? true : chosen.length === 0 ? false : "indeterminate"}
          aria-label="Select all fields"
          onCheckedChange={() => onSetAll(allOn ? [] : options)}
        />
        <span className="text-sm text-muted-foreground">
          {chosen.length} of {options.length} selected
        </span>
      </label>
      {/* Bounded, and the bound is why: without it the panel's own scrollbar was the only one, so
          on a wide table the Row limit below this list was several screens down and the author had
          to scroll past every column to reach it. `max-h-64` is eight rows — past the point where
          the filter above is the faster way in. */}
      <div className="max-h-64 space-y-0.5 overflow-y-auto [scrollbar-gutter:stable]">
        {shown.length === 0 && (
          <p className="px-1 py-2 text-xs text-muted-foreground">No field matches “{query.trim()}”.</p>
        )}
        {shown.map((name) => (
          // The whole row is the target: a `<label>` wrapping the checkbox, so the 235px that used
          // to do nothing now toggles it, and the row is at least 32px tall in both axes.
          <label
            key={name}
            htmlFor={`${idPrefix}-${name}`}
            className="flex min-h-8 cursor-pointer items-center gap-2 rounded-md px-1 hover:bg-muted/50"
          >
            <Checkbox id={`${idPrefix}-${name}`} checked={selected.includes(name)} onCheckedChange={() => onToggle(name)} />
            {/* The friendly name, with the machine name on hover — the chip in `FieldZone` has said
                it exactly this way since #438, and matching it is the point of the change. A second
                LINE showing the raw name (which the zone's own popover uses) was tried and dropped:
                it doubles the height of renamed rows only, so a list with three renames in it goes
                ragged, and this list is bounded and scrolled where that popover is neither. */}
            <span className="min-w-0 flex-1 truncate text-sm" title={fieldTitle(name, labels)}>
              {displayName(name, labels)}
            </span>
          </label>
        ))}
      </div>
    </div>
  );
}
