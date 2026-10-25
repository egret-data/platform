import type { ColumnFormat } from "../../format-types";
import { formatValue, formatDimensionValue } from "../../format";

export interface ResultColumn { name: string; type: string }
export interface FunnelBuildConfig {
  dimension?: string;
  metric?: string;
  /** Step names in display order; any not listed keep their natural place after them. Metabase's
   *  `funnel.rows`, which is also where a step gets hidden. */
  stepOrder?: string[];
  /** Step names left out of the funnel entirely. */
  hiddenSteps?: string[];
  columnSettings?: Record<string, ColumnFormat>;
  metricTypes?: Record<string, string>;
}

export interface FunnelStep {
  /** The step's name AS DRAWN — the dimension cell run through that column's own stored format.
   *  See `raw` for the value it came from, which is not the same string once a format is set. */
  name: string;
  /**
   * The dimension cell as it came out of the query, before its column format.
   *
   * THE IDENTITY, and the reason this field exists at all. `stepOrder` and `hiddenSteps` are stored
   * by `FunnelStepsControl`, which derives its list from `String(row[dimensionIndex])` — raw — so
   * ranking and hiding have to be keyed on the raw cell or a chart whose steps were ordered by hand
   * would lose that order the moment someone chose a date style. Callers that need to identify a
   * step (`FunnelChart`'s bar mode keys its per-step lookups on it) use this; only what a reader
   * SEES uses `name`.
   */
  raw: string;
  value: number;
  /** Formatted through the metric's own column settings. */
  label: string;
  /** Share of the FIRST step, already rounded the way Metabase prints it ("85.39 %"). Empty on the
   *  first step, which is the thing everything else is a share of. */
  share: string;
  /** Height of this step's edge as a fraction of the tallest, which is how Metabase sizes the
   *  trapezoids: its polygons run from `(1 - v/max)/2` to `1 - (1 - v/max)/2` in a unit viewBox,
   *  and `3976/5061 = 0.78562` is exactly the edge height it drew for that step. */
  fraction: number;
  /** The result row this step came from, so a renderer that wants the whole row — the bar mode
   *  builds a real bar chart out of them — gets the ordering and the hiding for free. */
  row: unknown[];
}

/**
 * The steps a funnel draws, in the order it draws them.
 *
 * ROW ORDER, not value order. A funnel's steps are a SEQUENCE — visit, sign up, pay — and sorting
 * them by size rearranges that sequence into a story the data never told: measured against
 * Metabase on four categories whose values do not descend, it keeps them in row order while egret
 * reordered them.
 */
export function funnelSteps(columns: ResultColumn[], rows: unknown[][], cfg: FunnelBuildConfig): FunnelStep[] {
  const idx = Object.fromEntries(columns.map((c, i) => [c.name, i]));
  const di = cfg.dimension != null ? idx[cfg.dimension] : undefined;
  const mi = cfg.metric != null ? idx[cfg.metric] : undefined;
  if (di == null || mi == null) return [];

  const hidden = new Set(cfg.hiddenSteps ?? []);
  const rank = new Map((cfg.stepOrder ?? []).map((n, i) => [n, i]));
  const raw = rows
    .map((r) => ({ raw: String(r[di] ?? ""), value: Number(r[mi]), row: r }))
    // A step worth ZERO is kept, as Metabase keeps it: "nobody got this far" is the single most
    // important thing a funnel can say, and dropping the row silently shortens the sequence.
    //
    // HIDING AND RANKING ARE KEYED ON `raw`, NOT ON THE DRAWN NAME. `FunnelStepsControl` builds the
    // panel's step list out of `String(row[dimensionIndex])`, so that is what `stepOrder` and
    // `hiddenSteps` hold; matching them against a formatted name would silently drop a hand-made
    // order the first time anyone set a date style. See `FunnelStep.raw`.
    .filter((s) => Number.isFinite(s.value) && !hidden.has(s.raw))
    // A stored order names the steps it knows; anything new keeps its natural place after them,
    // the rule `applySeriesOrder` and the pie's `sliceOrder` already use.
    .sort((a, b) => (rank.get(a.raw) ?? Number.MAX_SAFE_INTEGER) - (rank.get(b.raw) ?? Number.MAX_SAFE_INTEGER));

  const fmt = (v: unknown) =>
    cfg.metric ? formatValue(v, cfg.metricTypes?.[cfg.metric] ?? "double", cfg.columnSettings?.[cfg.metric]) : String(v);

  /**
   * THE STEP NAME IS A VALUE AND IT HAS A FORMAT — the half of this builder that was missing.
   *
   * `styleColumnsFor` offers the funnel's DIMENSION a Formatting accordion (it fell through to
   * `return resultColumns`), and this function read `columnSettings` for the metric only, so a Date
   * style typed there was stored and drawn nowhere. Measured in the browser over `MB Monthly`: the
   * panel read `Date style: January 2024` while all twelve steps read `2026-01-01`.
   *
   * Metabase formats it — `FunnelNormal.tsx`'s `formatDimension` spreads
   * `settings.column?.(cols[dimensionIndex])` into `formatValue` for every step header — so this is
   * a copied rule, not an invented one. `formatDimensionValue` carries the guard that stops a plain
   * RENAME from restyling every step; see its comment, which is where the trap is written down.
   */
  const dimType = columns[di]?.type ?? "varchar";
  const dimFormat = cfg.dimension != null ? cfg.columnSettings?.[cfg.dimension] : undefined;

  const first = raw[0]?.value;
  const max = raw.reduce((m, s) => Math.max(m, s.value), 0);
  return raw.map((s, i) => ({
    name: formatDimensionValue(s.raw, cfg.dimension ?? "", dimType, dimFormat),
    raw: s.raw,
    value: s.value,
    label: fmt(s.value),
    share: i > 0 && first ? `${((s.value / first) * 100).toFixed(2)} %` : "",
    fraction: max > 0 ? s.value / max : 0,
    row: s.row,
  }));
}

/**
 * How opaque the band leaving step `index` is, out of `bandCount` bands — Metabase's
 * `calculateStepOpacity`, copied exactly: `1 - index * (0.9 / (bandCount + 1))`.
 *
 * This ramp is the whole reason a Metabase funnel reads as a sequence rather than one blue shape.
 * Every band is the SAME brand colour; only its opacity falls, so the eye follows the flow left to
 * right without being told the steps are different categories. Measured on the live chart with
 * three bands: 1, 0.775, 0.55.
 */
export function stepOpacity(index: number, bandCount: number): number {
  return 1 - index * (0.9 / (bandCount + 1));
}
