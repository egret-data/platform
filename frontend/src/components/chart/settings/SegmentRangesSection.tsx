import { Plus, Trash2 } from "lucide-react";
import { SERIES_PALETTE } from "@/components/chart/echarts/palette";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";

/** The widest shape both charts' segments fit into. The gauge's bounds are always numbers; the
 * scalar's may be `null`, meaning unbounded on that side. */
export interface SegmentRow {
  min: number | null;
  max: number | null;
  color: string;
  label?: string;
}

/**
 * Edits a list of coloured ranges. Shared by the gauge (chart 04) and the scalar (chart 06),
 * which need the same row — swatch, two bounds, a name, a delete — and differ only in the four
 * behaviours below. Each difference exists because one chart has a constraint the other does not,
 * so they are props rather than a copy of this file:
 *
 * - `nullableBounds` — a scalar band may be open at one end ("≥ 1701 is High"); a gauge's arc
 *   needs both ends to have a length.
 * - `contiguous` — ECharts' `axisLine` colour stops are CUMULATIVE, so a gap between two gauge
 *   zones silently swallows one. A scalar band is a plain interval test with no such coupling.
 * - `minRows` — a gauge with no ranges has no arc; a scalar with no bands is the default state.
 * - `fallbackRows` — the gauge renders three derived zones when nothing is stored, so its editor
 *   shows those same three rather than an empty list. The scalar renders no colour at all.
 *
 * Rows are rendered in stored order and NEVER sorted here. Sorting on every keystroke would make
 * a row jump out from under the cursor while its bound is half-typed. For the gauge, ordering
 * belongs to the option builder (`sanitizeSegments`); for the scalar, authored order IS the
 * meaning, because the first matching band wins.
 */
export function SegmentRangesSection({
  rows, onChange, nullableBounds = false, contiguous = false, minRows = 0, emptyHint,
}: {
  rows: SegmentRow[];
  onChange: (rows: SegmentRow[]) => void;
  nullableBounds?: boolean;
  contiguous?: boolean;
  minRows?: number;
  /** Shown in place of the list when there are no rows. Only reachable when `minRows` is 0. */
  emptyHint?: string;
}) {
  function patch(i: number, field: keyof SegmentRow, v: SegmentRow[keyof SegmentRow]) {
    const next = rows.map((s) => ({ ...s }));
    next[i] = { ...next[i], [field]: v };
    // Contiguity: the gauge renderer treats a segment's `max` as a cumulative stop — the NEXT
    // segment starts painting exactly where this one's `max` lands, never at its own declared
    // `min`. So a gap or overlap between neighbours silently swallows one of them. Editing `max`
    // is the one edit that can create that, so it PUSHES the next range's `min` to match.
    // Editing `min` has no such effect: a range's floor only matters for display and validity,
    // never for where its neighbour begins.
    if (contiguous && field === "max" && i + 1 < next.length) {
      next[i + 1] = { ...next[i + 1], min: v as number };
    }
    onChange(next);
  }

  /** An emptied bound is `null` (unbounded) where that is allowed, and rejected where it is not.
   * Never `0`: `Number("")` is `0`, so coercing would turn "clear this bound" into "from zero" —
   * a different, silently wrong band. */
  function patchBound(i: number, field: "min" | "max", text: string) {
    if (text.trim() === "") {
      if (nullableBounds) patch(i, field, null);
      return;
    }
    const n = Number(text);
    if (!Number.isNaN(n)) patch(i, field, n);
  }

  function addRange() {
    const last = rows[rows.length - 1];
    const base = last && typeof last.max === "number" ? last.max : 0;
    onChange([...rows, { min: base, max: base + 10, color: SERIES_PALETTE[rows.length % SERIES_PALETTE.length] }]);
  }

  function removeRange(i: number) {
    if (rows.length <= minRows) return;
    onChange(rows.filter((_, k) => k !== i));
  }

  return (
    <div className="space-y-1.5 pt-1.5">
      <div className="text-xs text-muted-foreground">Ranges</div>
      {rows.length === 0 && emptyHint && (
        <div className="rounded border border-dashed px-2 py-1.5 text-xs text-muted-foreground">{emptyHint}</div>
      )}
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
            placeholder={nullableBounds ? "−∞" : undefined}
            value={s.min ?? ""}
            onChange={(e) => patchBound(i, "min", e.target.value)}
          />
          <span className="shrink-0 text-xs text-muted-foreground">–</span>
          <Input
            aria-label={`range ${i + 1} max`}
            type="number"
            className="h-7 w-16 shrink-0 text-sm"
            placeholder={nullableBounds ? "∞" : undefined}
            value={s.max ?? ""}
            onChange={(e) => patchBound(i, "max", e.target.value)}
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
            disabled={rows.length <= minRows}
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
