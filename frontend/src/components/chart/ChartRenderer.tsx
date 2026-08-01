// src/components/chart/ChartRenderer.tsx
import { useMemo, useRef, useState } from "react";
import type {
  AnyChartParams, BoxplotConfig, CartesianConfig, FunnelConfig, GaugeConfig,
  PieConfig, ProgressConfig, SankeyConfig, ScalarConfig, SmartScalarConfig, VizType, WaterfallConfig,
} from "@/api/charts";
import type { PivotConfig } from "@/components/chart/viz/pivot";
import { isCartesianViz } from "@/api/charts";
import { vizConfig, shellSettings } from "@/components/chart/viz/renderConfig";
import type { DrillContext, DrillDim } from "@/components/chart/drill";
import { EChartsRenderer, type EChartsOptionInput, type EChartsSize } from "@/components/chart/echarts/EChartsRenderer";
import { buildCartesianOption, type CartesianBuildConfig } from "@/components/chart/echarts/options/cartesian";
import { cartesianMeta, type CartesianMeta } from "@/components/chart/echarts/options/cartesianMeta";
import {
  buildPieOption, buildPieLegend, buildPieHole, buildPieHoleGraphic, pieLegendPlacement, pieHasSlices,
  buildPieBreakdown, pieSliceDrillPath,
} from "@/components/chart/echarts/options/pie";
import { PieBreakdown } from "@/components/chart/PieBreakdown";
import { PieLegend } from "@/components/chart/PieLegend";
import { useElementSize } from "@/components/chart/useElementSize";
import { cn } from "@/lib/utils";
import { buildGaugeOption } from "@/components/chart/echarts/options/gauge";
import { FunnelChart } from "@/components/chart/FunnelChart";
import { buildWaterfallOption } from "@/components/chart/echarts/options/waterfall";
import { buildBoxplotOption } from "@/components/chart/echarts/options/boxplot";
import { buildSankeyOption, sankeyProblem } from "@/components/chart/echarts/options/sankey";
import { ChartTable, type ResultColumn } from "@/components/chart/ChartTable";
import { PivotTable } from "@/components/chart/pivot/PivotTable";
import { BigNumber } from "@/components/chart/BigNumber";
import { ProgressChart } from "@/components/chart/ProgressChart";
import { SmartScalarChart } from "@/components/chart/SmartScalarChart";
import { ObjectDetail } from "@/components/chart/ObjectDetail";
import { MapChart } from "@/components/chart/MapChart";

/** `message` defaults to the empty-RESULT wording every viz shares. A caller passes its own when
 * the result was not empty — the pie's is the case that needs it: rows came back and none of them
 * is something a share can be taken of. */
const EmptyState = ({ message = "No data" }: { message?: string }) => (
  <div className="flex h-full items-center justify-center text-sm text-muted-foreground">{message}</div>
);

/** Stable stand-in for an omitted `metricTypes` prop. A `= {}` default parameter allocates a
 * fresh object on every render, which would defeat any memo downstream that depends on it. */
const NO_METRIC_TYPES: Record<string, string> = {};

/**
 * The pie branch, as its own component purely so the option can be memoised: pie is the one
 * viz whose `option` is a FUNCTION of the measured container size, and EChartsRenderer's
 * `[option]` effect re-applies the whole option whenever that function's identity changes.
 * For pie, re-applying means re-running buildPieOption's per-slice fit pass — real canvas
 * `measureText` for every slice in every ring. Built inline in ChartRenderer, that arrow was
 * fresh on every render of whatever renders the chart (a hover, a filter chip, a sibling's
 * state change), so the measurement pass ran far more often than the resize path
 * EChartsRenderer's rAF coalescing already guards — a strictly bigger version of the same cost.
 *
 * Hooks can't live in ChartRenderer itself past its early returns, and a component boundary is
 * cheaper than a memo that runs for every viz type.
 */
function PieChart({ params, columns, rows, metricTypes, onValueClick }: {
  params: AnyChartParams;
  columns: ResultColumn[];
  rows: unknown[][];
  metricTypes: Record<string, string>;
  onValueClick?: (ctx: DrillContext, x: number, y: number) => void;
}) {
  const pie = vizConfig<PieConfig>("pie", params);
  const { columnSettings } = shellSettings(params);
  // Keyed by column name, same shape as the spec map itself — each ring looks up its own
  // dimension's unit rather than ChartRenderer resolving just the first one.
  const dimensionUnits = params.spec.temporal_units;

  // A VALUE key, deliberately not an identity dep list. `params` cannot be one: every real call
  // site rebuilds it from scratch on each render (ChartExplore's `previewParams()` spreads a
  // fresh object, the dashboard's `applyVizOverride()` returns a new one), so `[params]` would
  // memoise nothing at all while looking like it did. Same for `metricTypes`, which ChartExplore
  // rebuilds with `Object.fromEntries` every render. All four of these are small, flat setting
  // records; stringifying them once per render costs nothing next to the per-slice canvas
  // measurement it saves. `columns`/`rows` stay identity deps — they're the big objects, and
  // they come straight from the query cache, which does keep them stable between renders.
  const settingsKey = JSON.stringify([pie, columnSettings, metricTypes, dimensionUnits]);
  const input = useMemo(
    // Spread rather than list the fields, for the same reason the cartesian branch does: this
    // was a hand-maintained whitelist and it silently dropped decimals, sliceNames,
    // hiddenSlices and sliceOrder — making slice rename, hide, reorder and percent precision
    // unreachable from the app despite being fully implemented and exposed in the panel.
    () => ({ ...pie, columnSettings, metricTypes, dimensionUnits }),
    // settingsKey encodes all four of them by value, so nothing here can go stale — a config or
    // format edit changes the key and rebuilds. Listing them as well would defeat the point.
    [settingsKey],
  );
  // The legend is markup beside the chart, so THIS element is the whole host (chart + legend) and
  // the placement decision is made from its size — `pieLegendPlacement` is the one place that
  // decides, and the answer is handed to the option builder rather than re-derived there from a
  // size that by then means something different (the chart's own box, already legend-free, which
  // EChartsRenderer measures for itself).
  const hostRef = useRef<HTMLDivElement>(null);
  const placement = pieLegendPlacement(useElementSize(hostRef));

  // The arcs-to-markup alternative, from the same `input` the chart is drawn from, so the two
  // cannot disagree about hidden slices, renames, Other-folding or number formatting. It returns
  // nothing below two rings — a single-ring pie is already read in full by `ariaDescription`.
  const breakdown = useMemo(() => buildPieBreakdown(columns, rows, input), [columns, rows, input]);

  // Ephemeral, per-mount: which ring-0 slices the reader has clicked off. Deliberately NOT stored
  // in the chart's settings — this replaces the ECharts legend's own click-to-hide, which was
  // never persisted either (that is what `pie.hiddenSlices` is for).
  const [toggledOff, setToggledOff] = useState<string[]>([]);
  // The legend needs the same toggle list the option is built with, because a toggled-off slice is
  // not in the sunburst's datum list at all and every index after it shifts down — see
  // `PieLegendEntry.sliceDataIndex`. It still LISTS every entry; only the index is withheld.
  const legendCfg = useMemo(() => ({ ...input, toggledOffSlices: toggledOff }), [input, toggledOff]);
  const legend = useMemo(() => buildPieLegend(columns, rows, legendCfg), [legendCfg, columns, rows]);

  // The slice under the pointer, from either direction — the chart or the legend. HOVER IS
  // DELIBERATELY NOT A DEPENDENCY OF `option` BELOW: building that option runs a per-slice
  // `measureText` fit pass over every ring, and re-applying it would additionally tear the sunburst
  // down and re-init it — losing, mid-hover, the very emphasis state the hover is producing.
  // Everything hover-driven therefore lands somewhere cheap: the centre readout as a merge-mode
  // `graphic` overlay (see `overlay`), the legend as ordinary markup, and the chart's own emphasis
  // as a `dispatchAction`.
  const [hovered, setHovered] = useState<{
    name: string;
    value: unknown;
    fromLegend: boolean;
    /** The ring-0 ancestor's name — which legend entry stays lit. Its own name at ring 0. */
    rootName: string;
    /** Legend hovers only: the datum the chart should hold in emphasis. */
    dataIndex?: number;
  } | null>(null);

  const chartCfg = useMemo(
    () => ({ ...input, legendPlacement: placement, toggledOffSlices: toggledOff }),
    [input, placement, toggledOff],
  );

  const option = useMemo(
    // `undefined` here is buildPieOption's own measureText param (position 4) — its default (a
    // real canvas measurer) is exactly what production wants; `size` (the measured container
    // from EChartsRenderer) fills containerSize (position 5) — see that function's doc comment.
    () => (size: EChartsSize) => buildPieOption(columns, rows, chartCfg, undefined, size),
    [chartCfg, columns, rows],
  );

  // What the hole says. Built once per data/config change and then merely FORMATTED per hover, so
  // pointing at a slice costs two text measurements rather than another pass over the rows.
  const hole = useMemo(() => buildPieHole(columns, rows, input), [input, columns, rows]);
  const overlay = useMemo(
    () =>
      hole.shown
        ? (size: EChartsSize) => ({ graphic: buildPieHoleGraphic(hole, chartCfg, hovered, size) })
        : undefined,
    [hole, chartCfg, hovered],
  );

  // NOTHING TO DRAW — rows, but no wedge among them (see `pieHasSlices`). ChartRenderer's shared
  // `rows.length === 0` guard cannot see this case, and what used to happen was an empty sunburst:
  // a blank rectangle, with the chart's own aria description already saying "No data." to anyone
  // reading it that way. Placed after every hook, since the count depends on the same memoised
  // config they do.
  const hasSlices = useMemo(() => pieHasSlices(columns, rows, input), [input, columns, rows]);

  const chart = (
    <EChartsRenderer
      key="chart"
      option={option}
      overlay={overlay}
      className="min-h-0 min-w-0 flex-1"
      // A sunburst node carries its own name and value on the hover event, so the readout needs no
      // lookup back into the data at all. `treePathInfo[1]` is the ring-0 slice this one descends
      // from (entry 0 being ECharts' virtual root) — the legend entry that names its colour family,
      // and so the one that stays lit while everything else fades.
      onPointHover={(p) => setHovered(p ? {
        name: p.name ?? "",
        // The datum's OWN figure, not the one ECharts laid the wedge out with: a sub-0.5% slice is
        // drawn at the floor so it can be pointed at (options/pie.ts, MIN_DRAWN_SLICE_FRACTION),
        // and the hole is a readout — it must say what the slice is worth. `?? p.value` is the
        // path every other viz takes, none of which draws anything but its own value.
        value: (p.data as { rawValue?: unknown } | undefined)?.rawValue ?? p.value,
        fromLegend: false,
        rootName: p.treePathInfo?.[1]?.name ?? p.name ?? "",
      } : null)}
      // Only the LEGEND direction dispatches, which is why only the legend path sets `dataIndex`:
      // when the pointer is on the chart, ECharts is already running its own emphasis/blur for the
      // element under it, and asking for a second one would fight it. Metabase draws the same line,
      // for the same reason.
      highlightDataIndex={hovered?.dataIndex}
      onPointClick={onValueClick && pie.dimensions?.[0] ? (p) => {
        // The slice's whole root-to-here path in RAW dimension values, from the datum's own
        // aggregation key. Three things the old `{ col: dimensions[ring], value: p.name }` got
        // wrong, all of them silent:
        //   - a nested slice contributed only its own ring, so "Widgets" under "North" drilled to
        //     every region's Widgets — the answer to a question nobody asked;
        //   - a folded bucket drilled to `= 'Other'`, which matches no row at all;
        //   - a bucketed date drilled to the instant the label shows instead of the period it
        //     stands for, the same under-report the table, the pivot and the cartesian had.
        const path = pieSliceDrillPath((p.data as { key?: string } | undefined)?.key);
        if (!path) return; // an Other bucket, or a datum with no key — nothing single to filter by
        const dims: DrillDim[] = [];
        path.forEach((value, ring) => {
          const col = pie.dimensions![ring];
          if (!col) return;
          const unit = params.spec.temporal_units?.[col];
          dims.push(unit ? { col, value, unit } : { col, value });
        });
        if (dims.length === 0) return;
        onValueClick({ dims }, p.x, p.y);
      } : undefined}
    />
  );
  const legendEl = (
    <PieLegend
      key="legend"
      entries={legend}
      placement={placement}
      toggledOff={toggledOff}
      onToggle={(name) =>
        setToggledOff((prev) => (prev.includes(name) ? prev.filter((n) => n !== name) : [...prev, name]))
      }
      highlighted={hovered?.rootName ?? null}
      onHover={(name) => {
        const entry = name != null ? legend.find((e) => e.name === name) : undefined;
        setHovered(entry
          ? { name: entry.name, value: entry.value, fromLegend: true, rootName: entry.name, dataIndex: entry.sliceDataIndex }
          : null);
      }}
    />
  );

  // "No values to chart", not "No data": the query DID return rows — a pie just cannot show a
  // share of a column that is all zeros. Said in the shell's own empty state rather than as
  // Metabase's grey "Other" disc, because egret has one and because a full circle in a single
  // colour reads as one category worth everything.
  if (!hasSlices) return <EmptyState message="No values to chart" />;

  // The column goes to the LEFT of the chart (where Metabase's is, and where the ECharts side
  // legend was); the block goes underneath. Keyed children rather than two branches, so flipping
  // placement on a resize never remounts the chart — a remount would re-init ECharts and re-run
  // the whole per-slice fit pass from scratch.
  //
  // `hostRef` sits on the chart+legend row, NOT on the outer element, so the breakdown's closed
  // summary can never feed back into `pieLegendPlacement` — measuring the outer one would let the
  // disclosure's own height change the side/bottom answer, and opening it would change it again.
  return (
    <div className="flex h-full w-full min-h-0 min-w-0 flex-col">
      <div ref={hostRef} className={cn("flex min-h-0 min-w-0 flex-1", placement === "side" ? "flex-row" : "flex-col")}>
        {placement === "side" ? [legendEl, chart] : [chart, legendEl]}
      </div>
      <PieBreakdown breakdown={breakdown} />
    </div>
  );
}

/**
 * The cartesian branch, as its own component for the same reason `PieChart` is one: the `option`
 * becomes a FUNCTION of the measured size, and `EChartsRenderer`'s `[option]` effect re-applies
 * the whole option whenever that function's identity changes. Built inline, the arrow would be
 * fresh on every render of whatever renders the chart — a hover, a filter chip, a sibling's state
 * change — and each one would re-run the builder and re-apply the option.
 *
 * Hooks cannot live in `ChartRenderer` itself past its early returns, so this is a component
 * rather than a memo.
 *
 * Both memos are keyed on the SAME dependencies on purpose: `cartesianMeta` builds the option a
 * second time (see its doc comment — it reads `__foldedCategory` and `series[].__raw` off a build
 * rather than reimplementing the folding rules), so that second build has to be pinned to data and
 * config changes too, or it runs on every render.
 */
function CartesianChart({ columns, rows, cfg, children }: {
  columns: ResultColumn[];
  rows: unknown[][];
  cfg: CartesianBuildConfig;
  children: (meta: CartesianMeta, option: EChartsOptionInput) => React.ReactNode;
}) {
  // A VALUE key, deliberately not an identity dep — the same treatment, for the same reason,
  // `PieChart` gives its own config above. `cfg` CANNOT be an identity dep: `ChartRenderer`
  // rebuilds it from scratch on every render (its `cartesian` is `{ yMetrics: [], ...vizConfig() }`,
  // a fresh object each time), so `[cfg]` would memoise nothing at all while looking like it did.
  // That is not a guess — with `[cfg]` the option's identity did not survive a parent render that
  // changed nothing, which is precisely the case this component exists to make cheap. `cfg` is a
  // small, flat, plain-data record; stringifying it once per render costs nothing next to the TWO
  // option builds it saves. `columns`/`rows` stay identity deps — they're the big objects, and they
  // come straight from the query cache, which does keep them stable between renders.
  const cfgKey = JSON.stringify(cfg);
  const option = useMemo(
    () => (size: EChartsSize) => buildCartesianOption(columns, rows, cfg, size),
    // cfgKey encodes cfg by value, so nothing here can go stale — a config edit changes the key and
    // rebuilds. Listing `cfg` as well would defeat the point.
    [columns, rows, cfgKey],
  );
  const meta = useMemo(() => cartesianMeta(columns, rows, cfg), [columns, rows, cfgKey]);
  return <>{children(meta, option)}</>;
}

export interface ChartRendererProps {
  vizType: VizType;
  /** Either stored shape — the config is resolved through the viz's descriptor. */
  params: AnyChartParams;
  columns: ResultColumn[];
  rows: unknown[][];
  /** metric_name -> data_type, for metric value formatting. Optional; defaults to {}. */
  metricTypes?: Record<string, string>;
  onValueClick?: (ctx: DrillContext, x: number, y: number) => void;
  /** The card's width in dashboard grid columns, when it is on a grid. Supplied only by
   * `DashboardCardView`; the studio preview has no grid and omits it. */
  gridCols?: number;
}

export function ChartRenderer({ vizType, params, columns, rows, metricTypes = NO_METRIC_TYPES, onValueClick, gridCols }: ChartRendererProps) {
  // One config, from the viz's own descriptor; each branch below reads it as its type.
  const cfg = vizConfig(vizType, params);
  const cartesian = { yMetrics: [], ...cfg } as CartesianConfig;
  const scalar = cfg as ScalarConfig;
  const progress = cfg as ProgressConfig;
  const gauge = cfg as GaugeConfig;
  const funnel = cfg as FunnelConfig;
  const waterfall = cfg as WaterfallConfig;
  const smart = cfg as SmartScalarConfig;
  const box = cfg as BoxplotConfig;
  const sankey = cfg as SankeyConfig;
  const pivot = cfg as unknown as PivotConfig;
  const { columnSettings, conditionalFormatting, tableState } = shellSettings(params);
  const dimensionColumns = params.spec.metrics.length > 0 ? params.spec.dimensions : [];

  if (vizType === "object") {
    return <ObjectDetail columns={columns} rows={rows} columnSettings={columnSettings} />;
  }

  if (vizType === "map") {
    return <MapChart columns={columns} rows={rows} params={params} metricTypes={metricTypes} onValueClick={onValueClick} />;
  }

  if (vizType === "table") {
    return (
      <ChartTable
        columns={columns}
        rows={rows}
        columnSettings={columnSettings}
        conditionalFormatting={conditionalFormatting}
        tableState={tableState}
        dimensionColumns={dimensionColumns}
        temporalUnits={params.spec.temporal_units ?? {}}
        onCellDrill={onValueClick}
      />
    );
  }

  if (vizType === "pivot_table") {
    return (
      <PivotTable
        input={{
          columns: columns.map((c) => c.name),
          rows,
          rowDims: pivot.rows ?? [],
          colDims: pivot.cols ?? [],
          metrics: pivot.metrics ?? [],
          showRowTotals: pivot.showRowTotals ?? true,
          showColTotals: pivot.showColTotals ?? true,
          condense: pivot.condense ?? false,
          collapsed: new Set(pivot.collapsedRows ?? []),
          sortDims: pivot.sort ?? {},
          dimTotals: pivot.dimTotals,
        }}
        columnWidths={pivot.columnWidths}
        temporalUnits={params.spec.temporal_units ?? {}}
        columnTypes={Object.fromEntries(columns.map((c) => [c.name, c.type]))}
        columnSettings={columnSettings}
        conditionalFormatting={conditionalFormatting}
        metricTypes={metricTypes}
        onCellDrill={onValueClick}
      />
    );
  }

  if (isCartesianViz(vizType)) {
    const seriesUnit = cartesian.series ? params.spec.temporal_units?.[cartesian.series] : undefined;
    // Named rather than written inline on `CartesianChart` purely so the JSX below stays readable —
    // this object is fresh on every render either way, which is exactly why `CartesianChart` keys
    // its memos on its VALUE rather than its identity. Annotated, so a field that stops matching
    // `CartesianBuildConfig` is caught here rather than silently widening the prop.
    const cartesianCfg: CartesianBuildConfig = {
      // Spread rather than list the fields: this used to be a hand-maintained whitelist, and
      // four settings added in one branch were silently dropped between the panel and the
      // chart because nobody remembered to extend it.
      ...cartesian,
      kind: vizType,
      columnSettings,
      metricTypes,
      xAxisUnit: params.spec.temporal_units?.[cartesian.xAxis ?? ""],
      xAxisType: columns.find((c) => c.name === cartesian.xAxis)?.type,
      seriesUnit,
      seriesType: columns.find((c) => c.name === cartesian.series)?.type,
    };
    return (
      <div className="h-full w-full">
        {rows.length === 0
          ? <EmptyState />
          : <CartesianChart columns={columns} rows={rows} cfg={cartesianCfg}>{(meta, option) =>
            // `meta` is what the click handler used to read back off the built option — impossible
            // now that the option is a function. See `cartesianMeta`.
            <EChartsRenderer option={option}
            onPointClick={onValueClick && cartesian.xAxis ? (p) => {
              // The x value carries its temporal unit, so a bucketed axis drills to the whole
              // bucket. Without it a bar grouped by month filtered `= 2026-01-01`, which is one
              // day of the month the bar actually covers — the same silent under-count the pivot
              // had, understating the result with nothing on screen to say so.
              //
              // It also retires the note that used to sit here: with an EXTRACTION unit
              // (`day_of_week`) `p.name` is the display label ("Mon"), not the raw integer, so the
              // filter could never match. `drillFilters` now drops extraction dimensions instead
              // of emitting a predicate that silently matches nothing.
              // The long-tail fold's "Other" is a bucket the chart invented, so there is nothing
              // single to filter by — and because the fold merges into a real category of that
              // name when one exists, `= 'Other'` can even return a fraction of the bar and pass
              // for an answer. Same refusal the pie makes for its own folded slice.
              if (meta.foldedCategory != null && p.name === meta.foldedCategory) return;
              const unit = params.spec.temporal_units?.[cartesian.xAxis as string];
              const dims: DrillDim[] = [
                unit
                  ? { col: cartesian.xAxis as string, value: p.name, unit }
                  : { col: cartesian.xAxis as string, value: p.name },
              ];
              // The series' RAW value, not `p.seriesName` — that is the legend's label, so a
              // renamed series (or one whose column carries a date format) filtered by text no
              // row holds and drilled to nothing at all.
              if (cartesian.series) {
                const raw = meta.rawSeriesAt(p.seriesIndex) ?? p.seriesName;
                if (raw != null) {
                  dims.push(seriesUnit
                    ? { col: cartesian.series, value: raw, unit: seriesUnit }
                    : { col: cartesian.series, value: raw });
                }
              }
              onValueClick({ dims }, p.x, p.y);
            } : undefined}
            />}
          </CartesianChart>}
      </div>
    );
  }

  if (vizType === "pie") {
    return (
      <div className="h-full w-full">
        {rows.length === 0
          ? <EmptyState />
          : <PieChart params={params} columns={columns} rows={rows} metricTypes={metricTypes} onValueClick={onValueClick} />}
      </div>
    );
  }

  if (vizType === "scalar") {
    // SPREADS its config, for the reason spelled out above the five branches below: a
    // hand-maintained field list is a trap, not a safeguard. This branch used to list `metric`
    // alone, and this unit adds three more fields to the other side of it.
    return (
      <div className="h-full w-full">
        <BigNumber
          {...scalar}
          columns={columns}
          rows={rows}
          format={scalar.metric ? columnSettings[scalar.metric] : undefined}
          onValueClick={onValueClick}
          gridCols={gridCols}
        />
      </div>
    );
  }

  if (vizType === "progress") {
    return (
      <div className="h-full w-full">
        <ProgressChart
          columns={columns}
          rows={rows}
          metric={progress.metric}
          goal={progress.goal}
          goalColumn={progress.goalColumn}
          color={progress.color}
          format={progress.metric ? columnSettings[progress.metric] : undefined}
        />
      </div>
    );
  }

  // THE FIVE BRANCHES BELOW SPREAD THEIR CONFIG, for the reason cartesian and pie already do.
  // Each of them used to re-list the fields it forwarded, and each list happened to match its
  // config interface — which is precisely the state the pie branch was in until a field was added
  // to one side and not the other. That cost four settings (decimals, sliceNames, hiddenSlices,
  // sliceOrder) which shipped implemented and panel-exposed but unreachable from the app, and
  // cartesian lost four of its own the same way before that. A whitelist that is correct today is
  // a trap, not a safeguard; spreading makes the two sides impossible to drift apart.
  //
  // `columnSettings` and `metricTypes` come AFTER the spread deliberately: they are the shell's,
  // not the viz's, so a stray same-named key in stored settings cannot shadow them.
  if (vizType === "gauge") {
    return (
      <div className="h-full w-full">
        {rows.length === 0
          ? <EmptyState />
          : <EChartsRenderer option={buildGaugeOption(columns, rows, { ...gauge, columnSettings, metricTypes })} />}
      </div>
    );
  }

  if (vizType === "funnel") {
    return (
      <div className="h-full w-full">
        {rows.length === 0
          ? <EmptyState />
          : <FunnelChart columns={columns} rows={rows} {...funnel} columnSettings={columnSettings} metricTypes={metricTypes} />}
      </div>
    );
  }

  if (vizType === "waterfall") {
    return (
      <div className="h-full w-full">
        {rows.length === 0
          ? <EmptyState />
          : <EChartsRenderer
            option={buildWaterfallOption(columns, rows, { ...waterfall, columnSettings, metricTypes })}
            // A waterfall step IS a row, so it drills like a bar — Metabase's does, being a
            // CartesianChart. This branch passed no handler at all, so clicking a waterfall did
            // nothing; the same seam-level omission the pie had in the studio preview.
            onPointClick={onValueClick && waterfall.xAxis ? (p) => {
              // The Total is a bar the CHART invented by summing the steps; no row holds it, so
              // `= 'Total'` would drill to nothing (or, worse, to a real category of that name).
              // Same refusal the cartesian fold makes for its "Other".
              if (waterfall.showTotal !== false && p.name === "Total") return;
              const unit = params.spec.temporal_units?.[waterfall.xAxis as string];
              const dim: DrillDim = unit
                ? { col: waterfall.xAxis as string, value: p.name, unit }
                : { col: waterfall.xAxis as string, value: p.name };
              onValueClick({ dims: [dim] }, p.x, p.y);
            } : undefined}
          />}
      </div>
    );
  }

  if (vizType === "smartscalar") {
    return (
      <div className="h-full w-full">
        <SmartScalarChart
          columns={columns}
          rows={rows}
          dimension={smart.dimension}
          metric={smart.metric}
          format={smart.metric ? columnSettings[smart.metric] : undefined}
          dimensionFormat={smart.dimension ? columnSettings[smart.dimension] : undefined}
          unit={smart.dimension ? params.spec.temporal_units?.[smart.dimension] : undefined}
          switchPositiveNegative={!!smart.switchPositiveNegative}
          compact={!!smart.compact}
        />
      </div>
    );
  }

  if (vizType === "boxplot") {
    const boxOption = buildBoxplotOption(columns, rows, { ...box, columnSettings, metricTypes });
    // The categories the builder settled on, read back rather than recomputed — a box's own
    // `dataIndex` and a point's `data[0]` are both indexes into this list.
    const boxCategories = ((boxOption as { xAxis?: { data?: string[] } }).xAxis?.data ?? []);
    return (
      <div className="h-full w-full">
        {rows.length === 0
          ? <EmptyState />
          : <EChartsRenderer
            option={boxOption}
            // Metabase's box plot drills; egret's branch passed no handler, so clicking a box did
            // nothing. A box IS a category, and so is the point drawn above it.
            onPointClick={onValueClick && box.category ? (p) => {
              const i = Array.isArray(p.data) ? Number((p.data as unknown[])[0]) : p.dataIndex;
              const value = boxCategories[i ?? -1] ?? p.name;
              if (value == null) return;
              const unit = params.spec.temporal_units?.[box.category as string];
              const dim: DrillDim = unit
                ? { col: box.category as string, value, unit }
                : { col: box.category as string, value };
              onValueClick({ dims: [dim] }, p.x, p.y);
            } : undefined}
          />}
      </div>
    );
  }

  if (vizType === "sankey") {
    // Checked BEFORE building, the way Metabase's `checkRenderable` is. A circular flow otherwise
    // reaches ECharts, which throws "Sankey is a DAG, the original data has cycle!" onto the card —
    // the library's own wording, naming an acronym rather than a fix.
    const sankeyIssue = sankeyProblem(columns, rows, sankey);
    return (
      <div className="h-full w-full">
        {rows.length === 0
          ? <EmptyState />
          : sankeyIssue
            ? <div className="flex h-full items-center justify-center p-6 text-center text-sm text-muted-foreground">
              {sankeyIssue}
            </div>
            : <EChartsRenderer
              option={buildSankeyOption(columns, rows, { ...sankey, columnSettings, metricTypes })}
              // Metabase's sankey drills from both kinds of element; egret's branch passed no
              // handler at all — the third chart in a row with that gap. An EDGE names both of its
              // ends, so it filters on both columns; a NODE names only itself, and which column
              // that is depends on which side of the flow it sits on.
              onPointClick={onValueClick && sankey.source && sankey.target ? (p) => {
                const src = sankey.source as string;
                const tgt = sankey.target as string;
                if (p.dataType === "edge") {
                  const link = p.data as { source?: unknown; target?: unknown } | undefined;
                  if (link?.source == null || link?.target == null) return;
                  onValueClick({ dims: [
                    { col: src, value: String(link.source) },
                    { col: tgt, value: String(link.target) },
                  ] }, p.x, p.y);
                  return;
                }
                if (p.dataType !== "node" || p.name == null) return;
                // A node that anything FLOWS INTO is a value of the target column; one that only
                // emits is a value of the source column. Metabase picks the same way, and it
                // matters: the two columns can hold different vocabularies.
                const si = columns.findIndex((c) => c.name === src);
                const ti = columns.findIndex((c) => c.name === tgt);
                const hasInputs = ti >= 0 && rows.some((r) => String(r[ti] ?? "") === p.name);
                const col = hasInputs ? tgt : src;
                if ((hasInputs ? ti : si) < 0) return;
                onValueClick({ dims: [{ col, value: p.name }] }, p.x, p.y);
              } : undefined}
            />}
      </div>
    );
  }

  return null;
}
