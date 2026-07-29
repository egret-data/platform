import { Plus, Trash2 } from "lucide-react";
import type { GaugeSegment } from "@/api/charts";
import { defaultGaugeSegments } from "@/components/chart/echarts/options/gauge";
import { SERIES_PALETTE } from "@/components/chart/echarts/palette";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";

/**
 * Edits a gauge's coloured ranges. With nothing stored, the renderer draws three default
 * zones derived from the current value (see `defaultGaugeSegments`) rather than storing
 * them — so this shows exactly those three rows, reusing that same function, rather than
 * showing an empty list the user has nothing to click on.
 *
 * Rows are rendered in stored order and never sorted here: sorting on every keystroke
 * would make a row jump out from under the cursor while its bound is mid-edit. Sorting by
 * `max` belongs solely to the option builder (`sanitizeSegments` in echarts/options/gauge).
 */
export function GaugeSegmentsSection({ segments, value, onChange }: {
  segments?: GaugeSegment[];
  /** The metric's current value, used only to size the three default zones when nothing
   * is stored yet — the same derivation `buildGaugeOption` uses, so the editor shows what
   * would actually render. */
  value: number;
  onChange: (segments: GaugeSegment[]) => void;
}) {
  const rows = segments && segments.length > 0 ? segments : defaultGaugeSegments(value);

  function patch(i: number, field: keyof GaugeSegment, v: GaugeSegment[keyof GaugeSegment]) {
    const next = rows.map((s) => ({ ...s }));
    next[i] = { ...next[i], [field]: v };
    // Contiguity rule: the renderer treats a segment's `max` as a cumulative stop — the
    // NEXT segment starts painting exactly where this one's `max` lands, never at its own
    // declared `min`. So a gap or overlap between neighbours silently swallows one of them
    // (see the "resolves an overlap by upper bound" case in gauge.test.ts). Editing `max`
    // is the one edit that can create that gap/overlap, so it PUSHES the next range's
    // `min` to match — the two ends stay locked together rather than drifting apart and
    // needing a separate "fix the gap" step. Editing `min` has no such effect: a range's
    // floor only matters for display/validity, never for where its neighbour begins.
    if (field === "max" && i + 1 < next.length) {
      next[i + 1] = { ...next[i + 1], min: v as number };
    }
    onChange(next);
  }

  function addRange() {
    const last = rows[rows.length - 1];
    const base = last ? last.max : 0;
    onChange([...rows, { min: base, max: base + 10, color: SERIES_PALETTE[rows.length % SERIES_PALETTE.length] }]);
  }

  function removeRange(i: number) {
    if (rows.length <= 1) return; // a gauge with no ranges has no arc
    onChange(rows.filter((_, k) => k !== i));
  }

  return (
    <div className="space-y-1.5 pt-1.5">
      <div className="text-xs text-muted-foreground">Ranges</div>
      {rows.map((s, i) => (
        <div key={i} className="flex items-center gap-1">
          <input
            type="color"
            aria-label={`range ${i + 1} color`}
            className="h-7 w-7 shrink-0 cursor-pointer rounded border bg-transparent p-0.5"
            value={s.color}
            onChange={(e) => patch(i, "color", e.target.value)}
          />
          <Input
            aria-label={`range ${i + 1} min`}
            type="number"
            className="h-7 w-16 shrink-0 text-sm"
            value={s.min}
            onChange={(e) => {
              const n = Number(e.target.value);
              if (!Number.isNaN(n)) patch(i, "min", n);
            }}
          />
          <span className="shrink-0 text-xs text-muted-foreground">–</span>
          <Input
            aria-label={`range ${i + 1} max`}
            type="number"
            className="h-7 w-16 shrink-0 text-sm"
            value={s.max}
            onChange={(e) => {
              const n = Number(e.target.value);
              if (!Number.isNaN(n)) patch(i, "max", n);
            }}
          />
          <Input
            aria-label={`range ${i + 1} name`}
            className="h-7 min-w-0 flex-1 text-sm"
            placeholder="Name"
            value={s.label ?? ""}
            onChange={(e) => patch(i, "label", e.target.value.trim() ? e.target.value : undefined)}
          />
          <button
            type="button"
            aria-label={`remove range ${i + 1}`}
            disabled={rows.length <= 1}
            className="shrink-0 rounded p-0.5 text-muted-foreground hover:bg-accent hover:text-destructive disabled:opacity-30"
            onClick={() => removeRange(i)}
          >
            <Trash2 className="h-3.5 w-3.5" />
          </button>
        </div>
      ))}
      <Button type="button" variant="outline" size="sm" className="w-full" onClick={addRange}>
        <Plus className="mr-1 h-3.5 w-3.5" /> Add range
      </Button>
    </div>
  );
}
