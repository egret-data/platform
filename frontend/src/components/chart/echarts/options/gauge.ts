import type { EChartsCoreOption } from "echarts/core";
import type { GaugeSegment } from "@/api/charts";
import { measureText, type TextMeasurer } from "@/components/chart/measure-text";
import type { EChartsSize } from "../EChartsRenderer";
import type { ColumnFormat } from "../../format-types";
import { formatValue } from "../../format";
import { HORIZONTAL_TICKS_GAP } from "../axisLabelFit";
import { baseOption, itemTooltip } from "../base";
import { ARIA_A11Y, resolveFontFamily } from "../palette";
import { SERIES_PALETTE, COLOR_FALLBACK, resolveColor } from "../palette";

export interface ResultColumn { name: string; type: string }
export interface GaugeBuildConfig {
  metric?: string;
  /** Coloured ranges, in any order. The gauge's scale is derived from them. */
  segments?: GaugeSegment[];
  columnSettings?: Record<string, ColumnFormat>;
  metricTypes?: Record<string, string>;
}

const ZONE_LOW = SERIES_PALETTE[3];  // red
const ZONE_MID = SERIES_PALETTE[4];  // yellow
const ZONE_HIGH = SERIES_PALETTE[1]; // green

/**
 * Today's three zones, derived rather than stored so an unconfigured gauge is unchanged.
 * Exported so the segment editor (task 4) can show exactly these three rows when nothing
 * is stored yet, instead of re-deriving the thirds-of-[0, value*2] numbers a second time.
 */
export function defaultGaugeSegments(value: number): GaugeSegment[] {
  // Metabase's own default (`getDefaultGaugeSegments`): the arc runs 0 → 2v with boundaries at
  // v/2 and v, so the MIDDLE boundary is the value itself and the needle lands on it — the reader
  // can see where they are without reading a number.
  //
  // Equal thirds of 0 → 2v, which this used to be, put the needle at an arbitrary point AND made
  // the boundaries fractional: a value of 116,269 filled the editor's inputs with
  // `77512.66666666667`, eleven decimals of noise the user had to clear by hand before the panel
  // read as anything but broken.
  const max = value > 0 ? value * 2 : 100;
  const half = max / 4;   // = v/2
  const mid = max / 2;    // = v
  return [
    { min: 0, max: roundBound(half), color: ZONE_LOW },
    { min: roundBound(half), max: roundBound(mid), color: ZONE_MID },
    { min: roundBound(mid), max: roundBound(max), color: ZONE_HIGH },
  ];
}

/**
 * A derived bound, kept to something a person would type.
 *
 * Metabase's boundaries come out whole because halving and doubling an integer stays integral;
 * egret's did not, because a third of a value rarely is. Rounding here rather than only in the
 * INPUT matters: the number the editor shows is the number that gets stored the moment anything
 * else on the row is touched, so a display-only fix would have written the noise back anyway.
 * Sub-unit values (a rate of 0.42) keep three decimals instead of collapsing to 0.
 */
function roundBound(n: number): number {
  if (!Number.isFinite(n)) return 0;
  if (Number.isInteger(n)) return n;
  return Math.abs(n) >= 1 ? Math.round(n * 100) / 100 : Math.round(n * 1000) / 1000;
}

/**
 * A segment's bounds come from JSON, where a number is a number — a bound that arrives as a
 * string, `null`, or missing is malformed data, not a value to rescue by coercion (a prior
 * version of this function used `Number(s.min)`, which happily turned `null` into `0` and
 * `"12"` into `12`; both are dropped now). `color` is not checked here — it goes through
 * `resolveColor` unconditionally when building stops, which always returns something the
 * renderer can draw, so a bad colour no longer needs to cost the segment its bounds.
 */
function isValidSegment(s: GaugeSegment | null | undefined): s is GaugeSegment {
  if (s == null) return false;
  return typeof s.min === "number" && typeof s.max === "number"
    && Number.isFinite(s.min) && Number.isFinite(s.max) && s.min <= s.max;
}

/** Sanitised segments, sorted low to high by their upper bound — or today's three zones if
 * nothing usable survives (an empty array or one where every entry is invalid). */
function sanitizeSegments(value: number, segments: GaugeSegment[] | undefined): GaugeSegment[] {
  const valid = (segments ?? []).filter(isValidSegment).sort((a, b) => a.max - b.max);
  return valid.length > 0 ? valid : defaultGaugeSegments(value);
}

// ---------------------------------------------------------------------------------------------
// HOW MANY GRADUATIONS THE ARC CAN CARRY.
//
// THE DEFECT. This builder set no `splitNumber`, so every gauge got ECharts' default of 10 intervals
// — ELEVEN numbers round the arc at every size. On the 166x184 ZZ Chart Parity card that drew
// `0 76,612.39 153,224.78 229,837.17 306,449.56 383,061.95 459,674.34 536,286.73 612,899.12
// 689,511.51 766,123.9`, with `229,837.17` spanning x 65-122 and `306,449.56` spanning x 73-135 on
// the same line: two labels, 49px of shared ink.
//
// WHAT METABASE DOES, since it is the parity target and its answer is stronger than "fewer".
// `visualizations/Gauge/Gauge.tsx` does not graduate the arc at all — it labels the SEGMENT
// BOUNDARIES (`range` plus each segment's min and max, deduped), so an unconfigured gauge carries
// four numbers however wide it is. And below `MIN_WIDTH_LABEL_THRESHOLD` (250px, `constants.ts`) it
// draws `showLabels = false` and puts NO number on the arc, leaving the coloured zones, the needle
// and the centre reading. The harness cannot render its gauge ("Gauge visualization requires a
// number"), so neither of those is copied from a measurement — but both are read off its source, and
// the second is why the ladder below has a `0` rung rather than bottoming out at "the two ends".
//
// WHAT IS NOT PORTED, and deliberately: labelling boundaries instead of even splits. That would move
// which numbers a gauge shows on every card in the product, including the wide ones this defect does
// not touch, and it interacts with the segment editor. This change is about the COUNT.
//
// `splitNumber` IS THE RIGHT LEVER HERE, WHICH IS NOT WHAT THE CARTESIAN AXIS TAUGHT. On a value
// axis it is a hint ECharts rounds to a nice interval, and rounds DOWN — `intervalThatFits`
// (`axisLabelFit.ts`) exists because asking a 250px row chart for 3 splits produced five labels. A
// `series.gauge` is a different code path (`chart/gauge/GaugeView.js`, `_renderTicks`) with no
// `nice()` in it: it walks `i = 0..splitNumber` and formats `min + i * (max - min) / splitNumber`
// verbatim. Rendered through real ECharts in SSR at 166x184, `splitNumber` n drew exactly n + 1
// labels for every n from 1 to 8, with no rounding of the count and none of the values —
// `splitNumber: 3` over 0-766,123.9 draws `255,374.633`, a label WIDER than the maximum it divides.
// `axisLabel.showMinLabel`/`showMaxLabel` are cartesian-only and were ignored at all four
// combinations. `__tests__/gaugeGraduations.echarts.test.ts` pins the mapping rather than trusting
// this paragraph.
// ---------------------------------------------------------------------------------------------

/** The arc this gauge draws: 240 degrees, opening downwards. Hoisted out of the series because the
 *  fit below has to know where a label lands, and a sweep stated twice is a sweep that drifts. */
const START_ANGLE = 210;
const END_ANGLE = -30;
/** `axisLabel.distance` and its font — the two the fit reads, so they cannot be set on the series and
 *  measured differently here. */
const LABEL_DISTANCE = 18;
const LABEL_FONT_SIZE = 11;
/**
 * ECharts' own gauge geometry, read off `chart/gauge/GaugeView.js` and
 * `chart/gauge/GaugeSeries.js` at the version in `node_modules` — the four numbers that put a
 * graduation somewhere on the page, none of them ours to choose and all of them left at their
 * defaults by this builder.
 *
 *   `parsePosition`   r  = radius% of min(width, height) / 2, centred at (width/2, height/2)
 *   `_renderTicks`    label ring = r - splitLine.length - (axisLabel.distance + splitLine.distance)
 *
 * `splitLine: { show: false }` on the series does NOT remove its length from that subtraction —
 * `splitLineLen` is computed before the `show` branch and the label radius uses it either way. So
 * the ring is 20px inside the axis line whatever the split lines do, which is the difference between
 * predicting 26.25px of radius and the 24.25 ECharts actually draws at the live card's size.
 */
const RADIUS_PERCENT = 0.75;      // GaugeSeries defaultOption.radius, "75%"
const SPLIT_LINE_LENGTH = 10;     // GaugeSeries defaultOption.splitLine.length
const SPLIT_LINE_DISTANCE = 10;   // GaugeSeries defaultOption.splitLine.distance
/** `GaugeSeries` defaultOption.splitNumber — the 10 that produced eleven labels. It is the CEILING
 *  the ladder starts at, not a count to write back: a gauge roomy enough for it keeps an option
 *  byte-identical to the one this builder made before the fit existed, because the fit may only ever
 *  take graduations AWAY. (Same rule as `intervalThatFits`, and for the same reason — a chart wide
 *  enough for fifteen labels does not want fifteen.) */
const DEFAULT_SPLIT_NUMBER = 10;

/** ECharts rounds each graduation's VALUE before formatting it (`_renderTicks` calls
 *  `round(v, DEFAULT_PRECISION_FOR_ROUNDING_ERROR)`, and that constant is 14). Mirrored here so the
 *  string this fit measures is the string the gauge renders, rather than one differing by a float
 *  artefact in the fourteenth place — which is exactly where a formatter with no decimal cap would
 *  turn a 9-character label into a 20-character one. */
function roundLikeECharts(v: number): number {
  return Number.isFinite(v) ? Number(v.toFixed(14)) : v;
}

/** The horizontal band of page a graduation's ink occupies, and the vertical one — the same box
 *  ECharts builds, from the same `align`/`verticalAlign` rules `_renderTicks` applies:
 *  `align` is left/right/centre by whether the unit x is past ±0.4, and `verticalAlign` top/bottom/
 *  middle by whether the unit y is past ∓0.8. Both matter: the two END graduations sit at the same
 *  height, anchored INWARD towards each other, so they can collide however far apart they are along
 *  the arc — while a label at the top of the arc clears its neighbours vertically even when their x
 *  bands overlap completely. */
interface LabelBox { x0: number; x1: number; y0: number; y1: number }

/**
 * Where the `n + 1` graduations of an `n`-split gauge land, and how wide each one is.
 *
 * `undefined` when nothing can be measured — `measure-text.ts` returns `{width: 0}` under jsdom by
 * design ("Size claims belong in a browser"), and a zero-width label clears any budget, so an
 * unmeasured environment would otherwise be told that eleven labels fit. Declining keeps this
 * function's contract "decide from a width, or decline".
 */
function graduationBoxes(
  size: EChartsSize,
  min: number,
  max: number,
  splitNumber: number,
  format: (v: number) => string,
  measure: TextMeasurer,
): LabelBox[] | undefined {
  const r = (RADIUS_PERCENT * Math.min(size.width, size.height)) / 2;
  const ring = r - SPLIT_LINE_LENGTH - (LABEL_DISTANCE + SPLIT_LINE_DISTANCE);
  if (!(ring > 0)) return undefined;

  const cx = size.width / 2;
  const cy = size.height / 2;
  const font = { size: LABEL_FONT_SIZE, family: resolveFontFamily(), weight: 400 };
  const boxes: LabelBox[] = [];
  let widest = 0;
  for (let i = 0; i <= splitNumber; i++) {
    const text = format(roundLikeECharts(min + (i * (max - min)) / splitNumber));
    const w = measure(text, font).width;
    if (!Number.isFinite(w)) return undefined;
    if (w > widest) widest = w;
    // ECharts negates the option's angle before taking its sine and cosine (`_renderMain`:
    // `-startAngle / 180 * PI`), which is what turns a mathematical angle into a screen one — the
    // arc opens downwards on the page. Reproduced rather than simplified so the two stay comparable.
    const radians = (-(START_ANGLE + (i * (END_ANGLE - START_ANGLE)) / splitNumber) * Math.PI) / 180;
    const unitX = Math.cos(radians);
    const unitY = Math.sin(radians);
    const x = unitX * ring + cx;
    const y = unitY * ring + cy;
    const x0 = unitX < -0.4 ? x : unitX > 0.4 ? x - w : x - w / 2;
    // A text box is one line tall, and zrender's line height at this font is the font size itself —
    // measured off the rendered SVG, where a top-aligned graduation's centre sits exactly
    // `LABEL_FONT_SIZE / 2` below its anchor.
    const y0 = unitY < -0.8 ? y : unitY > 0.8 ? y - LABEL_FONT_SIZE : y - LABEL_FONT_SIZE / 2;
    boxes.push({ x0, x1: x0 + w, y0, y1: y0 + LABEL_FONT_SIZE });
  }
  return widest > 0 ? boxes : undefined;
}

/** Whether every graduation keeps `HORIZONTAL_TICKS_GAP` of clear ink from every other, by inflating
 *  each box half a gap on all sides and asking that no two intersect. EVERY PAIR and not just the
 *  adjacent ones: on an arc the pair that collides need not be adjacent, and on this gauge the first
 *  pair to collide is the two OUTERMOST — 210° and -30° are the same height and anchored towards
 *  each other, so at the live card's 24.25px ring their anchors are 42px apart while `766,123.9`
 *  alone measures 49.3. */
function noneOverlap(boxes: LabelBox[]): boolean {
  const pad = HORIZONTAL_TICKS_GAP / 2;
  for (let i = 0; i < boxes.length; i++) {
    for (let j = i + 1; j < boxes.length; j++) {
      const a = boxes[i], b = boxes[j];
      if (a.x0 - pad < b.x1 + pad && b.x0 - pad < a.x1 + pad
        && a.y0 - pad < b.y1 + pad && b.y0 - pad < a.y1 + pad) return false;
    }
  }
  return true;
}

/**
 * How many intervals this gauge's arc can graduate — the answer this file's header argues for.
 *
 * `undefined` means SAY NOTHING: either the caller has no measurement, or nothing could be measured,
 * or ECharts' own default of 10 already clears, and in all three the option keeps exactly what it
 * built before this fit existed. `0` means draw NO graduations, Metabase's own answer below 250px.
 * Anything else is the `splitNumber` to set, and draws that many labels plus one.
 *
 * IT WALKS DOWN FROM THE CEILING RATHER THAN COMPUTING A COUNT, because the count is not a function
 * of one width. The labels an `n`-split gauge draws are `min + i * (max - min) / n` FORMATTED, and
 * that set changes with `n` in a way no arithmetic predicts: over 0-766,123.9 the maximum formats
 * `766,123.9` at 49.3px, `n = 2` adds `383,061.95` at 55.4px, and `n = 3` adds `255,374.633` at
 * 61.6px — the thirds are WIDER than the whole. So each rung is measured on the labels that rung
 * would really draw, which is the rule the temporal stride ladder settled on for the same reason
 * (`strideThatFits`, and the four blank axes that came of measuring a string the axis never drew).
 */
export function gaugeSplitNumber({
  size,
  min,
  max,
  format,
  measure = measureText,
}: {
  size: EChartsSize;
  min: number;
  max: number;
  /** The gauge's OWN label formatter, so the candidate is a string this arc would really render. */
  format: (v: number) => string;
  measure?: TextMeasurer;
}): number | undefined {
  if (!(size.width > 0) || !(size.height > 0)) return undefined;
  if (!Number.isFinite(min) || !Number.isFinite(max)) return undefined;
  let measurable = false;
  for (let n = DEFAULT_SPLIT_NUMBER; n >= 1; n--) {
    const boxes = graduationBoxes(size, min, max, n, format, measure);
    if (!boxes) continue;
    measurable = true;
    if (noneOverlap(boxes)) return n === DEFAULT_SPLIT_NUMBER ? undefined : n;
  }
  // Not even the two ends clear each other. Metabase draws no number on the arc at that size either;
  // the value is still on the card, in the centre detail at 24px, and the zones are still coloured.
  // `undefined` rather than `0` where nothing was measurable at all, so an unmeasured environment
  // keeps today's gauge instead of being silently stripped of its graduations.
  return measurable ? 0 : undefined;
}

// Metabase-style Gauge: one value on an arc between min and max.
export function buildGaugeOption(
  columns: ResultColumn[],
  rows: unknown[][],
  cfg: GaugeBuildConfig,
  /** The chart's measured pixel size, when the caller has one — this builder is pure data with no
   * DOM of its own, so EChartsRenderer measures the container and threads it through (see
   * `buildPieOption`, the first to take one).
   *
   * READ, since PR 05, for exactly one decision: how many graduations the arc can carry. That is a
   * question about the RADIUS the gauge was drawn at, which follows from this box and from nothing
   * else the builder can see — see the header above `gaugeSplitNumber`. Undefined means the caller
   * has no measurement (every unit test that does not pass one, and any caller handing over a plain
   * option object rather than a size function), and the gauge keeps ECharts' eleven numbers rather
   * than thinning against a guessed radius. */
  size?: EChartsSize,
): EChartsCoreOption {
  const mi = cfg.metric != null ? columns.findIndex((c) => c.name === cfg.metric) : -1;
  const raw = mi >= 0 && rows.length > 0 ? rows[0][mi] : undefined;
  const value = raw != null && Number.isFinite(Number(raw)) ? Number(raw) : 0;

  const segments = sanitizeSegments(value, cfg.segments);
  // The scale is the envelope of all zones: the lowest floor to the highest ceiling.
  const min = Math.min(...segments.map((s) => s.min));
  const max = Math.max(...segments.map((s) => s.max));
  const span = max - min;
  // ECharts' axisLine colour stops are cumulative upper bounds — [[0.3, A], [0.6, B], [1, C]]
  // reads as "A from 0 to 0.3, B from 0.3 to 0.6, C from 0.6 to 1", not as independent
  // per-segment ranges. Sorted by `max` (sanitizeSegments), each segment therefore paints
  // from the *previous* segment's `max` up to its own — its own declared `min` never enters
  // the calculation. That means an overlap resolves silently in favour of the lower `max`:
  // given [0,60] green and [40,100] red, green claims the whole 0-0.6 stretch (including the
  // 40-60 the red segment meant to own) and red only gets what's left, 0.6-1. Task 4's editor
  // keeps authored ranges contiguous so this can't be created from the UI; this is what
  // happens if it arrives anyway (e.g. hand-edited JSONB).
  // Guard span === 0 (degenerate range, e.g. a single point segment) rather than produce NaN.
  const stops: [number, string][] = segments.map((s, i) => [
    span > 0 ? (s.max - min) / span : 1,
    resolveColor(s.color, SERIES_PALETTE[i % SERIES_PALETTE.length]),
  ]);

  const fmt = (v: unknown) =>
    cfg.metric ? formatValue(v, cfg.metricTypes?.[cfg.metric] ?? "double", cfg.columnSettings?.[cfg.metric]) : String(v);
  const axisLabelColor = resolveColor("var(--muted-foreground)", COLOR_FALLBACK.axisLabel);
  const textColor = resolveColor("var(--foreground)", COLOR_FALLBACK.text);

  // ECharts' gauge has no per-zone label slot on the axis line itself, so a segment's `label`
  // (e.g. "Healthy" / "Critical") is surfaced in the aria description instead of the tooltip:
  // the description is already the one place this gauge states its scale in words and is read
  // regardless of where on the arc a user's attention is, whereas the tooltip only ever reports
  // the current pointer value and would say nothing about zones the pointer isn't over.
  const zoneLabels = segments
    .filter((s) => s.label)
    .map((s) => `${s.label} (${fmt(s.min)}–${fmt(s.max)})`)
    .join(", ");

  // `undefined` at both ends of this: no measured size, or a size with room for ECharts' own
  // eleven. Either way the two keys below are left off and the option is the one this builder made
  // before the fit existed.
  const splitNumber = size ? gaugeSplitNumber({ size, min, max, format: fmt }) : undefined;

  return {
    ...baseOption(),
    // One reading on an arc; what matters is the value and the range it sits in.
    aria: {
      ...ARIA_A11Y,
      label: {
        description: `Gauge${cfg.metric ? ` of ${cfg.metric}` : ""}. ${fmt(value)}, on a scale from ${fmt(min)} to ${fmt(max)}.`
          + (zoneLabels ? ` Zones: ${zoneLabels}.` : ""),
      },
    },
    tooltip: itemTooltip(fmt),
    series: [{
      type: "gauge",
      min, max,
      startAngle: START_ANGLE, endAngle: END_ANGLE,
      // How many numbers the arc carries. Omitted where the fit declines, which leaves ECharts'
      // default of 10 — see `gaugeSplitNumber`, and note that a gauge honours this LITERALLY where a
      // cartesian value axis rounds it.
      ...(splitNumber != null && splitNumber > 0 ? { splitNumber } : {}),
      // Coloured zones from cfg.segments (today's fixed red/yellow/green thirds when unset).
      axisLine: { lineStyle: { width: 18, color: stops } },
      axisTick: { show: false },
      splitLine: { show: false },
      axisLabel: {
        formatter: (v: number) => fmt(v), distance: LABEL_DISTANCE, fontSize: LABEL_FONT_SIZE,
        color: axisLabelColor,
        // `0` is the fit's "not even the two ends clear each other" — Metabase's own answer below
        // 250px. `show` is written only then, so a gauge that keeps its graduations keeps an
        // `axisLabel` byte-identical to the one it had before this fit existed.
        ...(splitNumber === 0 ? { show: false } : {}),
      },
      pointer: { show: true, itemStyle: { color: textColor } },
      anchor: { show: false },
      title: { show: false },
      detail: { valueAnimation: false, formatter: (v: number) => fmt(v), fontSize: 24, fontWeight: 700, offsetCenter: [0, "55%"], color: textColor },
      data: [{ value }],
    }],
  };
}
