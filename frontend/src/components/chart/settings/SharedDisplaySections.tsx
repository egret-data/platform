import { type VizType, type ChartParams, isCartesianViz } from "@/api/charts";
import type { ColumnFormat, Rule } from "@/components/chart/format-types";
import type { TableState } from "@/api/charts";
import { TableColumnsSection, type QueryFieldGroup } from "./TableColumnsSection";
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
    // The plotted metrics AND the axis/series dimensions. Only measures were offered, so a date
    // on the x-axis had no format to set — and the axis labels therefore always drew the raw
    // bucket ("2026-01-01" for what is really January 2026). Same pairing as the pivot: the
    // renderer could format it and the panel gave nowhere to say so.
    const named = new Set([
      ...(params.cartesian?.yMetrics ?? []),
      ...(params.cartesian?.xAxis ? [params.cartesian.xAxis] : []),
      ...(params.cartesian?.series ? [params.cartesian.series] : []),
    ]);
    return resultColumns.filter((c) => named.has(c.name));
  }
  if (vizType === "scalar") return resultColumns.filter((c) => c.name === params.scalar?.metric);
  if (vizType === "pie") {
    // Its dimensions AND its metric. Returning [] hid a section the pie's own renderer had always
    // been reading: `metricFormatter` resolves `columnSettings[metric]` for every label, tooltip
    // and the centre total, and the slice names resolve their dimension's format the same way — so
    // both were implemented with nowhere to say so. A month breakout drew "2026-01-01" and the
    // total drew a bare number, with no currency, prefix or decimals reachable from the chart.
    const named = new Set([
      ...(params.pie?.dimensions ?? []),
      ...(params.pie?.metric ? [params.pie.metric] : []),
    ]);
    return resultColumns.filter((c) => named.has(c.name));
  }
  if (vizType === "map") {
    // ITS METRIC ONLY — the one viz below this line that narrows rather than threads.
    //
    // The bare `return resultColumns` beneath handed a Formatting accordion to every result column
    // on a funnel, a box plot, a sankey and a map, while all four builders read `columnSettings`
    // for the metric alone. The first three are fixed by THREADING the dimension's format into the
    // label they draw, because that is what Metabase does — `FunnelNormal`'s `formatDimension`,
    // `echarts/boxplot/model`'s `createXAxisFormatter`, `sankey/model/formatters`' `node`. The map
    // is the exception on both counts.
    //
    // METABASE FORMATS NOTHING HERE. `ChoroplethMap.tsx`'s only `settings.column?.(…)` call is on
    // the METRIC, for the legend; the region cell goes through `getCanonicalRowKey` (lowercase, and
    // state/country names to codes) and the name actually PAINTED is the geojson feature's own
    // `region_name` property. `Map.tsx` passes `showColumnSetting` on neither its dimension nor its
    // metric, so its sidebar offers no per-column format widget at all.
    //
    // AND EGRET COULD NOT THREAD IT IF IT WANTED TO. `MapChart` builds each datum as
    // `{ name: normalizeKey(cell) }` and ECharts matches that string against the registered
    // geojson's `properties.key` to decide which shape to shade. A prefix or a date style there
    // does not restyle a label — it breaks the match and the region stops being drawn. There is no
    // second place the raw region text appears; the tooltip's name is the matched key.
    //
    // THE METRIC STAYS, which is a deliberate divergence from Metabase's "no widget at all": egret's
    // map really does format its metric — `MapChart`'s `fmt` feeds both `itemTooltip` and the
    // `visualMap` range labels, and `options/map.ts` records that being added on purpose — so
    // withdrawing that control would remove a setting that IS honoured. That is this same defect in
    // the mirror, and the rule being copied from Metabase is which columns can take effect, not
    // which widgets it happens to draw.
    return resultColumns.filter((c) => c.name === params.map?.metric);
  }
  return resultColumns;
}

/**
 * Which columns a CONDITIONAL FORMATTING rule may target.
 *
 * Not the same set as `styleColumnsFor`. A pivot colours its value cells only — `buildConditional`
 * is keyed by metric — so offering its dimensions would mean a rule that saves, reads back, and
 * colours nothing. Number formatting is different: a dimension header genuinely does format, which
 * is why the two lists diverge.
 */
export function ruleColumnsFor(
  vizType: VizType,
  params: ChartParams,
  resultColumns: ResultColumn[],
): ResultColumn[] {
  if (vizType === "pivot_table") {
    const metrics = new Set(params.metrics ?? []);
    return resultColumns.filter((c) => metrics.has(c.name));
  }
  return styleColumnsFor(vizType, params, resultColumns);
}

/**
 * The display controls that belong to no single viz: table column layout, number
 * formatting and conditional formatting. They edit settings the shell owns
 * (`column_settings`, `conditional_formatting`, table state), so both the builder's
 * Display tab and the dashboard's per-card override editor compose them.
 */
export function SharedDisplaySections({
  vizType, resultColumns, styleColumns, tableState, columnSettings, conditionalFormatting,
  onTableState, onColumnSettings, onConditionalFormatting, only, ruleColumns, queryFields,
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
  /** Columns a rule may target; defaults to `styleColumns`. A pivot narrows it to its measures —
   * see `ruleColumnsFor`. */
  ruleColumns?: ResultColumn[];
  /** The query's own field lists, folded behind the table column list's `Add or remove columns`.
   * Passed only by the builder route: a dashboard card can override how a column is DRAWN but
   * cannot change what its query selects, so `CardVizPanel` leaves this out and the affordance is
   * simply absent there. */
  queryFields?: QueryFieldGroup[];
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
          <TableColumnsSection columns={resultColumns} value={tableState} onChange={onTableState}
            columnSettings={columnSettings} onColumnSettings={onColumnSettings} queryFields={queryFields} />
        </div>
      )}
      {/* NOT for the table. Its formattable columns ARE its result columns, so a second accordion
          under this one printed every column's name a third time and put its rename two lists away
          from its visibility — the whole defect PR B closes. The table's formatting moved into the
          row's drill-in above. Every other viz keeps this section, because for them `styleColumns`
          is a genuine SUBSET of the result (a pivot's measures, a cartesian's plotted metrics and
          axis) and so is a different list, not a duplicate one. */}
      {showFormat && vizType !== "table" && styleColumns.length > 0 && (
        <div className="space-y-1.5">
          <SectionLabel>Formatting</SectionLabel>
          {/* `false`, flatly, on BOTH. This branch is now every viz EXCEPT the table, and the
              table was the only one that drew a mini bar or read "Display as" — the old
              `vizType === "table"` here would read as a live condition that is in fact never true.
              The table's own mini-bar switch and Display-as select are in the drill-in above, where
              `ColumnSettingsPopover` defaults both on.

              `allowViewAs` is the newer of the two and closes the identical defect: `view_as`,
              `link_text` and `link_url` are read by `ChartTable.renderTableCell` and by nothing
              else, so on every viz below this line the select and its two template boxes saved a
              value that painted nowhere. See `ColumnFormatFields`. */}
          <FormatSection columns={styleColumns} value={columnSettings} onChange={onColumnSettings}
            allowMiniBar={false} allowViewAs={false} />
        </div>
      )}
      {showConditional && (vizType === "table" || vizType === "pivot_table") && (
        <div className="space-y-1.5">
          <SectionLabel>Conditional formatting</SectionLabel>
          <ConditionalFormattingSection columns={ruleColumns ?? styleColumns} rules={conditionalFormatting}
            isPivot={vizType === "pivot_table"} onChange={onConditionalFormatting} />
        </div>
      )}
    </>
  );
}
