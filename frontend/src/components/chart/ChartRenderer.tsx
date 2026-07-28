// src/components/chart/ChartRenderer.tsx
import type {
  AnyChartParams, BoxplotConfig, CartesianConfig, FunnelConfig, GaugeConfig,
  PieConfig, ProgressConfig, SankeyConfig, ScalarConfig, SmartScalarConfig, VizType,
} from "@/api/charts";
import type { PivotConfig } from "@/components/chart/viz/pivot";
import { isCartesianViz } from "@/api/charts";
import { vizConfig, shellSettings } from "@/components/chart/viz/renderConfig";
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

export interface ChartRendererProps {
  vizType: VizType;
  /** Either stored shape — the config is resolved through the viz's descriptor. */
  params: AnyChartParams;
  columns: ResultColumn[];
  rows: unknown[][];
  /** metric_name -> data_type, for metric value formatting. Optional; defaults to {}. */
  metricTypes?: Record<string, string>;
  onValueClick?: (ctx: DrillContext, x: number, y: number) => void;
}

export function ChartRenderer({ vizType, params, columns, rows, metricTypes = {}, onValueClick }: ChartRendererProps) {
  // One config, from the viz's own descriptor; each branch below reads it as its type.
  const cfg = vizConfig(vizType, params);
  const cartesian = { yMetrics: [], ...cfg } as CartesianConfig;
  const pie = cfg as PieConfig;
  const scalar = cfg as ScalarConfig;
  const progress = cfg as ProgressConfig;
  const gauge = cfg as GaugeConfig;
  const funnel = cfg as FunnelConfig;
  const waterfall = cfg as { xAxis?: string; metric?: string; showTotal?: boolean };
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
    return <MapChart columns={columns} rows={rows} params={params} metricTypes={metricTypes} />;
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
          rowDims: pivot.rows ?? [],
          colDims: pivot.cols ?? [],
          metrics: pivot.metrics ?? [],
          showRowTotals: pivot.showRowTotals ?? true,
          showColTotals: pivot.showColTotals ?? true,
          condense: pivot.condense ?? false,
          collapsed: new Set(pivot.collapsedRows ?? []),
          sortDims: pivot.sort ?? {},
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
          : <EChartsRenderer option={buildCartesianOption(columns, rows, {
              // Spread rather than list the fields: this used to be a hand-maintained whitelist, and
              // four settings added in one branch were silently dropped between the panel and the
              // chart because nobody remembered to extend it.
              ...cartesian,
              kind: vizType,
              columnSettings,
              metricTypes,
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
          : <EChartsRenderer option={buildPieOption(columns, rows, {
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
          : <EChartsRenderer option={buildGaugeOption(columns, rows, {
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
          : <EChartsRenderer option={buildFunnelOption(columns, rows, {
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
          : <EChartsRenderer option={buildWaterfallOption(columns, rows, {
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
          : <EChartsRenderer option={buildBoxplotOption(columns, rows, {
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
          : <EChartsRenderer option={buildSankeyOption(columns, rows, {
              source: sankey.source, target: sankey.target, metric: sankey.metric, columnSettings, metricTypes,
            })} />}
      </div>
    );
  }

  return null;
}
