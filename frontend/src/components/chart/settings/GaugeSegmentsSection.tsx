import type { GaugeSegment } from "@/api/charts";
import { defaultGaugeSegments } from "@/components/chart/echarts/options/gauge";
import { SegmentRangesSection, type SegmentRow } from "./SegmentRangesSection";

/**
 * Edits a gauge's coloured ranges.
 *
 * The row editor itself is shared with the scalar (`SegmentRangesSection`); this wrapper supplies
 * the four behaviours that are the gauge's own — bounds are required numbers, neighbours stay
 * contiguous, the list never empties, and an unconfigured gauge shows the three zones the
 * renderer would actually draw rather than an empty list the user has nothing to click on.
 */
export function GaugeSegmentsSection({ segments, value, onChange }: {
  segments?: GaugeSegment[];
  /** The metric's current value, used only to size the three default zones when nothing is stored
   * yet — the same derivation `buildGaugeOption` uses, so the editor shows what would actually
   * render. */
  value: number;
  onChange: (segments: GaugeSegment[]) => void;
}) {
  const rows = segments && segments.length > 0 ? segments : defaultGaugeSegments(value);

  return (
    <SegmentRangesSection
      rows={rows}
      contiguous
      minRows={1}
      onChange={(next) => onChange(next.map(toGaugeSegment))}
    />
  );
}

/** The shared editor's bounds are nullable because the scalar's are. A gauge never produces a
 * null — `nullableBounds` is off, so an emptied input is rejected rather than cleared — but the
 * type has to be narrowed back on the way out. `0` is the only sane floor for a bound that cannot
 * exist. */
function toGaugeSegment(s: SegmentRow): GaugeSegment {
  return { ...s, min: s.min ?? 0, max: s.max ?? 0 };
}
