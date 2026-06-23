import { type ReactNode } from "react";
import { type VizType, type ChartParams, isCartesianViz } from "@/api/charts";
import { cartesianSeriesNames } from "@/components/chart/echarts/options/cartesian";
import { CartesianDisplaySection } from "./CartesianDisplaySection";
import { PieDisplaySection } from "./PieDisplaySection";
import { TableColumnsSection } from "./TableColumnsSection";
import { FormatSection } from "./FormatSection";
import { ConditionalFormattingSection } from "./ConditionalFormattingSection";

interface ResultColumn { name: string; type: string }

function SectionLabel({ children }: { children: ReactNode }) {
  return <div className="border-b pb-1 text-xs font-medium uppercase tracking-wide text-muted-foreground">{children}</div>;
}

interface Props {
  vizType: VizType;
  params: ChartParams;
  columns: ResultColumn[];
  rows?: unknown[][];
  onChange: (next: ChartParams) => void;
}

/**
 * The chart builder's "Display" tab section composition, extracted so the same
 * display-config UI (cartesian / pie / table columns / formatting / conditional
 * formatting) can be reused outside ChartExplore (e.g. per-card viz overrides on
 * a dashboard). It reads the current display config from `params`, derives the
 * same auxiliary values ChartExplore used (series names, pie slices, style
 * columns), and emits a full ChartParams replacing only the edited field.
 */
export function ChartSettingsSections({ vizType, params, columns, rows = [], onChange }: Props) {
  const cartesian = params.cartesian ?? { yMetrics: [] };
  const pie = params.pie ?? {};
  const tableState = params.tableState ?? {};
  const columnSettings = params.columnSettings ?? {};
  const conditionalFormatting = params.conditionalFormatting ?? [];

  // Result columns offered to formatting (mirrors ChartExplore's resultColumns).
  const resultColumns = columns;

  // In a pivot, only value (metric) cells are formatted/colored. For cartesian,
  // only the plotted metrics; for scalar, the single metric; pie has no column
  // formatting UI. Everything else: all columns. (Mirrors ChartExplore.)
  const styleColumns =
    vizType === "pivot_table" ? resultColumns.filter((c) => (params.metrics ?? []).includes(c.name))
    : isCartesianViz(vizType) ? resultColumns.filter((c) => cartesian.yMetrics.includes(c.name))
    : vizType === "scalar" ? resultColumns.filter((c) => c.name === params.scalar?.metric)
    : vizType === "pie" ? []
    : resultColumns;

  // Series the cartesian chart will draw — used for per-series colour/rename.
  // Empty until results (rows) arrive, mirroring ChartExplore's `results.data`
  // gate so the per-series controls only appear once the chart has been run.
  const hasRows = rows.length > 0;
  const cartesianSeries = isCartesianViz(vizType) && hasRows
    ? cartesianSeriesNames(columns, rows, cartesian)
    : [];

  // Distinct slice (dimension) values for the pie per-slice colour controls.
  const dimIdx = columns.findIndex((c) => c.name === pie.dimension);
  const pieSlices = vizType === "pie" && pie.dimension && dimIdx !== -1
    ? Array.from(new Set(
        rows
          .map((r) => String(r[dimIdx] ?? ""))
          .filter((v) => v !== ""),
      ))
    : [];

  return (
    <>
      {isCartesianViz(vizType) && (
        <div className="space-y-1">
          <SectionLabel>Options</SectionLabel>
          <CartesianDisplaySection
            kind={vizType}
            config={cartesian}
            onChange={(patch) => onChange({ ...params, cartesian: { ...cartesian, ...patch } })}
            seriesNames={cartesianSeries}
          />
        </div>
      )}
      {vizType === "pie" && (
        <div className="space-y-1">
          <SectionLabel>Options</SectionLabel>
          <PieDisplaySection
            config={pie}
            onChange={(patch) => onChange({ ...params, pie: { ...pie, ...patch } })}
            sliceNames={pieSlices}
          />
        </div>
      )}

      {resultColumns.length === 0 ? (
        <p className="text-xs text-muted-foreground">Run the chart to configure column formatting.</p>
      ) : (
        <>
          {vizType === "table" && (
            <div className="space-y-1.5">
              <SectionLabel>Columns</SectionLabel>
              <TableColumnsSection columns={resultColumns} value={tableState} onChange={(next) => onChange({ ...params, tableState: next })} />
            </div>
          )}
          {styleColumns.length > 0 && (
            <div className="space-y-1.5">
              <SectionLabel>Formatting</SectionLabel>
              <FormatSection columns={styleColumns} value={columnSettings} onChange={(value) => onChange({ ...params, columnSettings: value })} />
            </div>
          )}
          {(vizType === "table" || vizType === "pivot_table") && (
            <div className="space-y-1.5">
              <SectionLabel>Conditional formatting</SectionLabel>
              <ConditionalFormattingSection columns={styleColumns} rules={conditionalFormatting} isPivot={vizType === "pivot_table"} onChange={(rules) => onChange({ ...params, conditionalFormatting: rules })} />
            </div>
          )}
        </>
      )}
    </>
  );
}
