// src/components/chart/ChartRenderer.tsx
import type { ChartParams, VizType } from "@/api/charts";
import { isCartesianViz } from "@/api/charts";
import type { DrillContext } from "@/components/chart/drill";
import { EChartsRenderer } from "@/components/chart/echarts/EChartsRenderer";
import { buildCartesianOption } from "@/components/chart/echarts/options/cartesian";
import { buildPieOption } from "@/components/chart/echarts/options/pie";
import { buildGaugeOption } from "@/components/chart/echarts/options/gauge";
import { buildFunnelOption } from "@/components/chart/echarts/options/funnel";
import { buildWaterfallOption } from "@/components/chart/echarts/options/waterfall";
import { buildBoxplotOption } from "@/components/chart/echarts/options/boxplot";
import { buildSankeyOption } from "@/components/chart/echarts/options/sankey";
import { ChartTable, type ResultColumn } from "@/components/chart/ChartTable";
import { PivotTable } from "@/components/chart/pivot/PivotTable";
import { BigNumber } from "@/components/chart/BigNumber";
import { ProgressChart } from "@/components/chart/ProgressChart";
import { SmartScalarChart } from "@/components/chart/SmartScalarChart";
import { ObjectDetail } from "@/components/chart/ObjectDetail";
import { MapChart } from "@/components/chart/MapChart";

const EmptyState = () => (
  <div className="flex h-full items-center justify-center text-sm text-muted-foreground">No data</div>
);

const VIZ_NAMES: Partial<Record<VizType, string>> = {
  bar: "Bar", row: "Row bar", line: "Line", area: "Area", scatter: "Scatter", combo: "Combo",
  pie: "Pie", gauge: "Gauge", funnel: "Funnel", waterfall: "Waterfall", boxplot: "Box plot", sankey: "Sankey",
};

/** One-line summary read by assistive tech (the chart canvas is otherwise opaque). */
function ariaSummary(vizType: VizType, rows: number, detail: string): string {
  const name = VIZ_NAMES[vizType] ?? vizType;
  const what = detail.trim() ? `: ${detail.trim()}` : "";
  return `${name} chart${what} (${rows} data ${rows === 1 ? "point" : "points"})`;
}

export interface ChartRendererProps {
  vizType: VizType;
  params: ChartParams;
  columns: ResultColumn[];
  rows: unknown[][];
  /** metric_name -> data_type, for metric value formatting. Optional; defaults to {}. */
  metricTypes?: Record<string, string>;
  onValueClick?: (ctx: DrillContext, x: number, y: number) => void;
}

export function ChartRenderer({ vizType, params, columns, rows, metricTypes = {}, onValueClick }: ChartRendererProps) {
  const cartesian = params.cartesian ?? { yMetrics: [] };
  const pie = params.pie ?? {};
  const scalar = params.scalar ?? {};
  const progress = params.progress ?? {};
  const gauge = params.gauge ?? {};
  const funnel = params.funnel ?? {};
  const waterfall = params.waterfall ?? {};
  const smart = params.smartscalar ?? {};
  const box = params.boxplot ?? {};
  const sankey = params.sankey ?? {};
  const columnSettings = params.columnSettings ?? {};
  const conditionalFormatting = params.conditionalFormatting ?? [];
  const tableState = params.tableState ?? {};
  const dimensionColumns = params.spec.metrics.length > 0 ? params.spec.dimensions : [];

  if (vizType === "object") {
    return <ObjectDetail columns={columns} rows={rows} columnSettings={columnSettings} />;
  }

  if (vizType === "map") {
    return <MapChart columns={columns} rows={rows} params={params} />;
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
          rowDims: params.rows ?? [],
          colDims: params.cols ?? [],
          metrics: params.metrics ?? [],
          showRowTotals: params.showRowTotals ?? true,
          showColTotals: params.showColTotals ?? true,
          condense: params.condense ?? false,
          collapsed: new Set(params.collapsedRows ?? []),
          sortDims: params.pivotSort ?? {},
        }}
        columnSettings={columnSettings}
        conditionalFormatting={conditionalFormatting}
        metricTypes={metricTypes}
        onCellDrill={onValueClick}
      />
    );
  }

  if (isCartesianViz(vizType)) {
    return (
      <div className="h-full w-full">
        {rows.length === 0
          ? <EmptyState />
          : <EChartsRenderer ariaLabel={ariaSummary(vizType, rows.length, `${(cartesian.yMetrics ?? []).join(", ")}${cartesian.xAxis ? ` by ${cartesian.xAxis}` : ""}`)} option={buildCartesianOption(columns, rows, {
              kind: vizType, xAxis: cartesian.xAxis, xMetric: cartesian.xMetric, yMetrics: cartesian.yMetrics, series: cartesian.series,
              stacked: cartesian.stacked, horizontal: cartesian.horizontal, smooth: cartesian.smooth,
              showLabels: cartesian.showLabels, showLegend: cartesian.showLegend,
              xLabel: cartesian.xLabel, yLabel: cartesian.yLabel, seriesTypes: cartesian.seriesTypes,
              rightAxisMetrics: cartesian.rightAxisMetrics, logScale: cartesian.logScale,
              normalized: cartesian.normalized, goalValue: cartesian.goalValue, goalLabel: cartesian.goalLabel,
              trendLine: cartesian.trendLine, connectNulls: cartesian.connectNulls, step: cartesian.step,
              showStackTotal: cartesian.showStackTotal,
              seriesColors: cartesian.seriesColors, seriesLabels: cartesian.seriesLabels,
              yMin: cartesian.yMin, yMax: cartesian.yMax,
              showMarkers: cartesian.showMarkers, xScale: cartesian.xScale, sort: cartesian.sort,
              columnSettings, metricTypes,
              xAxisUnit: params.spec.temporal_units?.[cartesian.xAxis ?? ""],
            })}
            onPointClick={onValueClick && cartesian.xAxis ? (p) => {
              // NOTE: for an extraction temporal unit (e.g. day_of_week) p.name is the display label
              // ("Mon"), not the raw integer (1), so drill-through filters won't match the column.
              // Tracked as a follow-up; extraction-grouped drill is an unusual path.
              const dims: { col: string; value: unknown }[] = [{ col: cartesian.xAxis as string, value: p.name }];
              if (cartesian.series && p.seriesName != null) dims.push({ col: cartesian.series, value: p.seriesName });
              onValueClick({ dims }, p.x, p.y);
            } : undefined}
          />}
      </div>
    );
  }

  if (vizType === "pie") {
    return (
      <div className="h-full w-full">
        {rows.length === 0
          ? <EmptyState />
          : <EChartsRenderer ariaLabel={ariaSummary("pie", rows.length, `${pie.metric ?? ""}${pie.dimension ? ` by ${pie.dimension}` : ""}`)} option={buildPieOption(columns, rows, {
              dimension: pie.dimension, metric: pie.metric, donut: pie.donut,
              showLegend: pie.showLegend, showLabels: pie.showLabels,
              sliceThreshold: pie.sliceThreshold, sortRows: pie.sortRows,
              showTotal: pie.showTotal, percentVisibility: pie.percentVisibility,
              sliceColors: pie.sliceColors,
              columnSettings, metricTypes,
              dimensionUnit: params.spec.temporal_units?.[pie.dimension ?? ""],
            })}
            onPointClick={onValueClick && pie.dimension ? (p) => onValueClick({ dims: [{ col: pie.dimension as string, value: p.name }] }, p.x, p.y) : undefined}
          />}
      </div>
    );
  }

  if (vizType === "scalar") {
    return (
      <div className="h-full w-full">
        <BigNumber
          columns={columns}
          rows={rows}
          metric={scalar.metric}
          format={scalar.metric ? columnSettings[scalar.metric] : undefined}
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
          format={progress.metric ? columnSettings[progress.metric] : undefined}
        />
      </div>
    );
  }

  if (vizType === "gauge") {
    return (
      <div className="h-full w-full">
        {rows.length === 0
          ? <EmptyState />
          : <EChartsRenderer ariaLabel={ariaSummary("gauge", rows.length, gauge.metric ?? "")} option={buildGaugeOption(columns, rows, {
              metric: gauge.metric, min: gauge.min, max: gauge.max, columnSettings, metricTypes,
            })} />}
      </div>
    );
  }

  if (vizType === "funnel") {
    return (
      <div className="h-full w-full">
        {rows.length === 0
          ? <EmptyState />
          : <EChartsRenderer ariaLabel={ariaSummary("funnel", rows.length, `${funnel.metric ?? ""}${funnel.dimension ? ` by ${funnel.dimension}` : ""}`)} option={buildFunnelOption(columns, rows, {
              dimension: funnel.dimension, metric: funnel.metric, columnSettings, metricTypes,
            })} />}
      </div>
    );
  }

  if (vizType === "waterfall") {
    return (
      <div className="h-full w-full">
        {rows.length === 0
          ? <EmptyState />
          : <EChartsRenderer ariaLabel={ariaSummary("waterfall", rows.length, `${waterfall.metric ?? ""}${waterfall.xAxis ? ` by ${waterfall.xAxis}` : ""}`)} option={buildWaterfallOption(columns, rows, {
              xAxis: waterfall.xAxis, metric: waterfall.metric, showTotal: waterfall.showTotal, columnSettings, metricTypes,
            })} />}
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
        />
      </div>
    );
  }

  if (vizType === "boxplot") {
    return (
      <div className="h-full w-full">
        {rows.length === 0
          ? <EmptyState />
          : <EChartsRenderer ariaLabel={ariaSummary("boxplot", rows.length, `${box.value ?? ""}${box.category ? ` by ${box.category}` : ""}`)} option={buildBoxplotOption(columns, rows, {
              category: box.category, value: box.value, columnSettings, metricTypes,
            })} />}
      </div>
    );
  }

  if (vizType === "sankey") {
    return (
      <div className="h-full w-full">
        {rows.length === 0
          ? <EmptyState />
          : <EChartsRenderer ariaLabel={ariaSummary("sankey", rows.length, `${sankey.metric ?? ""}${sankey.source && sankey.target ? ` from ${sankey.source} to ${sankey.target}` : ""}`)} option={buildSankeyOption(columns, rows, {
              source: sankey.source, target: sankey.target, metric: sankey.metric, columnSettings, metricTypes,
            })} />}
      </div>
    );
  }

  return null;
}
