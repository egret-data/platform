// src/components/chart/ChartRenderer.tsx
import type { ChartParams, VizType } from "@/api/charts";
import { isCartesianViz } from "@/api/charts";
import { EChartsRenderer } from "@/components/chart/echarts/EChartsRenderer";
import { buildCartesianOption } from "@/components/chart/echarts/options/cartesian";
import { buildPieOption } from "@/components/chart/echarts/options/pie";
import { buildGaugeOption } from "@/components/chart/echarts/options/gauge";
import { buildFunnelOption } from "@/components/chart/echarts/options/funnel";
import { buildWaterfallOption } from "@/components/chart/echarts/options/waterfall";
import { buildBoxplotOption } from "@/components/chart/echarts/options/boxplot";
import { buildSankeyOption } from "@/components/chart/echarts/options/sankey";
import { ChartTable } from "@/components/chart/ChartTable";
import { PivotTable } from "@/components/chart/pivot/PivotTable";
import { BigNumber } from "@/components/chart/BigNumber";
import { ProgressChart } from "@/components/chart/ProgressChart";
import { SmartScalarChart } from "@/components/chart/SmartScalarChart";

type Col = { name: string; type: string };

export interface ChartRendererProps {
  vizType: VizType;
  params: ChartParams;
  columns: Col[];
  rows: unknown[][];
  /** metric_name -> data_type, for metric value formatting. Optional; defaults to {}. */
  metricTypes?: Record<string, string>;
}

export function ChartRenderer({ vizType, params, columns, rows, metricTypes = {} }: ChartRendererProps) {
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

  if (vizType === "table") {
    return (
      <ChartTable
        columns={columns}
        rows={rows}
        columnSettings={columnSettings}
        conditionalFormatting={conditionalFormatting}
        tableState={tableState}
        dimensionColumns={dimensionColumns}
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
      />
    );
  }

  if (isCartesianViz(vizType)) {
    return (
      <div className="h-[60vh] rounded-md border p-2">
        {rows.length === 0
          ? <div className="flex h-full items-center justify-center text-sm text-muted-foreground">No data</div>
          : <EChartsRenderer option={buildCartesianOption(columns, rows, {
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
              showMarkers: cartesian.showMarkers, xScale: cartesian.xScale,
              columnSettings, metricTypes,
            })} />}
      </div>
    );
  }

  if (vizType === "pie") {
    return (
      <div className="h-[60vh] rounded-md border p-2">
        {rows.length === 0
          ? <div className="flex h-full items-center justify-center text-sm text-muted-foreground">No data</div>
          : <EChartsRenderer option={buildPieOption(columns, rows, {
              dimension: pie.dimension, metric: pie.metric, donut: pie.donut,
              showLegend: pie.showLegend, showLabels: pie.showLabels,
              sliceThreshold: pie.sliceThreshold, sortRows: pie.sortRows,
              showTotal: pie.showTotal, percentVisibility: pie.percentVisibility,
              sliceColors: pie.sliceColors,
              columnSettings, metricTypes,
            })} />}
      </div>
    );
  }

  if (vizType === "scalar") {
    return (
      <div className="rounded-md border">
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
      <div className="rounded-md border">
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
      <div className="h-[50vh] rounded-md border p-2">
        <EChartsRenderer option={buildGaugeOption(columns, rows, {
          metric: gauge.metric, min: gauge.min, max: gauge.max, columnSettings, metricTypes,
        })} />
      </div>
    );
  }

  if (vizType === "funnel") {
    return (
      <div className="h-[60vh] rounded-md border p-2">
        {rows.length === 0
          ? <div className="flex h-full items-center justify-center text-sm text-muted-foreground">No data</div>
          : <EChartsRenderer option={buildFunnelOption(columns, rows, {
              dimension: funnel.dimension, metric: funnel.metric, columnSettings, metricTypes,
            })} />}
      </div>
    );
  }

  if (vizType === "waterfall") {
    return (
      <div className="h-[60vh] rounded-md border p-2">
        {rows.length === 0
          ? <div className="flex h-full items-center justify-center text-sm text-muted-foreground">No data</div>
          : <EChartsRenderer option={buildWaterfallOption(columns, rows, {
              xAxis: waterfall.xAxis, metric: waterfall.metric, showTotal: waterfall.showTotal, columnSettings, metricTypes,
            })} />}
      </div>
    );
  }

  if (vizType === "smartscalar") {
    return (
      <div className="rounded-md border">
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
      <div className="h-[60vh] rounded-md border p-2">
        {rows.length === 0
          ? <div className="flex h-full items-center justify-center text-sm text-muted-foreground">No data</div>
          : <EChartsRenderer option={buildBoxplotOption(columns, rows, {
              category: box.category, value: box.value, columnSettings, metricTypes,
            })} />}
      </div>
    );
  }

  if (vizType === "sankey") {
    return (
      <div className="h-[60vh] rounded-md border p-2">
        {rows.length === 0
          ? <div className="flex h-full items-center justify-center text-sm text-muted-foreground">No data</div>
          : <EChartsRenderer option={buildSankeyOption(columns, rows, {
              source: sankey.source, target: sankey.target, metric: sankey.metric, columnSettings, metricTypes,
            })} />}
      </div>
    );
  }

  return null;
}
