import { type VizType, type ChartParams, isCartesianViz } from "@/api/charts";
import type { ColumnFormat, Rule } from "@/components/chart/format-types";
import type { TableState } from "@/api/charts";
import { TableColumnsSection } from "./TableColumnsSection";
import { FormatSection } from "./FormatSection";
import { ConditionalFormattingSection } from "./ConditionalFormattingSection";
import { SectionLabel } from "./SectionLabel";

export interface ResultColumn { name: string; type: string }

/**
 * Which result columns a viz offers for number formatting and conditional colouring.
 * In a pivot only the value cells; for cartesian only the plotted metrics; for scalar the
 * single metric; pie has no column formatting; everything else offers all columns.
 */
export function styleColumnsFor(
  vizType: VizType,
  params: ChartParams,
  resultColumns: ResultColumn[],
): ResultColumn[] {
  if (vizType === "pivot_table") {
    // Dimensions as well as measures. `PivotTable` has always read `column_title` for its
    // row-dimension headers and `rowsToCsv` for every column, but only measures were offered here
    // — so renaming a dimension was implemented and unreachable. It also gates date formatting,
    // without which a month bucket renders as the raw `2026-01-01` instead of `January 2026`.
    const named = new Set([...(params.rows ?? []), ...(params.cols ?? []), ...(params.metrics ?? [])]);
    return resultColumns.filter((c) => named.has(c.name));
  }
  if (isCartesianViz(vizType)) {
    const yMetrics = params.cartesian?.yMetrics ?? [];
    return resultColumns.filter((c) => yMetrics.includes(c.name));
  }
  if (vizType === "scalar") return resultColumns.filter((c) => c.name === params.scalar?.metric);
  if (vizType === "pie") return [];
  return resultColumns;
}

/**
 * The display controls that belong to no single viz: table column layout, number
 * formatting and conditional formatting. They edit settings the shell owns
 * (`column_settings`, `conditional_formatting`, table state), so both the builder's
 * Display tab and the dashboard's per-card override editor compose them.
 */
export function SharedDisplaySections({
  vizType, resultColumns, styleColumns, tableState, columnSettings, conditionalFormatting,
  onTableState, onColumnSettings, onConditionalFormatting, only,
}: {
  vizType: VizType;
  resultColumns: ResultColumn[];
  styleColumns: ResultColumn[];
  tableState: TableState;
  columnSettings: Record<string, ColumnFormat>;
  conditionalFormatting: Rule[];
  onTableState: (next: TableState) => void;
  onColumnSettings: (next: Record<string, ColumnFormat>) => void;
  onConditionalFormatting: (next: Rule[]) => void;
  /** Which half to render. Metabase splits these across two tabs (Columns | Conditional
   * Formatting) and egret stacked all of it in one column, which is why the pivot's panel — the
   * longest in the app — read as cut off. Callers that want the old single column omit this and
   * get everything, so the dashboard's card editor is unchanged. */
  only?: "format" | "conditional";
}) {
  const showFormat = only !== "conditional";
  const showConditional = only !== "format";

  if (resultColumns.length === 0) {
    // Name what THIS viz needs, not a generic "column formatting" sentence — for a table that
    // sat exactly where the column list belongs.
    const empty =
      vizType === "table" || vizType === "object"
        ? "Pick columns and run the query — they appear here once there is data."
        : vizType === "pivot_table"
          ? "Pick rows, columns and a metric, then run the query."
          : "Run the chart to format its values.";
    return <p className="text-xs text-muted-foreground">{empty}</p>;
  }
  return (
    <>
      {showFormat && vizType === "table" && (
        <div className="space-y-1.5">
          <SectionLabel>Columns</SectionLabel>
          <TableColumnsSection columns={resultColumns} value={tableState} onChange={onTableState} columnSettings={columnSettings} />
        </div>
      )}
      {showFormat && styleColumns.length > 0 && (
        <div className="space-y-1.5">
          <SectionLabel>Formatting</SectionLabel>
          <FormatSection columns={styleColumns} value={columnSettings} onChange={onColumnSettings}
            allowMiniBar={vizType === "table"} />
        </div>
      )}
      {showConditional && (vizType === "table" || vizType === "pivot_table") && (
        <div className="space-y-1.5">
          <SectionLabel>Conditional formatting</SectionLabel>
          <ConditionalFormattingSection columns={styleColumns} rules={conditionalFormatting}
            isPivot={vizType === "pivot_table"} onChange={onConditionalFormatting} />
        </div>
      )}
    </>
  );
}
