import type { EChartsCoreOption } from "echarts/core";
import type { GaugeSegment } from "@/api/charts";
import type { ColumnFormat } from "../../format-types";
import { formatValue } from "../../format";
import { baseOption, itemTooltip } from "../base";
import { ARIA_A11Y } from "../palette";
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

/** Today's three zones, derived rather than stored so an unconfigured gauge is unchanged. */
function defaultSegments(value: number): GaugeSegment[] {
  const max = value > 0 ? value * 2 : 100;
  const third = max / 3;
  return [
    { min: 0, max: third, color: ZONE_LOW },
    { min: third, max: 2 * third, color: ZONE_MID },
    { min: 2 * third, max, color: ZONE_HIGH },
  ];
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
  return valid.length > 0 ? valid : defaultSegments(value);
}

// Metabase-style Gauge: one value on an arc between min and max.
export function buildGaugeOption(columns: ResultColumn[], rows: unknown[][], cfg: GaugeBuildConfig): EChartsCoreOption {
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
      startAngle: 210, endAngle: -30,
      // Coloured zones from cfg.segments (today's fixed red/yellow/green thirds when unset).
      axisLine: { lineStyle: { width: 18, color: stops } },
      axisTick: { show: false },
      splitLine: { show: false },
      axisLabel: { formatter: (v: number) => fmt(v), distance: 18, fontSize: 11, color: axisLabelColor },
      pointer: { show: true, itemStyle: { color: textColor } },
      anchor: { show: false },
      title: { show: false },
      detail: { valueAnimation: false, formatter: (v: number) => fmt(v), fontSize: 24, fontWeight: 700, offsetCenter: [0, "55%"], color: textColor },
      data: [{ value }],
    }],
  };
}
