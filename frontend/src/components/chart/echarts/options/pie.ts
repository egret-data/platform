import type { EChartsCoreOption } from "echarts/core";
import type { ColumnFormat } from "../../format-types";
import { formatValue } from "../../format";
import {
  ariaDescription,
  baseOption,
  TOOLTIP_HEADER_FONT_SIZE,
  TOOLTIP_WEIGHT,
  tooltipInk,
  tooltipSurface,
} from "../base";
import { OTHER_SLICE_COLOR, COLOR_FALLBACK, resolveColor, resolveFontFamily, seriesColor, ringVariantColor, fadeColor, getTextColorForBackground, getTextHaloForBackground } from "../palette";
import {
  calcAvailableDonutSliceLabelLength,
  calcInnerOuterRadiusesForRing,
  fitSliceLabel,
  measureText as defaultMeasureText,
  truncateLabelText,
  type MeasureText,
} from "../sliceLabel";
import { isExtractUnit, temporalLabel } from "@/components/chart/temporal-units";

export interface ResultColumn { name: string; type: string }

/** Metabase's own `pie.percent_visibility` domain, value for value: nowhere, beside each name
 * in the legend, on the slice itself, or both. `"legend"` was the one value egret was missing
 * — and it is the one Metabase's own sample sunburst (question 7) is configured with. */
export type PercentVisibility = "off" | "legend" | "inside" | "both";

const PERCENT_VISIBILITIES: readonly string[] = ["off", "legend", "inside", "both"];

/**
 * Where a pie's percentages go, given what (if anything) is stored.
 *
 * Unset defaults to "legend" ONCE THERE IS MORE THAN ONE RING, and stays "off" for a single
 * ring. On a sunburst the legend is where the reader compares — it lists ring 0 only (the
 * level whose slices are shares of the whole chart, and the only level whose colours are
 * unambiguous), and it has room for a number that the arcs do not: ring-0 labels are already
 * competing for arc space with the rings outside them, and the fit pass truncates or drops
 * them. Percentages on the outer rings would be worse than crowded — they are shares of a
 * PARENT slice, so stacking them next to ring-0's shares-of-the-whole invites comparing two
 * different denominators. A single-ring pie is left alone: its labels have room, nothing
 * about it was reported as unreadable, and every saved one would otherwise change appearance.
 *
 * A stored value that isn't one of the four degrades to "off" rather than being trusted:
 * settings arrive from a JSONB column that is never whitelisted server-side, so this can be
 * hand-written or mistyped, and drawing nothing is the safe reading of "I don't know what
 * this means" — the same treatment `resolveColor` gives an uninterpretable colour.
 *
 * Shared with the settings panel, which has to show the same answer the chart will draw or
 * it becomes a control that lies (the bug this program has already fixed for the legend, slice
 * labels and, just now, the centre total).
 */
export function resolvePercentVisibility(stored: string | undefined, ringCount: number): PercentVisibility {
  if (stored === undefined) return ringCount > 1 ? "legend" : "off";
  return PERCENT_VISIBILITIES.includes(stored) ? (stored as PercentVisibility) : "off";
}

export interface PieBuildConfig {
  /** Up to three, drawn as nested rings from the centre out: dimensions[0] is the
   * innermost ring, each subsequent entry nests inside the previous one's slices. */
  dimensions?: string[];
  metric?: string;
  donut?: boolean;
  showLegend?: boolean;
  showLabels?: boolean;
  /** Slices below this percent of the total are grouped into one "Other" slice. Default 2.5; 0 disables. */
  sliceThreshold?: number;
  /** Sort slices by value descending. Default true. */
  sortRows?: boolean;
  /** Show the grand total in the hole. Defaults ON once there is more than one ring (where the
   * hole is large and an empty one reads as unfinished), off for a one-ring donut; ignored by a
   * one-ring pie with no hole at all. */
  showTotal?: boolean;
  /** Where a slice's percent appears — see `resolvePercentVisibility` for the values and the
   * default. Unset defaults to "legend" for a multi-ring pie, "off" for a single ring. */
  percentVisibility?: PercentVisibility;
  /** Per-slice colour override, keyed by the slice (dimension) value. */
  sliceColors?: Record<string, string>;
  /** Curation of the FIRST dimension's slices only (ring 0) — matches Metabase's `pie.rows`.
   * Extending this to inner rings would need a compound key and isn't worth it for rings that
   * exist to be scanned, not curated.
   *
   * All three key on the slice's RAW dimension value, never its renamed display text — so a
   * rename never breaks a colour override, a hide, or a manual position.
   *
   * `hiddenSlices` rows are dropped before the ring's total/threshold is computed at all, so
   * the remaining slices' shares re-base as if the hidden category never existed; an outer
   * ring's rows nested under a hidden parent are dropped with it (the same orphan-drop already
   * used for a parent lost to the sign/zero rule), not folded into anything and not kept alone.
   *
   * `sliceOrder` is a display-order override: unlisted values keep their natural (post-sort)
   * order after the named ones — the same rule `CartesianConfig.seriesOrder` uses. */
  sliceNames?: Record<string, string>;
  hiddenSlices?: string[];
  sliceOrder?: string[];
  /** Decimal places shown on slice percentages (label and tooltip). Default 2. */
  decimals?: number;
  columnSettings?: Record<string, ColumnFormat>;
  metricTypes?: Record<string, string>;
  /** Temporal extraction unit per dimension, keyed by column name (e.g. `{ region: "day_of_week" }`) —
   * the same shape as `params.spec.temporal_units`, so ChartRenderer forwards it unmodified and each
   * ring looks up its own dimension's unit. */
  dimensionUnits?: Record<string, string>;
  /** RENDER-TIME, never stored: which layout the legend is being drawn in. The legend itself is
   * React markup outside the canvas now (see `pieLegendPlacement`, which is the ONE place that
   * decides this — the pie never re-derives it), so all this does here is say which fraction of
   * its own box the circle should fill. Unset means the bottom one, i.e. today's geometry. */
  legendPlacement?: PieLegendPlacement;
  /** RENDER-TIME, never stored: ring-0 slices the reader has clicked off in the legend, by their
   * DISPLAYED name (the legend only ever knows that). Distinct from `hiddenSlices`, which is a
   * saved curation applied before the ring's total and Other threshold are computed at all; this
   * is the ephemeral legend toggle, applied last — see `buildPieOption`. */
  toggledOffSlices?: string[];
}

const OTHER_NAME = "Other";
const DEFAULT_THRESHOLD = 2.5; // percent — Metabase SLICE_THRESHOLD (0.025)
/**
 * The smallest share of the chart a RING-0 slice is drawn at, however little it is worth —
 * Metabase's `OTHER_SLICE_MIN_PERCENTAGE` (0.005).
 *
 * A slice worth 0.1% of the total is about a pixel of arc on a 550px pie, most of it painted over
 * by the border: there is nothing to hover, nothing to click, and nothing to see. Drawing it at
 * half a percent gives it a target without giving it a claim — every number the chart REPORTS
 * about it (label, tooltip, legend percent, centre readout) still comes from its true value, which
 * is why the datum carries both (see `PieSliceNode.rawValue`).
 *
 * RING 0 ONLY, which is Metabase's own rule and its own reason: "if we do this for the outer
 * rings, it can lead to overlapping slices". An outer slice sits inside its parent's arc, and
 * there is no spare angle in there to take the extra from — every ring-0 slice, by contrast, is
 * competing for the whole turn, which the sum below re-normalises. egret's outer rings are more
 * crowded than Metabase's (a per-parent Other fold keeps more of them), so the rule binds harder
 * here, not less.
 */
const MIN_DRAWN_SLICE_FRACTION = 0.005;
// Sentinel identifying "this ring's own Other bucket" as a reorder group, distinct from any real
// column value. Collides only if a real dimension value were literally this string (same accepted
// risk as OTHER_NAME itself already colliding with a real "Other" category value).
const OTHER_KEY = "\u0000__other__\u0000";
// Joins a dimension-value tuple into one aggregation key. A control character keeps it well clear
// of ordinary column values (still not bulletproof against a value containing it, same class of
// accepted risk as OTHER_KEY/OTHER_NAME).
const KEY_SEP = "␟";

/** One (aggregated) slice going into a ring, still carrying the identity fields ring-nesting needs:
 * `key` — this ring's own aggregation key (the dimension-value path up to and including this ring);
 * `parentKey` — the same path one ring in, i.e. the key this slice's row(s) aggregate to in the
 * next ring inward. Absent on the synthetic "Other" slice, which has no single origin. */
interface Slice { name: string; value: number; key?: string; parentKey?: string; itemStyle?: { color: string } }

/** One node of the `sunburst` series' data tree — a drawn slice, plus the slices nested inside it.
 * Named so `toNode` can be recursive without TypeScript having to infer its own return type. */
interface PieSliceNode {
  name: string;
  /** WHAT ECHARTS DRAWS THIS SLICE AS — not necessarily what it is worth. `sunburstLayout` turns a
   * node's `value` straight into an angle, so this is the only field that can express "draw it a
   * little bigger than it is", which is what the tiny-slice floor needs (see
   * MIN_DRAWN_SLICE_FRACTION). Equal to `rawValue` for every slice the floor doesn't touch. */
  value: number;
  /** WHAT THE SLICE IS ACTUALLY WORTH — the number anything that REPORTS the slice must use. Split
   * from `value` by the tiny-slice floor: a sub-0.5% slice is drawn bigger so it can be pointed at,
   * and would otherwise start reading out the inflated figure the moment you did point at it. */
  rawValue: number;
  /** This slice's share of its own parent (of the whole chart, at ring 0), already rounded to the
   * chart's `decimals`. A `sunburst` series supplies no `percent` of its own — see `toNode`. */
  slicePercent: number;
  /** This slice is a synthetic "Other" bucket rather than a real dimension value. Metabase's own
   * `isOther`: it changes what the tooltip lists (its CHILDREN, not its siblings — pointing at
   * "Other" is asking what is in it) and it is never the focused row. */
  isOther: boolean;
  itemStyle: { color: string; borderWidth: number };
  label: {
    color: string;
    formatter: () => string;
    rotate: 0 | "radial";
    verticalAlign: "middle";
  };
  emphasis: { itemStyle: { color: string; borderColor: string } };
  blur: { itemStyle: { color: string; opacity: number }; label: { opacity: number } };
  children?: PieSliceNode[];
}

/** A ring's raw (pre-robustness-rules) aggregate: rows grouped by the dimension-value tuple up to
 * and including this ring's own dimension, values summed. Pre-existing behaviour, unchanged by
 * nesting: grouping is by DISPLAYED value, so `null`, `undefined` and `""` all render as the same
 * `""`-named slice and merge together. */
function aggregateRing(rows: unknown[][], dimIdxs: (number | undefined)[], units: (string | undefined)[], mi: number): Slice[] {
  const groups = new Map<string, Slice>();
  for (const r of rows) {
    const parts = dimIdxs.map((di, i) => {
      const raw = di != null ? r[di] : undefined;
      return isExtractUnit(units[i]) ? temporalLabel(units[i], raw) : String(raw ?? "");
    });
    const key = parts.join(KEY_SEP);
    const value = Number(r[mi]);
    const existing = groups.get(key);
    if (existing) existing.value += value;
    else groups.set(key, {
      key,
      parentKey: parts.length > 1 ? parts.slice(0, -1).join(KEY_SEP) : undefined,
      name: parts[parts.length - 1],
      value,
    });
  }
  return [...groups.values()];
}

// Identifies a specific parent's own "Other" bucket, distinct from every other parent's, so two
// different parents' folded buckets never collide as the same map key downstream (groupOf, and
// the colour-anchor pass's per-ring anchor map — see buildPieOption). Ring 0's items all carry
// `parentKey: undefined` (one implicit parent, the whole chart), so this reduces to the plain
// OTHER_KEY sentinel there — unchanged from before this fix.
function otherKeyFor(parentKey: string | undefined): string {
  return parentKey != null ? `${OTHER_KEY}${KEY_SEP}${parentKey}` : OTHER_KEY;
}

// Clean-room of Metabase's pie data prep (pie/model/index.ts): drop zeros, handle
// negatives (all-negative → abs, mixed → drop negatives), sort by value desc, then
// fold sub-threshold slices into a muted "Other" — Runs once per ring — the outer
// rings feed it their own aggregated (name, value) pairs instead of raw rows, but every
// rule above applies identically and independently at every level.
//
// The Other fold itself is judged PER PARENT, not across the whole ring (Metabase's own model —
// pie/model/index.ts's aggregateChildrenSlices — folds within `node.children`, i.e. per parent,
// for exactly this reason): ring 0's items have no parentKey (one implicit parent, the whole
// chart), so grouping by parentKey there degenerates to the old ring-wide behaviour — a
// single-ring pie is untouched. A ring below 0 has one real parentKey per item (aggregateRing
// always gives these a defined parentKey), so each parent's slices are judged against THAT
// PARENT's own total and, if it has 2+ sub-threshold slices, get THEIR OWN "Other" bucket —
// instead of a chart-wide Other, scattered across every parent, that belongs to none of them
// (the bug this fixes: with 36 outer slices each ~2.8% of the RING but a healthy share of their
// own small parent group, the old ring-wide judgment folded about half of them together with no
// parent to sit under).
//
// Also returns `groupOf`: for every item that survived the finite/sign filter, the key of
// the ring's OWN slice it ended up as — either its own `key` (kept) or that parent's own
// other-key (folded, see otherKeyFor). An item absent from `groupOf` was dropped outright
// (non-finite, zero, or mixed-sign negative) — the next ring inward uses that to drop its now-
// parentless children too.
function buildSlices(items: Slice[], cfg: PieBuildConfig): { slices: Slice[]; groupOf: Map<string, string> } {
  let slices: Slice[] = items.filter((s) => Number.isFinite(s.value));

  const allNegative = slices.length > 0 && slices.every((s) => s.value < 0);
  slices = allNegative
    ? slices.map((s) => ({ ...s, value: Math.abs(s.value) }))
    : slices.filter((s) => s.value > 0); // drops zeros and (mixed-sign) negatives

  if (cfg.sortRows !== false) {
    slices = [...slices].sort((a, b) => b.value - a.value);
  }

  const groupOf = new Map<string, string>();
  for (const s of slices) if (s.key) groupOf.set(s.key, s.key);

  const threshold = (cfg.sliceThreshold ?? DEFAULT_THRESHOLD) / 100;
  if (threshold > 0 && slices.length > 0) {
    const parentGroups = new Map<string | undefined, Slice[]>();
    for (const s of slices) {
      const group = parentGroups.get(s.parentKey);
      if (group) group.push(s);
      else parentGroups.set(s.parentKey, [s]);
    }

    const result: Slice[] = [];
    for (const group of parentGroups.values()) {
      const groupTotal = group.reduce((t, s) => t + s.value, 0);
      const small = group.filter((s) => s.value / groupTotal < threshold);
      // Only worth an "Other" bucket when it merges 2+ slices WITHIN THIS PARENT (matches
      // Metabase's per-parent merge-2+ rule).
      if (small.length > 1) {
        const big = group.filter((s) => s.value / groupTotal >= threshold);
        const otherTotal = small.reduce((t, s) => t + s.value, 0);
        const otherColor = resolveColor(OTHER_SLICE_COLOR, COLOR_FALLBACK.axisLabel);
        const parentKey = group[0].parentKey;
        const otherKey = otherKeyFor(parentKey);
        for (const s of small) if (s.key) groupOf.set(s.key, otherKey);
        result.push(
          ...big,
          { name: OTHER_NAME, value: otherTotal, key: otherKey, parentKey, itemStyle: { color: otherColor } },
        );
      } else {
        result.push(...group);
      }
    }
    slices = result;
  }

  return { slices, groupOf };
}

/** Groups a ring's slices under the key of the PARENT-ring slice each one belongs to — the
 * structure the sunburst's `children` needs.
 *
 * This replaced `reorderByParent`, which existed only because N independent `pie` series draw in
 * flat array order: a child ring's data had to be pre-sorted into its parents' arcs by hand,
 * because nothing in the option said which parent a slice belonged to. A tree says it outright,
 * so the ordering problem disappears and only the grouping is left. (The resulting draw order is
 * the same: ECharts lays a node's children out consecutively inside that node's arc, in array
 * order, which is what the hand-rolled version was reproducing.)
 *
 * Grouped by KEY, not by display name: two different parents can have children with the same
 * name (e.g. an "Accessories" category under two different regions), and grouping by name would
 * wrongly merge them.
 *
 * A slice whose parent got folded into the parent ring's own "Other" (parentGroupOf maps its
 * parentKey to that parent's own other-key, see otherKeyFor) lands under that Other bucket
 * instead of vanishing.
 * A slice with no resolvable group is dropped. Defensive only: every level > 0 item, including
 * each parent's own "Other" bucket, carries a defined parentKey (buildSlices folds within
 * parents, not across the ring) and orphans are filtered out earlier in buildPieRings, so this
 * path isn't expected to trigger. A tree has nowhere to put a parentless node — the flat version's
 * "append at the end of the ring" was itself a wrong-looking placement, not a save.
 */
function groupByParent(slices: Slice[], parentGroupOf: Map<string, string>): Map<string, Slice[]> {
  const groups = new Map<string, Slice[]>();
  for (const s of slices) {
    const groupKey = s.parentKey != null ? parentGroupOf.get(s.parentKey) : undefined;
    if (groupKey == null) continue;
    const arr = groups.get(groupKey);
    if (arr) arr.push(s); else groups.set(groupKey, [s]);
  }
  return groups;
}

/**
 * Every ring's slices, grouped under the finalized parent-ring slice each one belongs to — one map
 * per parent level, i.e. the sunburst's `children` structure as lookup tables.
 *
 * Resolved through the parent ring's OWN fold state (`groupOfByLevel[level - 1]`), not just the raw
 * parentKey — so a slice whose parent folded into that ring's "Other" bucket nests under the Other
 * wedge rather than disappearing. That is also why a ring's own "Other" reports a share of the same
 * parent as its folded-in siblings: it carries their `parentKey` and takes this same lookup.
 */
function pieChildrenByParent(rings: Slice[][], groupOfByLevel: Map<string, string>[]): Map<string, Slice[]>[] {
  return rings.slice(1).map((ring, i) => groupByParent(ring, groupOfByLevel[i] ?? new Map()));
}

/** How many nodes this slice contributes to the sunburst's flattened datum list: itself, plus
 * everything nested inside it. See `PieLegendEntry.sliceDataIndex` for what that is used for. */
function subtreeSize(s: Slice, level: number, childrenByParentKey: Map<string, Slice[]>[]): number {
  const children = childrenByParentKey[level]?.get(s.key ?? OTHER_KEY) ?? [];
  return 1 + children.reduce((t, c) => t + subtreeSize(c, level + 1, childrenByParentKey), 0);
}

// ---------------------------------------------------------------------------------------------
// Where the legend goes, and what that costs the rings.
//
// Measured side by side against a real Metabase (sample question 7, "Product breakdown" — a
// genuine 3-level sunburst) at 1440x920: Metabase drew 69px-thick rings, egret 58px, and
// Metabase's legend was a left COLUMN where egret's was a bottom ROW. egret's ring RATIOS already
// matched (see HOLE_RATIO); what differed was how much of the host the circle was allowed to use.
//
// The legend is React markup BESIDE the canvas (see PieLegend.tsx), not an ECharts component
// inside it — ECharts' `legend` does not work with a `sunburst` series, which is what this pie
// is. So the 190px column below is real layout: the chart's own box already excludes
// it, and this file's job is only to say what fraction of THAT box the circle fills. Each layout
// states the diameter it draws, and the bigger one wins:
//
//   bottom row : 0.70 * min(w, h)          — exactly what this file already drew, unchanged
//   side column: 0.85 * min(w - 190, h)    — the box a 190px legend column leaves it
//
// The row's 0.70 is left alone deliberately: it is what every saved pie renders as today, and
// its 15%-of-the-shorter-side margin above and below is already where the row sits. The column's
// 0.85 is the same idea applied to the box the column leaves — a pie's labels are drawn INSIDE
// its slices (see `position: "inside"` below), so nothing needs an annulus outside the circle
// and the remaining 7.5% each side is breathing room. It is also what the measurement asks for:
// on the measured 830x661 host, 0.85 * min(640, 661) = 544px of diameter, i.e. 68px rings
// against Metabase's 69.
//
// Worked both ways, since the point is that neither layout is universally right:
//   830x661 (measured): row 462.7px, column 544px    -> column
//   360x720 (narrow)  : row 252px,   column 144.5px  -> row, by a factor of ~1.7
//   735x735 (square)  : row 514.5px, column 463.3px  -> row
// The crossover for a 661-tall host is w > 734px (0.85 * (w - 190) > 0.7 * 661); a host narrower
// than it is tall never reaches it at all, since 0.85 * (w - 190) > 0.7 * w needs w > 1076px.
// There is no hard-coded breakpoint anywhere — only these two diameters, compared.
/** Beside the chart in a column, or under it in a row. */
export type PieLegendPlacement = "side" | "bottom";

/** How wide the legend column is when it goes beside the chart — read by the React legend, which
 * is the thing that actually occupies it, and by the arithmetic below that decides whether paying
 * for it is worth it. One number, one place. */
export const PIE_LEGEND_COLUMN_WIDTH_PX = 190;
const BOTTOM_LEGEND_DIAMETER_FRACTION = 0.7;
const SIDE_LEGEND_DIAMETER_FRACTION = 0.85;

/**
 * Where a pie's legend goes, given the size of the WHOLE chart host (legend included) — the one
 * place this is decided. React reads it to lay the two out, and hands the answer back to
 * `buildPieOption` as `legendPlacement` rather than letting the pie re-derive it from a size that
 * by then means something different (the chart's own box, already legend-free).
 *
 * "bottom" is the answer for an unknown size too: no measurement, no aspect to judge, so nothing
 * moves. A not-yet-laid-out 0x0 host needs no separate guard — at zero the column's own diameter
 * is zero or negative and the comparison below rejects it, which is the same answer.
 */
export function pieLegendPlacement(size?: { width: number; height: number }): PieLegendPlacement {
  if (!size) return "bottom";
  const bottomDiameter = BOTTOM_LEGEND_DIAMETER_FRACTION * Math.min(size.width, size.height);
  const sideDiameter =
    SIDE_LEGEND_DIAMETER_FRACTION * Math.min(size.width - PIE_LEGEND_COLUMN_WIDTH_PX, size.height);
  return sideDiameter > bottomDiameter ? "side" : "bottom";
}

/** One row of the React legend: the swatch's colour, the name, and (when percentages are on) the
 * slice's share of the whole chart, already formatted to the chart's own precision. */
export interface PieLegendEntry {
  name: string;
  color: string;
  percent?: string;
  /** The slice's raw metric value. Not shown in the legend — it is what the chart's centre reads
   * out when the pointer is on this entry, so that a legend hover says the same thing a slice
   * hover does (a slice hover gets the value off the ECharts event for free; a legend hover has
   * no event to get it from). */
  value: number;
  /**
   * Where this slice sits in the sunburst's own flattened datum list — what `dispatchAction`
   * needs to highlight it (see EChartsRenderer's `highlightDataIndex`).
   *
   * BY INDEX, NOT BY NAME, which is what Metabase dispatches. ECharts resolves a `name` payload
   * with `SeriesData.indexOfName`, a linear scan of the WHOLE tree in depth-first pre-order — and
   * a sunburst's names are only unique within a parent, so the first match can easily be some
   * other ring's node. "Other" makes that a likely case rather than a theoretical one: every ring
   * folds its own sub-threshold slices into a bucket of that exact name, so highlighting ring 0's
   * "Other" would light up whichever nested "Other" happened to come first.
   *
   * `undefined` for an entry the reader has toggled off: its slice is not in the tree at all, so
   * there is nothing to highlight (and the indexes of everything after it shift down, which is
   * why this is computed against the same `toggledOffSlices` the option is built with).
   */
  sliceDataIndex?: number;
}

/** Percent text, at the given precision — padded to exactly `decimals` places, never stripped.
 *
 * `computeMaxPercentDecimals` takes the MAXIMUM decimal count across a chart's values specifically
 * so the reader compares like with like down a column (see its own header comment) — a formatter
 * that then strips a value's own trailing zeros defeats that the moment one value happens to round
 * to a shorter string than its neighbours (28.97 -> "29%" beside "36.4%" and "34.6%", instead of the
 * "29.0%" the shared precision promised). `toFixed` alone is exactly the padded form: at
 * `decimals: 0` it already returns a plain integer string with no separator, so there is no special
 * case needed for "29%" not "29.%". */
function pctFormatter(decimals: number) {
  return (p: number) => `${p.toFixed(decimals)}%`;
}

/**
 * How many decimals a set of shares needs — Metabase's `computeMaxDecimalsForValues`
 * (visualizations/lib/utils.ts), ported rather than approximated.
 *
 * WHAT IT ACTUALLY DOES, because it is easy to assume something cleverer: it formats each fraction
 * as a percent under an `Intl.NumberFormat` SIGNIFICANT-DIGIT budget, counts the fraction digits
 * that survive that rounding, and returns the largest count over the set. Nothing compares one
 * value to another, and nothing looks for the shortest string that keeps them distinct — two
 * slices of identical size get the same answer as one. The budget is the whole mechanism: at 3
 * significant digits 23.88% rounds to `23.9%` (1 decimal) and 5.123% to `5.12%` (2), so a chart
 * containing both prints every share to 2 — which is how Metabase's legend comes to read `23.88%`
 * when no single value asked for four significant digits.
 *
 * Taking the MAX is what makes one number's precision everyone's: a chart with one 5.12% slice
 * lengthens all its percentages, so the reader compares like with like down the column.
 *
 * `fractions` are shares in the 0..1 sense (`style: "percent"` multiplies by 100), matching the
 * `normalizedPercentage` Metabase feeds it.
 *
 * Metabase wraps the whole thing in a try/catch and returns `undefined` when `Intl.NumberFormat`
 * throws, leaving its formatter to fall back on its own default. egret's caller needs a number, so
 * the fallback is stated here — 2, which is exactly what this file hard-coded before.
 */
export function computeMaxPercentDecimals(fractions: number[], maximumSignificantDigits: number): number {
  try {
    const formatter = Intl.NumberFormat("en", { style: "percent", maximumSignificantDigits });
    let maxDecimals = 0;
    for (const fraction of fractions) {
      const part = formatter.formatToParts(fraction).find((p) => p.type === "fraction");
      const decimals = part ? part.value.length : 0;
      if (decimals > maxDecimals) maxDecimals = decimals;
    }
    return maxDecimals;
  } catch {
    return 2;
  }
}

/** Metabase's two budgets (pie/format.ts): a legend has room for a longer number than a label
 * squeezed onto an arc does, so the same share prints as `23.88%` beside the chart and `23.9%` on
 * it. Deliberately NOT one shared number — the whole point is that the two places differ. */
const LEGEND_PERCENT_SIGNIFICANT_DIGITS = 3;
const SLICE_PERCENT_SIGNIFICANT_DIGITS = 2;

/** The chart's metric formatter — the same one for the slices, the hole and the tooltip, so a
 * number never reads differently depending on where you are looking at it. */
function metricFormatter(cfg: PieBuildConfig): (v: unknown) => string {
  return (v: unknown) =>
    cfg.metric ? formatValue(v, cfg.metricTypes?.[cfg.metric] ?? "double", cfg.columnSettings?.[cfg.metric]) : String(v);
}

// ---------------------------------------------------------------------------------------------
// Geometry, in one place.
//
// Pulled out of `buildPieOption` because the hole's readout (see `buildPieHoleGraphic` below)
// needs the SAME inner radius the rings are drawn against in order to decide what text fits in
// it — and re-deriving that beside the option is exactly how a chart and the thing drawn on top
// of it drift apart. One function, one answer, both callers.
interface PieGeometry {
  /** The hole, as a fraction of the outer radius. 0 means a true full pie (no hole at all). */
  holeRatio: number;
  diameterPx: number;
  outerRadiusPx: number;
  innerRadiusPx: number;
  /** A real container size was supplied; pixel radii are meaningful and text can be fitted. */
  hasMeasuredSize: boolean;
  radius: [number, number] | [string, string];
  center: [string, string];
}

/** How big the circle is allowed to get, in absolute pixels. Metabase's pie is 550px in a 735px
 * host — `DIMENSIONS.maxSideLength`, an absolute cap rather than a ratio, because past a point a
 * bigger circle stops adding information and just pushes the labels apart. */
const PIE_MAX_DIAMETER_PX = 550;
/** Metabase's `DIMENSIONS.padding.side` (12px a side), as a floor on the margin: the fraction
 * above already leaves one at any normal size (it only binds below ~160px of host), but it is a
 * fraction, so on a tiny host it would leave a proportionally tiny one. */
const PIE_SIDE_PADDING_PX = 12;

// The hole, as a ratio of the OUTER radius — Metabase's own ratios (constants.ts): two rings
// 2/5, three rings 1/4. Stated as a ratio rather than an absolute so a saved donut keeps its
// SHAPE when the outer edge moves (side vs bottom layout, and the diameter cap below).
const HOLE_RATIO: Record<number, number> = { 2: 2 / 5, 3: 1 / 4 };

function pieGeometry(
  cfg: PieBuildConfig,
  ringCount: number,
  containerSize?: { width: number; height: number },
): PieGeometry {
  const multiRing = ringCount > 1;

  // How much of its own box the circle fills. Not a constant 0.70: a wide, short host puts the
  // legend in a column beside the chart instead of a row beneath it, and the circle then gets a
  // differently shaped box to fill — one that already has the legend taken out of it, since the
  // legend is React markup outside the canvas. `pieLegendPlacement` (above) makes that choice
  // once, from the size of the WHOLE host; all that's left here is the fraction of its own box
  // each layout fills. A hidden legend reserves nothing, and so keeps today's geometry.
  const side = (cfg.showLegend ?? true) && cfg.legendPlacement === "side";
  const outerFraction = side ? SIDE_LEGEND_DIAMETER_FRACTION : BOTTOM_LEGEND_DIAMETER_FRACTION;

  // ONE ring is the deliberate exception to Metabase's ratios: it keeps egret's OWN donut on/off
  // semantics (donut:false -> no hole, donut:true -> 45/70 of the outer radius) rather than
  // Metabase's 3/5. A plain pie's proportions were never identified as unreadable, and moving the
  // hole would change what a saved donut LOOKS LIKE as a shape rather than how it is labelled.
  // TWO OR MORE rings is where the ratio is unconditional: there is no sensible multi-ring pie
  // with no hole at all.
  const holeRatio = multiRing
    ? HOLE_RATIO[Math.min(ringCount, 3)]
    : (cfg.donut ? 45 / 70 : 0);

  // `containerSize` is the chart's REAL rendered pixel size — this builder is pure data with no DOM
  // access, so EChartsRenderer measures it (ResizeObserver) and threads it through by calling this
  // function rather than handing over a plain option object (see that file). Unknown (most unit
  // tests, which aren't testing geometry at all) means the radii stay in ECharts' own percent-of-
  // the-shorter-side units and the label fit pass is skipped entirely — see `availableSpace` in
  // buildPieOption for why "don't guess" is the safe direction there.
  // A 0x0 host counts as UNKNOWN, not as a zero-sized pie: EChartsRenderer measures once on mount,
  // which for a not-yet-laid-out element is 0x0, and pixel radii of 0 would draw literally nothing
  // until the ResizeObserver's first real tick. Percent radii draw correctly at whatever size the
  // element turns out to have.
  const sideLengthPx = containerSize ? Math.min(containerSize.width, containerSize.height) : 0;
  const hasMeasuredSize = sideLengthPx > 0;
  const diameterPx = Math.max(
    0,
    Math.min(outerFraction * sideLengthPx, sideLengthPx - 2 * PIE_SIDE_PADDING_PX, PIE_MAX_DIAMETER_PX),
  );
  const outerRadiusPx = diameterPx / 2;
  const innerRadiusPx = outerRadiusPx * holeRatio;

  const fmtPct = (v: number) => `${parseFloat(v.toFixed(2))}%`;
  // ECharts takes a radius as either a pixel number or a percent of half the container's shorter
  // side, and accepts both in the same [inner, outer] pair. Pixels whenever the real size is known
  // — that is the only unit an absolute cap can be expressed in, and it makes the numbers the fit
  // pass measures against literally the numbers the renderer draws with, rather than a conversion
  // of them. Percent otherwise, which is exactly the geometry this file drew before the cap.
  const radius: [number, number] | [string, string] = hasMeasuredSize
    ? [innerRadiusPx, outerRadiusPx]
    : [fmtPct(100 * outerFraction * holeRatio), fmtPct(100 * outerFraction)];

  // Where the circle sits in its own box: DEAD CENTRE, in both layouts.
  //
  // The bottom layout used to keep a 2% upward lift "off the row beneath it". There is no row
  // beneath it inside the canvas any more — the legend became React markup outside it — so the
  // lift was left over from a legend that had already moved out, and all it did was put the
  // circle 2% of the height above the box's own middle. It also cost the hole's readout its
  // centre: `buildPieHoleGraphic` centres the total on the CANVAS (ECharts' `left: "center",
  // top: "center"`), which is the one point a shifted pie is no longer at — measured at 520px
  // tall, the total's block sat ~10px below the rings it belongs to.
  //
  // Centring both on the same point is what keeps them together BY CONSTRUCTION rather than by
  // two numbers agreeing: there is no offset here for the hole to have to reproduce.
  const center: [string, string] = ["50%", "50%"];

  return { holeRatio, diameterPx, outerRadiusPx, innerRadiusPx, hasMeasuredSize, radius, center };
}

// ---------------------------------------------------------------------------------------------
// The hole.

/** What the hole says when nothing is being pointed at, plus everything a hover readout needs to
 * say something else. Built once per data/config change (see `buildPieHole`) so that reacting to
 * a hover costs two `measureText` calls, not another pass over the rows. */
export interface PieHoleModel {
  /** The hole is written in at all: there IS one, the total is on, and there is data to total. */
  shown: boolean;
  /** How many rings the pie draws — the hole's size and its value font size both follow it. */
  ringCount: number;
  /** The chart's grand total, formatted with the metric's own settings. */
  totalText: string;
  /** The metric formatter, so a hovered slice's value reads exactly like the total does. */
  format: (v: unknown) => string;
}

/** The slice the reader is pointing at, as the hole needs to describe it. Both fields come
 * straight off the hover event (a sunburst's `mouseover` carries the node's own name and value),
 * so nothing has to look the slice back up in the data. */
export interface PieHoleReadout {
  name: string;
  value: unknown;
  /** Metabase uppercases the name when the hover came from the LEGEND rather than from the chart
   * — the two are different gestures and the hole says which one you are making. */
  fromLegend: boolean;
}

/** Metabase's `total.*` constants: the regular value size, and the escape hatch it drops to when
 * the text won't fit the hole.
 *
 * THE SMALL SIZE IS A FIT DECISION, NOT A RING COUNT. This used to start a three-ring pie at
 * `_SM` on the theory that its hole (1/4 of the outer edge) was too tight for 22px — which asked
 * the wrong question, and answered it without measuring: on the chart the live comparison was
 * made against, the hole's radius is 66px and `22,437` needs about 78 of the 132px available, so
 * the small size was being applied to text that fits. Metabase picks 22 and drops to 17 only when
 * the measured string exceeds `innerRadius * 2` (option.ts's `hasSufficientWidth`), which is what
 * `buildPieHoleGraphic` does now. */
const TOTAL_VALUE_FONT_SIZE = 22;
const TOTAL_VALUE_FONT_SIZE_SM = 17;
const TOTAL_LABEL_FONT_SIZE = 14;
/** Where each line sits INSIDE the centred group (Metabase's own offsets): the caption 26px below
 * the top of the value, and a value drawn with no caption under it nudged down 8px so that the
 * group — which is centred by its own bounding box — centres the lone line rather than a block
 * that still reserves room for a caption. */
const TOTAL_LABEL_TOP = 26;
const TOTAL_LONE_VALUE_TOP = 8;
const TOTAL_CAPTION = "Total";
/** Stable identities so the hole can be re-stated on its own — see `buildPieHoleGraphic`. */
const TOTAL_GROUP_ID = "pie-total";
const TOTAL_VALUE_ID = "pie-total-value";
const TOTAL_LABEL_ID = "pie-total-label";

function pieHoleModel(rings: Slice[][], cfg: PieBuildConfig, format: (v: unknown) => string): PieHoleModel {
  const ringCount = rings.length;
  const data = rings[0] ?? [];
  // DEFAULTS ON FOR A MULTI-RING PIE. Measured against a real Metabase (question 7, "Product
  // breakdown") at 1440x920: it renders `2,852` over `Total` in the hole, egret rendered nothing,
  // and an empty void in the middle of three rings is what made the chart read as unfinished. A
  // one-ring donut is left exactly as it was — its hole is small enough to read as deliberate, and
  // changing it would change what every saved donut looks like. `??`, not `||`, so an explicit
  // `false` still turns it off; resolved here at render time and never written into stored
  // settings (this program's standing rule).
  //
  // Gated on there BEING a hole rather than on `cfg.donut`: a multi-ring pie always has one (see
  // pieGeometry), whatever `donut` says, so keying off `donut` would have hidden the total on
  // exactly the charts this default exists for.
  const showTotal = cfg.showTotal ?? ringCount > 1;
  const shown =
    showTotal && data.length > 0 && pieGeometry(cfg, ringCount).holeRatio > 0;
  // The innermost ring's total is the grand total: it's summed over every outer dimension already.
  return { shown, ringCount, totalText: format(data.reduce((t, s) => t + s.value, 0)), format };
}

/**
 * Everything the hole can read out, from the data and settings alone.
 *
 * Exported so React can build it ONCE per data/config change and then re-render only the hole on
 * hover — the option itself must not be rebuilt per hover event, because building it runs a
 * per-slice `measureText` pass over every ring.
 */
export function buildPieHole(columns: ResultColumn[], rows: unknown[][], cfg: PieBuildConfig): PieHoleModel {
  const { rings } = buildPieRings(columns, rows, cfg);
  return pieHoleModel(rings, cfg, metricFormatter(cfg));
}

/** One `graphic` text element of the hole. `top` is an offset INSIDE the group below (pixels from
 * the top of its box), not a position on the canvas — the group is what gets centred. */
interface PieHoleText {
  id: string;
  type: "text";
  left: "center";
  top: number;
  style: {
    text: string;
    fontSize: number;
    fontWeight: number;
    fontFamily: string;
    fill: string;
    textAlign: "center";
  };
}

/** The hole, as ECharts states it: ONE group holding both lines. */
export interface PieHoleGraphic {
  id: string;
  type: "group";
  left: "center";
  top: "center";
  children: PieHoleText[];
}

/**
 * The hole's two lines of text, as ECharts `graphic` elements — Metabase's `getTotalGraphicOption`.
 *
 * `readout` is what the reader is pointing at, or `null` for the rest state:
 *   - a slice under the pointer -> that slice's value over its name
 *   - a slice under the pointer VIA THE LEGEND -> the same, with the name uppercased
 *   - nothing               -> the chart total over the word "Total"
 *
 * Returned as a bare array (never wrapped in an option) so it can be used two ways: baked into the
 * option `buildPieOption` returns for the rest state, and re-applied ON ITS OWN as a merge-mode
 * partial option when the hover changes. Both go through this one function, so the hovered hole
 * cannot end up styled differently from the resting one.
 *
 * THE TWO LINES ARE ONE GROUP, which is Metabase's construction and the fix for what the live
 * comparison found. They used to be two independent `graphic` texts pinned to 43% and 53% of the
 * canvas height — a spacing that scales with the chart (50.9px apart on the measured host, against
 * Metabase's 22) and a midpoint that only lands on the rings if the pie happens to be centred at
 * 48%. A group states the relationship instead: the caption sits `TOTAL_LABEL_TOP` below the
 * value's own top whatever size the chart is, and `left/top: "center"` centres the pair — as one
 * box, by its own bounding rect — on the canvas's middle, which is exactly where `pieGeometry`
 * puts the rings.
 */
export function buildPieHoleGraphic(
  hole: PieHoleModel,
  cfg: PieBuildConfig,
  readout: PieHoleReadout | null,
  containerSize?: { width: number; height: number },
  measureText: MeasureText = defaultMeasureText,
): PieHoleGraphic[] | undefined {
  if (!hole.shown) return undefined;

  const { innerRadiusPx, hasMeasuredSize } = pieGeometry(cfg, hole.ringCount, containerSize);

  let valueText = readout ? hole.format(readout.value) : hole.totalText;
  let labelText = readout
    ? (readout.fromLegend ? readout.name.toUpperCase() : readout.name)
    : TOTAL_CAPTION;
  // The full size unless the hole is measurably too narrow for it — see TOTAL_VALUE_FONT_SIZE.
  let valueFontSize = TOTAL_VALUE_FONT_SIZE;

  // The fit pass runs only against a REAL measured size, the same rule the slice labels follow:
  // "we don't know how big this is" must never become "guess small", or a perfectly readable
  // total gets truncated to an ellipsis against a number nobody measured. With no size, the two
  // strings are drawn in full and ECharts clips nothing.
  if (hasMeasuredSize) {
    // Metabase measures with the VALUE's font size for both lines: "technically incorrect for the
    // label text since it uses a smaller font, however using the value font size for measurements
    // makes up for the inaccuracy of our heuristic and provided a good end result." At the size it
    // intends to DRAW, so the question the fit answers is the one on screen.
    const fontStyle = { size: TOTAL_VALUE_FONT_SIZE, weight: 700 };
    const holeWidth = innerRadiusPx * 2;
    valueText = truncateLabelText(valueText, holeWidth, measureText, fontStyle);
    if (readout) labelText = truncateLabelText(labelText, holeWidth, measureText, fontStyle);

    const widest = Math.max(measureText(valueText, fontStyle), measureText(labelText, fontStyle));
    if (holeWidth < widest) {
      // Too narrow for both lines. The total is the chart's headline number, so it survives alone;
      // a hover readout is transient and goes entirely rather than showing a name with no value or
      // a value with no name.
      if (readout) { valueText = ""; labelText = ""; } else { labelText = ""; }
      valueFontSize = TOTAL_VALUE_FONT_SIZE_SM;
    }
  }

  return [
    {
      id: TOTAL_GROUP_ID, type: "group", left: "center", top: "center",
      children: [
        {
          id: TOTAL_VALUE_ID, type: "text", left: "center",
          top: labelText ? 0 : TOTAL_LONE_VALUE_TOP,
          style: {
            text: valueText, fontSize: valueFontSize, fontWeight: 700,
            // Named per element: a `graphic` is a zrender element of its own and does NOT inherit
            // the option root's `textStyle` the way a series label does, so the hole would stay in
            // the browser's default font while the rings around it moved. Metabase states it on
            // both of these children for the same reason.
            fontFamily: resolveFontFamily(),
            fill: resolveColor("var(--foreground)", COLOR_FALLBACK.text), textAlign: "center",
          },
        },
        {
          id: TOTAL_LABEL_ID, type: "text", left: "center", top: TOTAL_LABEL_TOP,
          style: {
            text: labelText, fontSize: TOTAL_LABEL_FONT_SIZE, fontWeight: 700,
            fontFamily: resolveFontFamily(),
            fill: resolveColor(OTHER_SLICE_COLOR, COLOR_FALLBACK.axisLabel), textAlign: "center",
          },
        },
      ],
    },
  ];
}

/**
 * Every ring's finalized slices — aggregated, filtered, Other-folded, coloured and renamed —
 * innermost first, plus each ring's own "did this key survive or fold into Other" map.
 *
 * Split out of `buildPieOption` because the React legend needs ring 0's slices exactly as the
 * chart draws them (same names after a rename, same colours after an override or an Other fold),
 * and re-deriving that beside the option is how a legend and a chart drift apart. `buildPieLegend`
 * and `buildPieOption` therefore go through this same pass; only the option builder continues on
 * to the expensive per-slice label fit.
 */
function buildPieRings(
  columns: ResultColumn[],
  rows: unknown[][],
  cfg: PieBuildConfig,
): { rings: Slice[][]; groupOfByLevel: Map<string, string>[] } {
  const idx = Object.fromEntries(columns.map((c, i) => [c.name, i]));
  const dims = cfg.dimensions ?? [];
  const mi = cfg.metric != null ? idx[cfg.metric] : undefined;

  // One entry per ring, innermost first. Aggregate level L groups rows by dimensions[0..L] (so the
  // innermost ring is coarsest, summing over every dimension nested inside it), run that ring's own
  // slice through buildSlices, then reorder it under its parent ring's finalized order.
  const rings: Slice[][] = [];
  // groupOfByLevel[L] is ring L's OWN "did this key survive or fold into Other" map (buildSlices'
  // `groupOf`), kept around after the loop so the colour-inheritance pass below and the tree
  // assembly in buildPieOption can both resolve a ring L+1 slice's parentKey through its parent's
  // fold state, not just its raw identity.
  const groupOfByLevel: Map<string, string>[] = [];
  if (mi != null && dims.length > 0) {
    let parentGroupOf: Map<string, string> | undefined;
    for (let level = 0; level < dims.length; level++) {
      const path = dims.slice(0, level + 1);
      const dimIdxs = path.map((d) => idx[d]);
      const units = path.map((d) => cfg.dimensionUnits?.[d]);
      let items = aggregateRing(rows, dimIdxs, units, mi);
      // A row whose ancestor got dropped outright (not even folded to Other) at the parent ring has
      // no home here either: its parent's slice doesn't exist to nest under.
      if (level > 0 && parentGroupOf) {
        items = items.filter((it) => it.parentKey != null && parentGroupOf!.has(it.parentKey));
      }
      // Ring 0 only (task 6): a hidden slice's rows are dropped BEFORE buildSlices sees them,
      // so its value never enters the total or the Other-threshold computation — the rest of
      // the ring re-bases as if the hidden category never existed. This also means `groupOf`
      // below never gets an entry for it, so the orphan-drop above removes its children in the
      // next ring out for free, the same way it already handles a parent lost to the sign/zero
      // rule.
      if (level === 0 && cfg.hiddenSlices?.length) {
        items = items.filter((it) => !cfg.hiddenSlices!.includes(it.name));
      }
      const { slices, groupOf } = buildSlices(items, cfg);
      // A ring below 0 keeps buildSlices' own order (value-desc, each parent's "Other" last within
      // its group). It no longer has to be re-sorted into parent-contiguous runs: the sunburst
      // nests these under their parents itself (groupByParent), and a node's children are drawn
      // consecutively inside its arc whatever order the ring array happened to be in.
      let ordered = slices;
      // Ring 0 only: sliceOrder is a display-order override keyed on the same raw value as
      // hiddenSlices — applied AFTER buildSlices, so it can't influence the Other threshold.
      // Ring 0's order IS the sunburst's top-level draw order, so every outer ring follows it.
      if (level === 0 && cfg.sliceOrder?.length) {
        const rank = new Map(cfg.sliceOrder.map((n, i) => [n, i]));
        ordered = [...ordered].sort(
          (a, b) => (rank.get(a.name) ?? Number.MAX_SAFE_INTEGER) - (rank.get(b.name) ?? Number.MAX_SAFE_INTEGER),
        );
      }
      rings.push(ordered);
      groupOfByLevel.push(groupOf);
      parentGroupOf = groupOf;
    }
  }
  const effectiveRings = rings.length > 0 ? rings : [[]];

  // Per-slice colour overrides win over the palette slot (the "Other" slice keeps its muted tint).
  // Matched by NAME on every ring, not just ring 0 — so a value that happens to appear as a
  // dimension value in more than one ring (e.g. the same string used for both a region and a
  // category) takes the override in both places it appears. Pre-existing behaviour, unchanged
  // here; flagged because it's easy to assume "override" means "ring-0 only".
  //
  // The override goes through `resolveColor` like every other colour in this file: it arrives
  // from the chart's settings JSONB, which is never whitelisted server-side, so it can be
  // hand-written, migrated or simply mistyped. `resolveColor` is the one place that validates
  // a colour from a less trusted source — anything the renderer can't draw comes back as the
  // fallback, and an empty-string fallback (not a colour, so unambiguous) turns that into "no
  // override": the slice keeps its palette slot instead of being painted with a value ECharts
  // would ignore and `getTextColorForBackground` would then silently mis-contrast against.
  if (cfg.sliceColors) {
    for (const ring of effectiveRings) {
      for (const s of ring) {
        const c = cfg.sliceColors[s.name];
        if (!c || s.name === OTHER_NAME) continue;
        const resolved = resolveColor(c, "");
        if (resolved) s.itemStyle = { color: resolved };
      }
    }
  }

  // Ring n's slice colour: its family's own BASE colour (an implicit palette slot, or a
  // sliceColors override — on whichever ring introduced it) at that ring's VARIANT.
  //
  // METABASE'S THREE FIXED LEVELS, not a computed per-depth ramp: inner ring dark, middle ring the
  // base, outer ring light (`getRingColorAlias`, called with "inner"/"middle"/"outer" from its own
  // model). egret used to start at the base and lighten OUTWARD in OKLCH, deliberately holding
  // chroma so the outer rings stayed saturated — which is why the two charts disagreed on every
  // ring at once and egret read as the more vivid of the pair: measured side by side, egret's
  // ring-1 blue was #76C4FF against Metabase's #87BCEC, and its innermost ring was the medium base
  // where Metabase's is the deep #227FD2. See `ringVariantColor` (palette.ts) for the derivation
  // and the check that it reproduces all 24 of Metabase's own accent hexes.
  //
  // The variants are DERIVED from the base rather than looked up in a table of accents, which is
  // where this parts company with Metabase: its own model maps a hex back to an accent name, so a
  // slice whose colour isn't one of the eight (a per-slice override, egret's muted "Other" grey)
  // gets no variant at all and renders flat at every depth. Deriving gives an override the same
  // three-level treatment the palette gets.
  //
  // `anchors` maps a ring's own key (or OTHER_KEY for its synthetic "Other" bucket) to
  // { base, sinceLevel, baseVariant }: the colour to derive from, the ring level it was FIXED at,
  // and the variant that level itself renders in — so a slice's own variant is
  // `level - sinceLevel + baseVariant`. Ring 0 fixes every slice's base at level 0 with
  // baseVariant -1 (the dark inner ring) once there is more than one ring, and 0 for a single-ring
  // pie, which is left exactly as it was drawn before. Two things reset the anchor instead of
  // propagating it unchanged:
  //  - a ring's own synthetic "Other" bucket already has a fixed muted colour (set above,
  //    unconditionally, at every ring depth) — it becomes a fresh anchor at ITS level, holding
  //    that colour as its own base (variant 0), so anything nested under it takes the LIGHT
  //    variant of Other's muted grey rather than of whatever palette slot its constituent
  //    (folded-away) parents would have had;
  //  - a sliceColors override on a ring below 0 (see the note above — the same NAME can match
  //    more than one ring) is itself a fresh anchor at that level, for the same reason.
  //
  // Ring 0's implicit palette slots are RESOLVED HERE rather than left to ECharts. They used to be
  // left implicit (no `itemStyle` at all on an un-overridden ring-0 slice), which was safe while
  // each ring was its own `pie` series: a pie colours a datum by its INDEX, so slot i was slot i
  // whatever else was going on. A `sunburst` colours a top-level node by NAME
  // (sunburstVisual.js -> `getColorFromPalette(current.name, paletteScope)`) and skips that call
  // entirely for a node that already has a colour — so one `sliceColors` override on ring 0 would
  // consume no slot and slide every LATER slice's colour up by one, while the React legend and the
  // label-contrast pass (which both compute `seriesColor(i)` from the ring index) kept naming the
  // old ones. Pinning removes the divergence by construction: every ring-0 slice states the colour
  // it is drawn in, and there is only one answer to read.
  type ColorAnchor = { base: string; sinceLevel: number; baseVariant: number };
  // Ring 0's own variant: the DARK one on a nested pie, the base itself on a single-ring pie —
  // which is Metabase's `hasMultipleRings` gate, and what leaves every saved one-ring pie looking
  // exactly as it does today.
  const ring0Variant = effectiveRings.length > 1 ? -1 : 0;

  (effectiveRings[0] ?? []).forEach((s, i) => {
    s.itemStyle ??= { color: seriesColor(i) };
  });
  let anchors = new Map<string, ColorAnchor>();
  (effectiveRings[0] ?? []).forEach((s) => {
    // The anchor holds the family's BASE colour, not the colour ring 0 is painted in — every
    // deeper ring's variant is measured from the base, so darkening ring 0 in place would make
    // ring 1 the base's own lightening of an already-darkened colour.
    anchors.set(s.key ?? OTHER_KEY, { base: s.itemStyle!.color, sinceLevel: 0, baseVariant: ring0Variant });
  });
  (effectiveRings[0] ?? []).forEach((s) => {
    s.itemStyle = { color: ringVariantColor(anchors.get(s.key ?? OTHER_KEY)!.base, ring0Variant) };
  });
  for (let level = 1; level < effectiveRings.length; level++) {
    const parentFold = groupOfByLevel[level - 1];
    const nextAnchors = new Map<string, ColorAnchor>();
    for (const s of effectiveRings[level]) {
      const groupKey = s.parentKey != null ? parentFold?.get(s.parentKey) : undefined;
      const ancestorAnchor = groupKey != null ? anchors.get(groupKey) : undefined;
      const alreadyStyled = s.itemStyle != null; // this ring's own Other bucket, or its own sliceColors override
      if (!alreadyStyled && ancestorAnchor) {
        const variant = level - ancestorAnchor.sinceLevel + ancestorAnchor.baseVariant;
        s.itemStyle = { color: ringVariantColor(ancestorAnchor.base, variant) };
      }
      const key = s.key ?? OTHER_KEY;
      if (alreadyStyled) {
        // A colour fixed at THIS level is its own family's base, so what nests under it takes the
        // light variant of it — one step out, not two.
        nextAnchors.set(key, { base: s.itemStyle!.color, sinceLevel: level, baseVariant: 0 });
      } else if (ancestorAnchor) {
        nextAnchors.set(key, ancestorAnchor); // propagate the SAME anchor, not this ring's own colour
      } else {
        // Defensive only: every non-Other slice's parentKey should resolve via groupOf (orphans
        // are filtered out earlier in the main loop) — a muted grey is a safe, visible fallback.
        nextAnchors.set(key, {
          base: resolveColor(OTHER_SLICE_COLOR, COLOR_FALLBACK.axisLabel), sinceLevel: level, baseVariant: 0,
        });
      }
    }
    anchors = nextAnchors;
  }

  // Ring 0 only, and deliberately LAST: a display rename must not disturb anything computed
  // above by raw value — the sliceColors match (by name, above) and the colour-inheritance
  // anchors (by key, just above) have already run against the raw value, so renaming now only
  // changes what's shown, never what's found by hiddenSlices/sliceOrder/sliceColors.
  if (cfg.sliceNames) {
    for (const s of effectiveRings[0] ?? []) {
      const renamed = cfg.sliceNames[s.name];
      if (renamed) s.name = renamed;
    }
  }

  return { rings: effectiveRings, groupOfByLevel };
}

/**
 * Whether this pie has a single wedge to draw.
 *
 * `false` is NOT the same as "no rows". A pie can only show a share of something, so every zero,
 * every non-number and (in a mixed-sign column) every negative is dropped before a ring is built
 * — which means a query that returned plenty of rows can still leave ring 0 empty. Untouched, that
 * reached the canvas as an empty sunburst: a blank rectangle with no message on it, while the
 * chart's own aria description already read "No data.".
 *
 * Exported so ChartRenderer can say so in the shell's own empty state rather than handing ECharts
 * nothing to draw. Metabase's answer is a single "Other" slice in its muted text colour — a whole
 * grey disc — which egret does not copy here: egret has a real empty state, and a full circle in
 * one colour still looks like one category worth all of the chart. What is worth copying is
 * Metabase's point, which is that an all-zero pie should read as "nothing to show" rather than as
 * broken.
 */
export function pieHasSlices(columns: ResultColumn[], rows: unknown[][], cfg: PieBuildConfig): boolean {
  return buildPieRings(columns, rows, cfg).rings[0].length > 0;
}

/**
 * The pie's legend, as data — the React component beside the chart (PieLegend.tsx) renders it.
 *
 * Ring 0 only: every ring's names used to go into one flat ECharts legend, but only ring 0's
 * slices have a single, unambiguous colour — an outer ring's "Widgets" can appear once per parent
 * slice, each a different lightened shade, so listing it would claim a colour it doesn't
 * consistently have. Ring 0's share of its own ring already IS its share of the whole chart (every
 * outer ring re-partitions the same rows), so its percentage needs no explanation of what it is a
 * percentage OF.
 *
 * Shows by default (`?? true`, not `||`, so an explicit `false` still turns it off): a pie's
 * colours are meaningless without the key that says which is which.
 */
export function buildPieLegend(
  columns: ResultColumn[],
  rows: unknown[][],
  cfg: PieBuildConfig,
): PieLegendEntry[] {
  if (!(cfg.showLegend ?? true)) return [];
  const { rings, groupOfByLevel } = buildPieRings(columns, rows, cfg);
  const ring0 = rings[0] ?? [];
  const visibility = resolvePercentVisibility(cfg.percentVisibility, rings.length);
  const withPercent = visibility === "legend" || visibility === "both";
  const total = ring0.reduce((t, s) => t + s.value, 0);
  // The legend's OWN precision, over the only shares it lists (ring 0's, each already a share of
  // the whole chart) and at the legend's own budget — see computeMaxPercentDecimals. An explicit
  // `decimals` stays authoritative; this is only what "unset" means.
  const pct = pctFormatter(
    cfg.decimals
      ?? computeMaxPercentDecimals(
        total > 0 ? ring0.map((s) => s.value / total) : [],
        LEGEND_PERCENT_SIGNIFICANT_DIGITS,
      ),
  );

  // The sunburst's datum list, walked without building it: ECharts wraps `series.data` in one
  // virtual root (SunburstSeries.getInitialData) and `Tree.createTree` pushes nodes depth-first,
  // pre-order — so index 0 is that root and each ring-0 slice starts right after the previous
  // one's entire subtree. Counting subtree sizes is all that is needed, and it costs no text
  // measurement, which is the whole reason this doesn't just build the option and read it back.
  const childrenByParentKey = pieChildrenByParent(rings, groupOfByLevel);
  const toggledOff = new Set(cfg.toggledOffSlices ?? []);
  let nextDataIndex = 1;

  return ring0.map((s, i) => {
    // The colour the slice is actually drawn in — read off the slice rather than re-derived, since
    // buildPieRings now pins every ring-0 slice's colour explicitly (see the note there on why the
    // sunburst's name-keyed palette made an implicit slot unsafe). `?? seriesColor(i)` is a
    // belt-and-braces default, not a second source of truth.
    const color = s.itemStyle?.color ?? seriesColor(i);
    let sliceDataIndex: number | undefined;
    if (!toggledOff.has(s.name)) {
      sliceDataIndex = nextDataIndex;
      nextDataIndex += subtreeSize(s, 0, childrenByParentKey);
    }
    const base = { name: s.name, color, value: s.value, sliceDataIndex };
    return withPercent && total > 0 ? { ...base, percent: pct((s.value / total) * 100) } : base;
  });
}

// ---------------------------------------------------------------------------------------------
// The three things the chart and the breakdown table must not be able to disagree about: which
// ring-0 slices are showing, how precise every reported share is, and whether a ring's own total
// diverges from the chart's. All three used to live inline in `buildPieOption`; the table reports
// the same numbers the arcs do, so a second copy of any of them is a divergence waiting to happen.

/** Ring 0, minus whatever the reader has clicked off in the legend — the slices the sunburst
 * actually roots its tree at. `cfg.hiddenSlices` is NOT this: that is a saved curation, already
 * applied by `buildPieRings` before any total or threshold was computed. */
function pieRootSlices(ring0: Slice[], cfg: PieBuildConfig): Slice[] {
  const toggledOff = new Set(cfg.toggledOffSlices ?? []);
  return toggledOff.size === 0 ? ring0 : ring0.filter((s) => !toggledOff.has(s.name));
}

/**
 * How precise the percentages this chart REPORTS are — the slice labels, the tooltip's table and
 * the breakdown table, which all share this one answer.
 *
 * Metabase's `getAllSlicePercentages`: every slice at every depth, each as a share of its own
 * parent (which at ring 0 is the chart), at the slice budget rather than the legend's. It pushes an
 * "Other" bucket's own share and then STOPS — the slices folded inside it are a breakdown of Other,
 * not of the chart, and their shares are not what a reader is comparing labels across. Kept
 * faithful to that even though egret's tooltip (and now its table) does list them.
 */
function pieSharePrecision(
  rootSlices: Slice[],
  rootTotal: number,
  childrenByParentKey: Map<string, Slice[]>[],
  cfg: PieBuildConfig,
  /** The significant-digit budget. Defaults to the in-arc one, because that is who most callers
   * are. The breakdown TABLE passes the legend's instead: the two sit on the same page, and at
   * the tighter budget a reader saw `APAC 34.6%` in the legend and `APAC 35%` in the table — not
   * merely coarser, a different number for the same slice. A table has room; a wedge does not. */
  significantDigits: number = SLICE_PERCENT_SIGNIFICANT_DIGITS,
): number {
  const collect = (slices: Slice[], level: number, parentValue: number, out: number[]): number[] => {
    for (const s of slices) {
      out.push(parentValue > 0 ? s.value / parentValue : 0);
      if (s.key != null && s.key.startsWith(OTHER_KEY)) continue;
      collect(childrenByParentKey[level]?.get(s.key ?? OTHER_KEY) ?? [], level + 1, s.value, out);
    }
    return out;
  };
  return (
    cfg.decimals
    ?? computeMaxPercentDecimals(collect(rootSlices, 0, rootTotal, []), significantDigits)
  );
}

/**
 * A ring drawing less (or more) than the chart's own total, said out loud.
 *
 * Every ring's total *should* equal the first ring's (each ring re-partitions the same rows), but
 * the zero/negative filter runs independently per ring (see buildSlices), so a surviving parent can
 * still lose children there — the sign/zero rule is applied per-item on the finer partition, not by
 * re-netting the parent's already-signed total. Nothing about the drawn chart reveals this, so the
 * chart's aria description and the breakdown table both carry the note.
 */
function pieDivergenceNotes(rings: Slice[][], dims: string[], fmt: (v: unknown) => string): string[] {
  const ringTotals = rings.map((ring) => ring.reduce((t, s) => t + s.value, 0));
  return ringTotals
    .map((t, level) => ({ t, level }))
    .filter(({ t, level }) => level > 0 && Math.abs(t - ringTotals[0]) > 1e-9)
    .map(({ t, level }) => `Note: the ${dims[level]} ring totals ${fmt(t)}, not ${fmt(ringTotals[0])} like the chart's overall total — some rows were dropped by the zero/negative filter at that ring.`);
}

/** One row of the breakdown table: one DRAWN slice, at the depth the chart draws it. */
export interface PieBreakdownRow {
  /** Which ring, counted from the centre — 0 is the innermost. Indexes `PieBreakdownModel.levels`. */
  depth: number;
  /** The slice's display name: after a `sliceNames` rename, and `"Other"` for a folded bucket. */
  name: string;
  /** The wedge this one sits inside, and its own, innermost first — empty at ring 0. This is the
   * POSITION half of what a sunburst encodes and a flat list cannot: an outer "Widgets" appears
   * once per parent, and only its ancestors say which one this is. */
  parents: string[];
  /** The slice's true metric value, through the chart's own formatter. Never the inflated figure a
   * sub-0.5% wedge is DRAWN at (see MIN_DRAWN_SLICE_FRACTION). */
  value: string;
  /** Its share of its parent — of the whole chart at ring 0 — formatted at the LEGEND's precision,
   * so a root slice reads character-for-character the same here as in the legend beside it. The
   * in-arc budget is deliberately not used: a wedge is cramped and a table is not, and at the
   * tighter one the same slice read `34.6%` in the legend and `35%` here. */
  share: string;
}

/**
 * The pie's arcs, as a structure — what `PieBreakdown.tsx` renders beside the chart.
 *
 * `rows` is empty for anything below two rings, which is the decision, not an accident: a
 * single-ring pie is a flat (name, value) list and the chart's own `aria` description already
 * reads that out in full. A second copy of the same list is a cost to a screen-reader user, not a
 * gain, so nothing is offered there at all.
 */
export interface PieBreakdownModel {
  /** Depth-first, in the order the sunburst lays the slices out: each slice immediately followed
   * by everything nested inside it. Empty below two rings — see above. */
  rows: PieBreakdownRow[];
  /** Each ring's dimension, innermost first, through `columnSettings` like every other label. */
  levels: string[];
  /** What the value column is measuring. */
  metric: string;
  /** See `pieDivergenceNotes`. */
  notes: string[];
}

/**
 * Every slice the chart draws, with its ancestry, its value and its share of its parent.
 *
 * THE ACCESSIBILITY GAP THIS CLOSES. Past two levels a sunburst encodes meaning in COLOUR (the
 * family, plus a lightened shade per ring) and in POSITION (a nested arc under one specific parent
 * wedge). A text description recovers neither, so the chart's `aria` description could only ever
 * read out ring 0 — a screen-reader user got the innermost ring and no way to know the outer ones
 * existed. `ui-ux-pro-max` grades the shape C for accessibility for exactly this and calls a table
 * alternative mandatory for production use.
 *
 * Built from the SAME pass the chart is (`buildPieRings` + `pieChildrenByParent` — the sunburst's
 * `children` tree, as lookup tables), and through the same `pieRootSlices` / `pieSharePrecision` /
 * `metricFormatter` helpers, for the reason `buildPieLegend` is: a table that re-derived any of
 * this from the rows would be a second answer to every question the chart already answers, and the
 * two would drift on the first rename, hide, fold or format change. What it costs is one extra
 * `buildPieRings` pass, which is the same trade the legend already makes — and it skips the
 * expensive part (the per-slice canvas label fit) entirely.
 */
export function buildPieBreakdown(
  columns: ResultColumn[],
  rows: unknown[][],
  cfg: PieBuildConfig,
): PieBreakdownModel {
  const dims = cfg.dimensions ?? [];
  const fmt = metricFormatter(cfg);
  // The same display-title lookup the tooltip's header takes, so a renamed column is renamed here
  // too rather than reverting to its raw name in the one place a screen reader reads it.
  const label = (col: string | undefined) => (col ? cfg.columnSettings?.[col]?.column_title || col : "");
  const { rings, groupOfByLevel } = buildPieRings(columns, rows, cfg);
  const model = { rows: [] as PieBreakdownRow[], levels: dims.map(label), metric: label(cfg.metric), notes: [] as string[] };
  // One ring (or none at all) — nothing here that the chart's own aria description doesn't say.
  if (rings.length < 2) return { ...model, levels: [] };

  const childrenByParentKey = pieChildrenByParent(rings, groupOfByLevel);
  const rootSlices = pieRootSlices(rings[0], cfg);
  const rootTotal = rootSlices.reduce((t, s) => t + s.value, 0);
  // The legend's budget, not the in-arc one — see `pieSharePrecision`'s last parameter.
  const decimals = pieSharePrecision(
    rootSlices, rootTotal, childrenByParentKey, cfg, LEGEND_PERCENT_SIGNIFICANT_DIGITS,
  );
  const pct = pctFormatter(decimals);

  const out: PieBreakdownRow[] = [];
  // Depth-first, parents before children — the order ECharts itself lays the tree out in, so the
  // table reads down the chart rather than in an order of its own invention.
  const walk = (slices: Slice[], level: number, parentValue: number, parents: string[]) => {
    for (const s of slices) {
      // Rounded exactly as `toNode` rounds a slice's own `slicePercent` (and then formatted by the
      // same `pctFormatter`), so this cell and the tooltip's cannot print different strings.
      const share = parseFloat(((parentValue > 0 ? s.value / parentValue : 0) * 100).toFixed(decimals));
      out.push({ depth: level, name: s.name, parents, value: fmt(s.value), share: pct(share) });
      // An "Other" bucket at the outermost ring has no children in the tree at all — the slices it
      // folded away are not drawn, so they are not listed. Below that it does, and they are.
      walk(childrenByParentKey[level]?.get(s.key ?? OTHER_KEY) ?? [], level + 1, s.value, [...parents, s.name]);
    }
  };
  walk(rootSlices, 0, rootTotal, []);

  return { ...model, rows: out, notes: pieDivergenceNotes(rings, dims, fmt) };
}

const HTML_ESCAPES: Record<string, string> = {
  "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
};

/** Every string below goes into an ECharts tooltip via `innerHTML`, and every one of them is a
 * DIMENSION VALUE — data, from a query the reader does not necessarily control. */
function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (c) => HTML_ESCAPES[c]);
}

/**
 * The pie's tooltip: a BREAKDOWN of where the hovered slice sits, not a readout of the slice
 * alone — Metabase's `getTooltipModel` (visualizations/PieChart/use-chart-events.ts).
 *
 *   header : at ring 0, the dimension's own name; deeper, the ancestor names joined with " > "
 *   rows   : the hovered slice's SIBLINGS — each with its value and its share of their common
 *            parent — with the hovered one marked; a colour marker only at ring 0, where a slice's
 *            colour is unambiguous (an outer ring's "Widgets" is a different shade under every
 *            parent, so a swatch there would claim a colour it doesn't have)
 *   footer : a Total row, only when there is more than one sibling to add up
 *
 * Pointing at an "Other" bucket lists its CHILDREN instead: "Other" is not a category you can
 * compare against its neighbours, it is a question about what went into it.
 *
 * That sibling framing is the whole point. `Organic: 743 (33.73%)` tells you what you already
 * clicked on; the table tells you what it is 33.73% OF, and how the alternatives compare.
 *
 * The pie gets its own formatter rather than extending the shared `itemTooltip` in base.ts,
 * because that one is what funnel, map, boxplot and sankey draw and none of them has a sibling
 * structure to break down.
 */
function pieTooltip(
  roots: PieSliceNode[],
  cfg: PieBuildConfig,
  fmt: (v: unknown) => string,
  pct: (p: number) => string,
) {
  const dimension = cfg.dimensions?.[0];
  const headerAtRoot = dimension ? (cfg.columnSettings?.[dimension]?.column_title || dimension) : "";
  const borderColor = resolveColor("var(--border)", COLOR_FALLBACK.gridLine);
  // The three emphasis levels this table is read in — see TOOLTIP_INK_WEIGHT in base.ts. Every cell
  // states its own, rather than leaving the value's to be inherited from the container's textStyle:
  // the whole point is a RAMP, and a ramp with one step written somewhere else is a ramp nobody can
  // check in one place.
  const ink = tooltipInk();

  return {
    trigger: "item" as const,
    ...tooltipSurface(),
    formatter: (p: { treePathInfo?: { name?: string }[] }) => {
      // The root-to-node path, minus ECharts' own virtual root (entry 0) — the same shape the
      // drill-through click reads. Without it there is nothing to locate the slice by.
      const path = (p.treePathInfo ?? []).slice(1).map((n) => n.name ?? "");
      if (path.length === 0) return "";

      // Walk the tree the option was built from. `ancestors` ends up as the chain down to and
      // including the hovered node — Metabase's `nodes`.
      const ancestors: PieSliceNode[] = [];
      let level = roots;
      for (const name of path) {
        const node = level.find((n) => n.name === name);
        if (!node) return "";
        ancestors.push(node);
        level = node.children ?? [];
      }
      const node = ancestors[ancestors.length - 1];
      const parent = ancestors[ancestors.length - 2];
      const siblings = parent ? (parent.children ?? []) : roots;
      // Pointing at "Other" asks what is IN it, so its children answer instead of its siblings.
      // Unless it has none: at the outermost ring the slices it folded away are not in the tree at
      // all (they are not drawn), and an empty table says less than the sibling comparison does.
      const rows = node.isOther && node.children?.length ? node.children : siblings;
      if (rows.length === 0) return "";

      const atRoot = ancestors.length === 1;
      const header = atRoot ? headerAtRoot : ancestors.slice(0, -1).map((n) => n.name).join("  >  ");
      const cell = "padding:1px 0 1px 8px;text-align:right;white-space:nowrap";
      // The number cells carry the semibold; the name cell is left on the row's own weight, which
      // is also what the focused row raises. So a focused row goes bold throughout without ever
      // inverting the ramp inside it.
      const numberWeight = (focused: boolean) => (focused ? TOOLTIP_WEIGHT.focused : TOOLTIP_WEIGHT.value);

      const body = rows.map((r) => {
        const focused = !node.isOther && r === node;
        // A marker only where a colour means one thing. Below ring 0 the same name is drawn in a
        // different shade under every parent.
        const marker = atRoot
          ? `<span style="display:inline-block;width:9px;height:9px;border-radius:2px;margin-right:6px;background:${escapeHtml(r.itemStyle.color)}"></span>`
          : "";
        return `<tr style="font-weight:${focused ? TOOLTIP_WEIGHT.focused : TOOLTIP_WEIGHT.name}">`
          + `<td style="padding:1px 0;white-space:nowrap;color:${ink.name}">${marker}${escapeHtml(r.name)}</td>`
          // `rawValue`, never `value`: a floored slice is DRAWN bigger than it is, and the table
          // that tells you what it is worth is the last place that should repeat the exaggeration.
          + `<td style="${cell};font-weight:${numberWeight(focused)};color:${ink.value}">${escapeHtml(fmt(r.rawValue))}</td>`
          + `<td style="${cell};font-weight:${numberWeight(focused)};color:${ink.muted}">${escapeHtml(pct(r.slicePercent))}</td>`
          + "</tr>";
      }).join("");

      // Only worth a total when there is more than one thing to add up — Metabase's own rule.
      const footer = rows.length > 1
        ? `<tr style="border-top:1px solid ${borderColor}">`
          + `<td style="padding:3px 0 1px;white-space:nowrap;color:${ink.name}">Total</td>`
          + `<td style="${cell};padding-top:3px;font-weight:${TOOLTIP_WEIGHT.value};color:${ink.value}">${escapeHtml(fmt(rows.reduce((t, r) => t + r.rawValue, 0)))}</td>`
          + `<td style="${cell};padding-top:3px;font-weight:${TOOLTIP_WEIGHT.value};color:${ink.muted}">100%</td>`
          + "</tr>"
        : "";

      // The header NAMES the comparison; it is not part of it. It used to be the boldest thing in
      // the tooltip (700, at the row size) — the most emphasis on the least information. Metabase
      // runs it the other way: a size smaller than the rows, regular weight, and the dim ink.
      const headerStyle = `font-size:${TOOLTIP_HEADER_FONT_SIZE}px;font-weight:${TOOLTIP_WEIGHT.header};color:${ink.muted};margin-bottom:3px`;
      return (header ? `<div style="${headerStyle}">${escapeHtml(header)}</div>` : "")
        + `<table style="border-collapse:collapse"><tbody>${body}${footer}</tbody></table>`;
    },
  };
}

export function buildPieOption(
  columns: ResultColumn[],
  rows: unknown[][],
  cfg: PieBuildConfig,
  // Injectable text measurer — defaults to a real canvas measurer (see sliceLabel.ts); tests
  // pass a deterministic stub instead of depending on a real text rasteriser (see that file's
  // own header comment on why this has to be threaded through as a plain callback here rather
  // than a renderingContext object like Metabase's).
  measureText: MeasureText = defaultMeasureText,
  // The chart's REAL rendered pixel size — and, since the legend left the canvas, the size of the
  // box the CIRCLE alone gets, with any legend column or row already taken out of it. This builder
  // is pure data with no DOM access of its own, so it can't measure its own container;
  // EChartsRenderer measures it (ResizeObserver) and threads it through by calling this function
  // itself rather than a plain option object (see that file). Undefined (no caller-supplied size —
  // most unit tests that aren't testing label fit at all) means "unknown": the radii stay in
  // ECharts' percent units and every ring's pixel radius is 0, which takes the SAME "true full pie"
  // path this file already uses when there's genuinely no donut hole (see the comment on
  // `availableSpace` further down) — the per-slice fit check is skipped entirely and every label is
  // shown in full, backstopped only by `labelLayout.hideOverlap`. That's the safe direction for "we
  // don't know the real size": it never falsely truncates against a guessed number that could just
  // as easily be wrong, the same reasoning that ruled out NOMINAL_CONTAINER_MIN_DIMENSION_PX in the
  // first place.
  containerSize?: { width: number; height: number },
): EChartsCoreOption {
  const dims = cfg.dimensions ?? [];
  const { rings: effectiveRings, groupOfByLevel } = buildPieRings(columns, rows, cfg);

  const dimension = dims[0];
  const data = effectiveRings[0];
  const ringCount = effectiveRings.length;
  const multiRing = ringCount > 1;

  const fmt = metricFormatter(cfg);

  // Label: name (showLabels) and/or percent (percentVisibility) — Metabase getSliceLabel. Applied
  // the same way to every ring. Defaults resolve here, at render time, never in stored settings
  // (this program's standing rule) — `??` so an explicit `false` stays off; `||` would treat a
  // saved `false` the same as "unset" and never let anyone turn labels off.
  const showName = cfg.showLabels ?? true;
  // "both" means what its name says and what Metabase means by it: the legend AND the slice.
  // Before "legend" existed here, a stored "both" could only ever draw the inside half — the
  // value promised something the renderer had no way to deliver.
  const percentVisibility = resolvePercentVisibility(cfg.percentVisibility, ringCount);
  const showPercent = percentVisibility === "inside" || percentVisibility === "both";

  // Slice label sizing — Metabase's constants.ts: multiRingFontSize (12) once there's more than
  // one ring, and Metabase's FLOOR (minFontSize, 14) for a single one. Metabase scales a
  // single-ring pie's label between 14 and 20 with the rendered side length; egret deliberately
  // does not, even though it now measures that side length (see `diameterPx` below). The floor
  // never renders LARGER than Metabase would at any size, which is the safe direction — a label
  // too big is what causes overlap and hiding — and growing a saved one-ring pie's labels is a
  // separate change of appearance from this one. fontWeight 700 either way
  // (slice.label.fontWeight) and drawn ON the slice, matching what makes Metabase's sunburst
  // readable without a legend.
  const sliceFontSize = multiRing ? 12 : 14;

  // ECharts has no built-in shrink-or-truncate for a label that doesn't fit its slice (unlike
  // Metabase's sunburst, which measures text against the slice's own arc length and truncates or
  // drops it). What it DOES have is `labelLayout.hideOverlap`: a post-layout pass that hides
  // whichever labels collide, keeping the rest legible — the same graceful-degradation Metabase
  // reaches for (`labelLayout: { hideOverlap: true }` in its own option.ts). A tiny slice's label
  // disappears instead of overlapping into mush; it is still reachable via the tooltip.
  const labelLayout = { hideOverlap: true };

  // Geometry and the hole are both computed by the shared helpers above (pieGeometry /
  // buildPieHoleGraphic) rather than inline here, because the hole has a SECOND caller now: React
  // re-states it on its own when the pointer moves onto a slice, and re-deriving the inner radius
  // or the text styling beside the option is how the two would drift apart.
  const { diameterPx, outerRadiusPx, innerRadiusPx, radius, center } =
    pieGeometry(cfg, ringCount, containerSize);

  // The rest state — no readout — is the same call the hover path makes with one.
  const hole = pieHoleModel(effectiveRings, cfg, fmt);
  const graphic = buildPieHoleGraphic(hole, cfg, null, containerSize, measureText);

  // Metabase's DIMENSIONS.slice.label.padding (pie/constants.ts) — a small margin subtracted
  // from the geometric space a label is allowed, so text doesn't butt right up against a
  // slice's own border.
  const SLICE_LABEL_PADDING = 2;

  // Multi-ring labels run RADIAL (along the radius), not horizontal — fix 1a. A horizontal
  // label needs a wide chord to sit in; a radial one only needs to fit along the ring's own
  // width, which is what makes a narrow multi-ring slice able to hold a label at all. A single
  // ring keeps the previous horizontal behaviour untouched (Metabase's own rule:
  // `chartModel.numRings > 1 ? "radial" : "horizontal"`).
  const labelsPosition = multiRing ? ("radial" as const) : ("horizontal" as const);

  // The two candidate label colours — WCAG AA contrast (fix 2): resolved once, not per slice,
  // since resolveColor's own cache would otherwise do this same var() lookup on every slice.
  // Reads the theme's own tokens (light theme: dark-on-light; dark theme: light-on-dark), same
  // as everything else in this file that reads a CSS custom property — but only because
  // resolveColor's cache is dropped on a theme change and EChartsRenderer then re-applies the
  // option. Neither is true "by construction": without both, these two resolve exactly once per
  // page load and a light<->dark toggle leaves every label on the previous theme's ink.
  const labelTextDark = resolveColor("var(--foreground)", COLOR_FALLBACK.text);
  const labelTextLight = resolveColor("var(--background)", COLOR_FALLBACK.textInverse);

  // The border between slices — Metabase's `getBorderWidth`. NO ANGULAR GAP: this used to set
  // ECharts' `padAngle: 1`, on a reading of Metabase's `borderProportion: 360 // 1 degree` as an
  // angular pad. It isn't one; it is a stroke WIDTH, `(π × innerSideLength) / 360`, i.e. the arc
  // length one degree subtends at the pie's own radius. The difference is not cosmetic:
  //  - a stroke straddles the boundary and NEIGHBOURING SLICES SHARE IT, so the fills still touch;
  //    an angular pad removes real area from every slice, so it under-reports its own value;
  //  - a pad is constant in ANGLE, so the gap it opens GROWS WITH RADIUS — which is what produced
  //    36 visible gaps on the outer ring, median 3.96px, against Metabase's 0.
  // Multi-ring is a flat 1 (Metabase's own `twoRingBorderWidth: 1` and `threeRingBorderWidth: 0.3`
  // constants are dead in its source — `getBorderWidth` reads neither, and returns 1 for every
  // ring count above one). Coloured to match the chart's own background (the card a chart renders
  // inside, per shadcn's Card `bg-card`) rather than Metabase's hardcoded white, so the separator
  // disappears into dark mode too.
  const borderWidth = multiRing ? 1 : (Math.PI * diameterPx) / 360;
  const borderColor = resolveColor("var(--card)", COLOR_FALLBACK.card);

  // The legend toggle, applied HERE and nowhere earlier. `cfg.hiddenSlices` is the saved curation,
  // applied before a ring is even aggregated precisely so that every total and threshold re-bases;
  // this is the ephemeral click, applied to the finished rings. What it now takes with it CHANGED
  // with the series type, and deliberately: as N independent `pie` series, dropping a ring-0 slice
  // left its children drawn as orphan arcs in the outer rings, sitting under nothing. A tree cannot
  // express that — a node's children go with it — so hiding a slice hides its whole family, which
  // is what the reader was asking for anyway. The hole's total and the legend's own percentages
  // still do NOT re-base (both are computed from the full rings, above), so the remaining wedges
  // stay comparable with the numbers beside them.
  const rootSlices = pieRootSlices(effectiveRings[0], cfg);

  // Which finalized parent-ring slice each of a ring's slices belongs to, one map per parent level.
  const childrenByParentKey = pieChildrenByParent(effectiveRings, groupOfByLevel);

  // Angles. ECharts' sunburstLayout gives every node the SAME scale — `unitRadian = 2π / rootSum`,
  // where rootSum is the sum of the top-level nodes — and lays a node's children out consecutively
  // starting at that node's own start angle. So an outer slice's arc is its share of the WHOLE
  // chart, nested inside its parent's arc, which is exactly the alignment N independent pie series
  // could not express: each of those angled its ring by that ring's own total, so a child ring's
  // wedges lined up with their parents' only by coincidence.
  //
  // Reproduced here (rather than read back off the chart) because the label fit below has to know
  // a slice's arc before anything renders. It starts at 12 o'clock and runs clockwise because that
  // is ECharts' default (`startAngle: 90, clockwise: true`) and this file never says otherwise —
  // the convention calcAvailableDonutSliceLabelLength's geometry assumes, and a test pins that both
  // stay unset so the model can't silently rot.
  const rootTotal = rootSlices.reduce((t, s) => t + s.value, 0);

  // THE TINY-SLICE FLOOR (see MIN_DRAWN_SLICE_FRACTION). Ring 0's drawn size, per slice — its own
  // value, or the floor if it is under it. `rootTotal` stays the TRUE total: it is the denominator
  // every reported percentage is taken against, and inflating it would move numbers on slices the
  // floor never touched.
  const drawnValueOf = (s: Slice) => Math.max(s.value, rootTotal * MIN_DRAWN_SLICE_FRACTION);
  // ...and the sum of those, which is what the turn gets divided by — ECharts' own
  // `unitRadian = 2π / rootSum`, reproduced here because the label fit below needs a slice's arc
  // before anything renders. Dividing by the inflated sum is what keeps the circle closed: the
  // extra a floored slice takes comes off every other slice proportionally, rather than off the
  // end of the turn.
  const drawnTotal = rootSlices.reduce((t, s) => t + drawnValueOf(s), 0);
  const unitRadian = drawnTotal > 0 ? (2 * Math.PI) / drawnTotal : 0;

  // How precise the percentages DRAWN ON THE CHART are — the slice labels, the tooltip table that
  // shares their formatter, and the breakdown table beside the chart, which all take the SAME
  // answer from `pieSharePrecision` (see it for the rule). Resolved here rather than beside
  // `showPercent` above because it needs the finished tree: the rings, the legend toggle, and each
  // ring's grouping under its parent.
  const decimals = pieSharePrecision(rootSlices, rootTotal, childrenByParentKey, cfg);
  const pct = pctFormatter(decimals);
  // The full, untruncated text a slice's label wants to say — Metabase's getSliceLabel. Called
  // directly with the slice's own name and share rather than through an ECharts callback: the fit
  // pass below has to MEASURE this text before anything renders, and a `sunburst` datum has no
  // `percent` of its own to hand a callback anyway (see `slicePercent`).
  const composeSliceLabel = (name: string, percent: number) => {
    const namePart = showName ? name : undefined;
    const percentPart = showPercent ? pct(percent) : undefined;
    if (namePart != null && percentPart != null) return `${namePart}: ${percentPart}`;
    return namePart ?? percentPart ?? "";
  };

  const fontStyle = { size: sliceFontSize, weight: 700 };

  /**
   * One slice, as a sunburst node — its own subtree included.
   *
   * `drawnScale` is how much bigger than itself this subtree is drawn: 1 everywhere except under a
   * ring-0 slice the floor lifted, where it is that slice's inflation factor. It has to reach the
   * WHOLE subtree, not just the floored slice, because ECharts lays a node's children out with the
   * same global `unitRadian` and does NOT renormalise them into their parent's arc — children
   * carrying their true values under an inflated parent would fill only a fraction of it and leave
   * the rest of the wedge blank, which reads as missing data rather than as a magnified slice.
   * Scaling the subtree uniformly keeps every nested proportion exactly as it was.
   */
  function toNode(s: Slice, level: number, startAngle: number, parentValue: number, drawnScale: number): PieSliceNode {
    const drawnValue = s.value * drawnScale;
    const endAngle = startAngle + drawnValue * unitRadian;
    const { inner: ringInnerPx, outer: ringOuterPx } = calcInnerOuterRadiusesForRing(
      innerRadiusPx,
      outerRadiusPx,
      ringCount,
      level + 1,
    );

    // A slice's share of its own parent — which at ring 0 is the whole chart, so ring 0's share of
    // the chart is unchanged. REVERSED from an earlier decision that made every ring a share of
    // ITS RING's total: on a real three-ring chart, a channel slice worth 500 out of its own
    // (region, product) parent's 1000 read as "500/12000 = 4.17% of everything" — arithmetically
    // true, and useless to whoever is looking at that one wedge. Metabase computes the same
    // parent-relative number (`normalizedPercentage`).
    //
    // Carried on the datum as `slicePercent` because a `sunburst` series has no percentage of its
    // own AT ALL: only `PieSeriesModel.getDataParams` adds `percent` (and only it honours
    // `percentPrecision`, which this option therefore no longer sets — a sunburst ignores it). Both
    // the label below and the shared tooltip (base.ts' itemTooltip) read this instead.
    const slicePercent = parseFloat(
      ((parentValue > 0 ? s.value / parentValue : 0) * 100).toFixed(decimals),
    );

    // Each label is measured against ITS OWN slice's geometry and truncated or dropped, rather
    // than relying solely on labelLayout.hideOverlap's post-layout backstop.
    //
    // calcAvailableDonutSliceLabelLength is, as its name says, a DONUT formula — with a true zero
    // inner radius (egret's "one ring, donut off" case) its inner chord is degenerate (a single
    // point), which would collapse every slice's space to 0 and drop every label outright.
    // Metabase never hits this because its own ratios always leave some hole, even at one ring. So
    // a true full pie skips the fit check entirely and keeps showing full labels, with hideOverlap
    // as its sole backstop. An unknown `containerSize` takes the same path, for the same reason it
    // always has: "we don't know the real size" must not become "guess small", which is what a
    // fixed 300px reference container once did — truncating "LATAM" to "L…" in a real ~530px card.
    const availableSpace =
      ringInnerPx > 0
        ? calcAvailableDonutSliceLabelLength(ringInnerPx, ringOuterPx, startAngle, endAngle, sliceFontSize, labelsPosition) -
          2 * SLICE_LABEL_PADDING
        : Infinity;
    const displayLabel = fitSliceLabel({
      label: composeSliceLabel(s.name, slicePercent),
      availableSpace,
      showLabels: showName,
      measureText,
      fontStyle,
    });

    // This slice's OWN render colour. Every slice states one (buildPieRings pins ring 0's palette
    // slots and lightens every ring below it), so the label's contrast is judged against the fill
    // that will actually be drawn — the reason egret has 0 labels below WCAG AA where Metabase's
    // own sunburst has 27.
    const color = s.itemStyle?.color ?? resolveColor(OTHER_SLICE_COLOR, COLOR_FALLBACK.axisLabel);
    const labelColor = getTextColorForBackground(color, labelTextDark, labelTextLight);
    // ...and, on the two fills where the better ink still cannot reach 4.5:1 (#7172AD in either
    // theme, #227FD2 in light), a thin outline in the opposite ink so the glyphs separate from the
    // fill anyway. `undefined` — i.e. nothing spread into `label` at all — on every other fill;
    // see `getTextHaloForBackground` for why the palette is not the thing being changed here.
    const labelHalo = getTextHaloForBackground(color, labelTextDark, labelTextLight);

    const children = childrenByParentKey[level]?.get(s.key ?? OTHER_KEY);
    let childStart = startAngle;

    return {
      name: s.name,
      value: drawnValue,
      rawValue: s.value,
      slicePercent,
      isOther: s.key != null && s.key.startsWith(OTHER_KEY),
      itemStyle: { color, borderWidth },
      label: {
        color: labelColor,
        ...labelHalo,
        // Metabase's own sentinel for "nothing to show" (option.ts ~L269): a single space.
        //
        // Still REQUIRED even though a dropped label is a proper `null`, and the null cannot simply
        // be passed through: SunburstPiece._updateLabel does `text = text || this.node.name`, so a
        // formatter returning null/""/undefined resurrects the very label the fit decision just
        // dropped. A space is truthy, so it wins — and a space rather than "" because an empty
        // string is indistinguishable from a slice whose dimension value is genuinely blank, which
        // is the whole point of fitSliceLabel's blank-label guard.
        formatter: () => displayLabel ?? " ",
        // Multi-ring labels run along the radius rather than horizontally — a radial label fits a
        // narrow arc a horizontal one can't, which is what lets a thin outer ring hold a label at
        // all. Per datum, like Metabase, since the series-level default would apply to every ring.
        rotate: labelsPosition === "horizontal" ? 0 : ("radial" as const),
        verticalAlign: "middle" as const,
      },
      // Hovering a slice focuses its ANCESTORS (`emphasis.focus: "ancestor"` on the series) and
      // blurs everything else. This keeps the hovered slice's own fill and border rather than
      // letting ECharts lighten it.
      emphasis: { itemStyle: { color, borderColor } },
      blur: {
        itemStyle: {
          // Faded through the COLOUR, not through `opacity`. ECharts applies `itemStyle.opacity` to
          // the border as well, so the background-coloured separator would go translucent and the
          // fill underneath would leak through it. Fading only the fill leaves the border opaque.
          color: fadeColor(color, 0.3),
          opacity: 1,
        },
        // A label in the theme's normal ink is still legible over a 30%-opacity fill, so it fades
        // with the slice; one in the INVERSE ink is only readable because its fill is dark, and
        // that fill is what just went pale — fading it too would leave nothing at all. (Metabase
        // makes the same split on its own text-primary/text-white pair.)
        label: { opacity: labelColor === labelTextDark ? 0.3 : 1 },
      },
      ...(children?.length
        ? {
            children: children.map((child) => {
              const node = toNode(child, level + 1, childStart, s.value, drawnScale);
              childStart += child.value * drawnScale * unitRadian;
              return node;
            }),
          }
        : {}),
    };
  }

  let rootStart = 0;
  const seriesData = rootSlices.map((s) => {
    const drawnValue = drawnValueOf(s);
    // `s.value > 0` always holds here (buildSlices drops zeros and negatives), so this only guards
    // against a future caller reaching this with one.
    const node = toNode(s, 0, rootStart, rootTotal, s.value > 0 ? drawnValue / s.value : 1);
    rootStart += drawnValue * unitRadian;
    return node;
  });

  // ONE series, of type `sunburst`, exactly like Metabase's — the point of this whole change.
  // N independent `pie` series could not express ancestry at all, which is why hovering an outer
  // slice used to change 3 of 55 wedges where Metabase changes 51 of 53.
  //
  // Written as a one-element ARRAY rather than Metabase's bare object: ECharts accepts either and
  // normalises them to the same thing, and an array is what every other builder in this directory
  // produces, so "how many series does this chart have" stays one question with one answer.
  const series = [
    {
      type: "sunburst" as const,
      // ECharts' sunburst sorts children by value DESCENDING by default, which would silently undo
      // `sortRows: false` and `sliceOrder`. `undefined` (the key present, the value absent) is what
      // suppresses it: zrender's `merge` skips a key that is already `in` the target, so the
      // default never lands. Metabase writes the same line for the same reason.
      sort: undefined,
      // Clicking a wedge must not zoom the chart into it (`nodeClick: 'rootToNode'` is the
      // default) — a click is egret's drill-through gesture, handled in ChartRenderer.
      nodeClick: false as const,
      radius,
      center,
      itemStyle: { borderColor },
      label: {
        show: showName || showPercent,
        position: "inside" as const,
        // Every label was already measured against its own slice, so ECharts must not then also
        // truncate or wrap it against the bounding box it computes for a rotated label.
        overflow: "none" as const,
        fontSize: sliceFontSize,
        fontWeight: 700,
      },
      labelLayout,
      emphasis: { focus: "ancestor" as const },
      data: seriesData,
    },
  ];

  // A screen-reader user has no other way to notice a ring quietly drawing less (or more) than the
  // chart's stated total — see `pieDivergenceNotes`. Carried by the breakdown table as well.
  const divergenceNotes = pieDivergenceNotes(effectiveRings, dims, fmt);

  // WHAT THIS DESCRIPTION IS, AND WHAT IT IS NOT. It reads out ring 0 — the slices whose shares are
  // shares of the whole chart — and nothing deeper, because a sunburst past two levels encodes
  // meaning in COLOUR (family + lightened shade) and POSITION (a nested arc under one specific
  // parent wedge) that no flat sentence can recover. That used to be a flagged gap; the rest of the
  // chart is now `buildPieBreakdown` + PieBreakdown.tsx, a real table of every slice under its own
  // parent with its share OF that parent, rendered beside the chart whenever there is more than one
  // ring.
  //
  // This description deliberately STAYS as it is, rather than standing down in favour of that
  // table. The two are not the same reading: this is what a screen reader announces on reaching the
  // chart itself — a one-breath summary of the level whose slices are shares of the whole — and the
  // table is a structure to be navigated when that summary raises a question. Replacing it with a
  // pointer would leave the chart element itself saying nothing, which is worse than saying the
  // innermost ring; and a single-ring pie, whose table is deliberately empty (see
  // `buildPieBreakdown`), would then have no description at all.
  const baseAria = ariaDescription({
    kind: "Pie chart", metric: cfg.metric, dimension, format: fmt,
    points: data.map((d) => ({ name: String(d.name), value: d.value })),
  });
  const aria = divergenceNotes.length > 0
    ? { ...baseAria, label: { description: `${baseAria.label.description} ${divergenceNotes.join(" ")}` } }
    : baseAria;

  return {
    ...baseOption(),
    // ALL animation off, not `animationDuration: 0` — Metabase's own note: on a sunburst the
    // duration alone does not stop the initial-render animation. It matters more here than it does
    // there: this option is rebuilt on every resize tick (see EChartsRenderer), and each rebuild
    // would otherwise replay a 1s expansion.
    animation: false,
    aria,
    tooltip: pieTooltip(seriesData, cfg, fmt, pct),
    // NO `legend` KEY, deliberately: the pie's legend is React markup beside the canvas
    // (buildPieLegend + PieLegend.tsx). ECharts' own legend component does not work with a
    // `sunburst` series, which is what this pie now is.
    graphic,
    series,
  };
}
