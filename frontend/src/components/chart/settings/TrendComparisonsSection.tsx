import { Plus, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { DecimalInput } from "./DecimalInput";
import {
  MAX_COMPARISONS, defaultComparison, previousPeriodName,
  type TrendComparison, type TrendComparisonType,
} from "@/components/chart/trend-comparison";

/**
 * Edits a trend's Comparisons — Metabase's list on the trend's DATA tab, not its Display tab.
 *
 * The shape was driven at :3033 rather than guessed: a "Comparisons" heading, one row per
 * comparison carrying a type dropdown and a remove ✕, and an "Add comparison" action underneath
 * that DISABLES at three (`MAX_COMPARISONS`, confirmed by reading `disabled` off the real button
 * with three rows present, not off the constant). Metabase's row also has a drag handle; see
 * `trend-comparison.ts` for why reordering is deliberately not here.
 *
 * ROWS ARE RENDERED IN STORED ORDER AND NEVER SORTED, the same rule `SegmentRangesSection` states
 * for the gauge and the scalar and for the same reason: sorting on every keystroke makes a row jump
 * out from under the cursor while one of its fields is half-typed.
 */
export function TrendComparisonsSection({ comparisons, unit, onChange }: {
  comparisons?: TrendComparison[];
  /**
   * The dimension's temporal unit, or undefined. GATES TWO OF THE FOUR TYPES: "previous month" and
   * "N months ago" have nothing to count without one, and Metabase omits both from its menu in
   * exactly that case (`getComparisonOptions` returns early when there is no `dateUnit`).
   *
   * A control offered for a chart that cannot honour it is the defect this programme keeps closing,
   * so they are ABSENT rather than present-and-inert.
   */
  unit?: string;
  onChange: (comparisons: TrendComparison[] | undefined) => void;
}) {
  const rows = comparisons ?? [];
  const periodName = previousPeriodName(unit);

  /** The type menu, per Metabase's, in its order. Only the two period types are conditional. */
  const options: { value: TrendComparisonType; label: string }[] = [
    ...(periodName ? [
      { value: "previousPeriod" as const, label: capitalise(periodName) },
      { value: "periodsAgo" as const, label: `${unitPlural(unit)} ago` },
    ] : []),
    { value: "previousValue", label: "Previous value" },
    { value: "staticNumber", label: "Custom value…" },
  ];

  /** Writes the list back, collapsing an empty one to `undefined`.
   *
   * `undefined` RATHER THAN `[]` IS LOAD-BEARING, and it is the same "store only what differs from
   * the default" rule the rest of these panels keep. An empty array would persist a key that says
   * nothing, and — because `resolveComparisons` treats both as "fall back to the default" — a
   * stored `[]` is a setting that can never be read back as anything. Removing the last row must
   * leave the chart in the state a brand-new one is in. */
  const write = (next: TrendComparison[]) => onChange(next.length > 0 ? next : undefined);

  const patch = (i: number, p: Partial<TrendComparison>) =>
    write(rows.map((r, j) => (j === i ? { ...r, ...p } : r)));

  const add = () => {
    // Metabase's "Add comparison" appends its DEFAULT, not an empty row — there is no such thing as
    // an untyped comparison, and a row that renders nothing until a type is picked would draw the
    // card's fallback comparison underneath it and read as a no-op.
    const d = defaultComparison(unit);
    write([...rows, { ...d, id: newId() }]);
  };

  return (
    <div className="space-y-2">
      <div className="space-y-1">
        <div className="text-xs font-medium">Comparisons</div>
        <p className="text-xs text-muted-foreground">
          What the number is measured against. With none set, a chart grouped by a time period
          compares against the previous one; an ungrouped one compares against the previous row.
        </p>
      </div>

      {rows.map((c, i) => (
        <div key={c.id} className="space-y-1.5 rounded-md border p-2">
          <div className="flex items-center gap-1.5">
            <Select
              value={c.type}
              onValueChange={(v) => patch(i, {
                type: v as TrendComparisonType,
                // The other type's fields do not travel: a "500" typed as a custom value is not a
                // count of periods, and carrying it over would silently change what the row means
                // while leaving the input looking untouched.
                value: v === "periodsAgo" ? 1 : undefined,
                label: undefined,
              })}
            >
              <SelectTrigger className="flex-1 text-sm" aria-label={`Comparison ${i + 1} type`}>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {options.map((o) => <SelectItem key={o.value} value={o.value}>{o.label}</SelectItem>)}
              </SelectContent>
            </Select>
            <Button variant="ghost" size="icon" className="w-8 shrink-0"
              aria-label={`Remove comparison ${i + 1}`}
              onClick={() => write(rows.filter((_, j) => j !== i))}>
              <Trash2 className="h-3.5 w-3.5" />
            </Button>
          </div>

          {/* The per-type second control. Two of the four types need one; the other two are fully
              described by their name, which is why this is conditional rather than a fixed row. */}
          {c.type === "periodsAgo" && (
            <div className="space-y-1">
              <Label htmlFor={`cmp-${c.id}-n`} className="text-xs text-muted-foreground">
                How many {unitPlural(unit)} back
              </Label>
              {/* `DecimalInput` is the house numeric field; a whole number of periods is what this
                  means, so a fractional entry is floored on the way out rather than rejected in the
                  box — rejecting mid-typing is what `DecimalInput` exists to avoid. `isComparisonValid`
                  is the backstop: a non-integer or a zero drops the row rather than drawing it. */}
              <DecimalInput id={`cmp-${c.id}-n`} className="text-sm" placeholder="1"
                value={c.value}
                onChange={(n) => patch(i, { value: n == null ? undefined : Math.max(1, Math.floor(n)) })} />
            </div>
          )}

          {c.type === "staticNumber" && (
            <div className="flex gap-1.5">
              <div className="flex-1 space-y-1">
                <Label htmlFor={`cmp-${c.id}-v`} className="text-xs text-muted-foreground">Value</Label>
                <DecimalInput id={`cmp-${c.id}-v`} className="text-sm" placeholder="0"
                  value={c.value} onChange={(n) => patch(i, { value: n })} />
              </div>
              <div className="flex-1 space-y-1">
                <Label htmlFor={`cmp-${c.id}-l`} className="text-xs text-muted-foreground">Label</Label>
                {/* REQUIRED, not decorative: this is the whole caption ("vs Goal: 500"), so a row
                    with no label has nothing to say and `isComparisonValid` drops it. */}
                <Input id={`cmp-${c.id}-l`} className="text-sm" placeholder="Goal"
                  value={c.label ?? ""}
                  onChange={(e) => patch(i, { label: e.target.value || undefined })} />
              </div>
            </div>
          )}
        </div>
      ))}

      <Button variant="outline" size="lg" className="w-full text-xs"
        disabled={rows.length >= MAX_COMPARISONS} onClick={add}>
        <Plus className="mr-1 h-3.5 w-3.5" /> Add comparison
      </Button>
    </div>
  );
}

/** Distinct per row and stable across edits, so React keys a row to its own inputs. `crypto.randomUUID`
 *  is not reachable on an insecure origin, and a chart editor served over plain HTTP in a dev
 *  environment is exactly that — so this does not use it. */
function newId(): string {
  return `c${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;
}

function capitalise(s: string): string {
  return s.charAt(0).toUpperCase() + s.slice(1);
}

/** "months", for the "N months ago" option and its input label. Metabase spells these out one by
 *  one (`formatPeriodsAgoOptionName`); every unit egret can bucket by pluralises with an "s", so
 *  this derives it from the name the caption already uses rather than keeping a fourth table of
 *  unit words that has to agree with the other three. */
function unitPlural(unit: string | undefined): string {
  const name = previousPeriodName(unit)?.replace(/^previous /, "");
  return name ? `${name}s` : "periods";
}
