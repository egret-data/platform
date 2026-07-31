import type { EChartsCoreOption } from "echarts/core";
import type { ColumnFormat } from "../../format-types";
import { formatValue } from "../../format";
import { ariaDescription, baseOption, itemTooltip } from "../base";


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

// Metabase-style Funnel: one row per stage, widths by metric, sorted descending.
export function buildFunnelOption(columns: ResultColumn[], rows: unknown[][], cfg: FunnelBuildConfig): EChartsCoreOption {
  const idx = Object.fromEntries(columns.map((c, i) => [c.name, i]));
  const di = cfg.dimension != null ? idx[cfg.dimension] : undefined;
  const mi = cfg.metric != null ? idx[cfg.metric] : undefined;

  // ROW ORDER, not value order. A funnel's steps are a SEQUENCE — visit, sign up, pay — and
  // sorting them by size rearranges that sequence into a story the data never told: measured
  // against Metabase on four categories whose values do not descend, it keeps them in row order
  // while egret reordered them. `sort: "none"` on the series below is the other half of this; the
  // ECharts default is descending and would undo it.
  const hidden = new Set(cfg.hiddenSteps ?? []);
  const rank = new Map((cfg.stepOrder ?? []).map((n, i) => [n, i]));
  const data = di == null || mi == null
    ? []
    : rows
        .map((r) => ({ name: String(r[di] ?? ""), value: Number(r[mi]) }))
        .filter((s) => Number.isFinite(s.value) && s.value > 0 && !hidden.has(s.name))
        // A stored order names the steps it knows; anything new keeps its natural place after
        // them, the rule `applySeriesOrder` and the pie's `sliceOrder` already use.
        .sort((a, b) => (rank.get(a.name) ?? Number.MAX_SAFE_INTEGER) - (rank.get(b.name) ?? Number.MAX_SAFE_INTEGER));

  // Every step's share of the FIRST one — what a funnel is read for. Metabase prints it on each
  // step after the first ("124.22 %"); egret printed only the raw value, so the reader had to
  // divide in their head.
  const firstValue = data[0]?.value;
  const share = (v: number) =>
    firstValue != null && firstValue !== 0 ? `${((v / firstValue) * 100).toFixed(2)} %` : "";

  const fmt = (v: unknown) =>
    cfg.metric ? formatValue(v, cfg.metricTypes?.[cfg.metric] ?? "double", cfg.columnSettings?.[cfg.metric]) : String(v);

  return {
    ...baseOption(),
    aria: ariaDescription({
      kind: "Funnel chart", metric: cfg.metric, dimension: cfg.dimension, format: fmt,
      points: data.map((d) => ({ name: String(d.name), value: d.value })),
    }),
    tooltip: itemTooltip(fmt, { percent: true }),
    series: [{
      type: "funnel",
      orient: "horizontal",   // Metabase renders funnels left→right
      // See the comment on `data`: ECharts would re-sort descending by default.
      sort: "none",
      funnelAlign: "center",
      gap: 2,
      left: "5%",
      right: "5%",
      top: "6%",
      bottom: "6%",
      data,
      // Always shown. This used to read `cfg.showLabels !== false`, but `showLabels` was on no
      // config any caller could produce — not `FunnelConfig`, not any settings panel — so the
      // `!== false` arm was unreachable and every funnel ever drawn took this branch. Now that
      // ChartRenderer SPREADS the config, an accepted-but-unsettable field stops being merely
      // dead and becomes a backdoor: a hand-edited `funnel.showLabels` in stored settings could
      // switch a chart's labels off with no control anywhere to switch them back on. Removed
      // rather than exposed — exposing it is a feature (field + panel toggle + builder), not
      // debt cleanup, and this keeps every funnel drawing exactly what it draws today.
      label: {
        show: true, position: "inside",
        formatter: (p: { name: string; value: unknown; dataIndex: number }) => {
          const head = `${p.name}: ${fmt(p.value)}`;
          const pct = p.dataIndex > 0 ? share(Number(p.value)) : "";
          return pct ? `${head} (${pct})` : head;
        },
      },
    }],
  };
}
