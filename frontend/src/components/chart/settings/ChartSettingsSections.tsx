import { type VizType, type ChartParams, isCartesianViz } from "@/api/charts";
import { cartesianSeriesNames } from "@/components/chart/echarts/options/cartesian";
import { CartesianDisplaySection } from "./CartesianDisplaySection";
import { PieDisplaySection } from "./PieDisplaySection";
import { SectionLabel } from "./SectionLabel";
import { SharedDisplaySections, styleColumnsFor } from "./SharedDisplaySections";

interface ResultColumn { name: string; type: string }

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

  // Shared with the builder's Display tab so both offer the same formatting columns.
  const styleColumns = styleColumnsFor(vizType, params, resultColumns);

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

      <SharedDisplaySections
        vizType={vizType}
        resultColumns={resultColumns}
        styleColumns={styleColumns}
        tableState={tableState}
        columnSettings={columnSettings}
        conditionalFormatting={conditionalFormatting}
        onTableState={(next) => onChange({ ...params, tableState: next })}
        onColumnSettings={(value) => onChange({ ...params, columnSettings: value })}
        onConditionalFormatting={(rules) => onChange({ ...params, conditionalFormatting: rules })}
      />
    </>
  );
}
