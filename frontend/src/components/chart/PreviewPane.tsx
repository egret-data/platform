import type { ReactNode, Ref } from "react";
import type { LucideIcon } from "lucide-react";
import type { AnyChartParams, TableState, VizType } from "@/api/charts";
import type { ColumnFormat, Rule } from "@/components/chart/format-types";
import type { DrillContext } from "@/components/chart/drill";
import { ChartRenderer } from "./ChartRenderer";
import { ChartErrorBoundary } from "./ChartErrorBoundary";
import { ChartTable } from "./ChartTable";
import { PivotTable } from "./pivot/PivotTable";
import { ChartEmptyState, ChartLoadingState, ChartErrorState } from "./ChartPreviewStates";

interface ResultColumn { name: string; type: string }

interface PivotProps {
  rowDims: string[];
  colDims: string[];
  metrics: string[];
  showRowTotals: boolean;
  showColTotals: boolean;
  condense: boolean;
  collapsed: Set<string>;
  sortDims: Record<string, "asc" | "desc">;
  onToggleCollapse: (path: string) => void;
  onCycleSort: (dim: string) => void;
}

interface TableProps {
  state: TableState;
  onStateChange: (next: TableState) => void;
  dimensionColumns: string[];
}

interface Props {
  viz: VizType;
  /** Either stored shape; ChartRenderer resolves the config through the descriptor. */
  params: AnyChartParams;
  columns: ResultColumn[];
  rows: unknown[][];
  metricTypes: Record<string, string>;
  columnSettings: Record<string, ColumnFormat>;
  conditionalFormatting: Rule[];

  isRunning: boolean;
  error: string | null;
  /** A result is on screen; false means show the empty state instead. */
  finished: boolean;
  onRetry: () => void;
  onCellDrill: (ctx: DrillContext, x: number, y: number) => void;

  emptyIcon?: LucideIcon;
  emptyTitle: string;
  emptyHint: string;

  /** Supplied only for `pivot_table`; only for `table`. */
  pivot?: PivotProps;
  table?: TableProps;
  /** Wraps the rendered result — the export menu screenshots this node. */
  containerRef?: Ref<HTMLDivElement>;
  /** Rendered above the preview (filter chips, SQL disclosure…). */
  children?: ReactNode;
}

/**
 * The preview half of the studio: the chart, or the state standing in for it. It renders
 * whichever of the three surfaces the viz needs — pivot, table, or a chart — and knows
 * nothing else about any viz type; the shell hands it the config-derived props.
 */
export function PreviewPane({
  viz, params, columns, rows, metricTypes, columnSettings, conditionalFormatting,
  isRunning, error, finished, onRetry, onCellDrill,
  emptyIcon, emptyTitle, emptyHint, pivot, table, containerRef, children,
}: Props) {
  return (
    <>
      {children}
      {error && <ChartErrorState message={error} onRetry={onRetry} />}
      {isRunning && <ChartLoadingState />}

      <div ref={containerRef} className="contents">
        {finished && viz === "pivot_table" && pivot && (
          <PivotTable
            input={{
              columns: columns.map((c) => c.name),
              rows,
              rowDims: pivot.rowDims, colDims: pivot.colDims, metrics: pivot.metrics,
              showRowTotals: pivot.showRowTotals, showColTotals: pivot.showColTotals,
              condense: pivot.condense, collapsed: pivot.collapsed, sortDims: pivot.sortDims,
            }}
            columnSettings={columnSettings}
            conditionalFormatting={conditionalFormatting}
            metricTypes={metricTypes}
            onToggleCollapse={pivot.onToggleCollapse}
            onCycleSort={pivot.onCycleSort}
            onCellDrill={onCellDrill}
          />
        )}
        {finished && viz === "table" && table && (
          <ChartTable columns={columns} rows={rows}
            columnSettings={columnSettings} conditionalFormatting={conditionalFormatting}
            tableState={table.state} onTableStateChange={table.onStateChange}
            dimensionColumns={table.dimensionColumns}
            onCellDrill={onCellDrill} />
        )}
        {finished && viz !== "table" && viz !== "pivot_table" && (
          <ChartErrorBoundary resetKey={`${viz}:${columns.length}:${rows.length}`}>
            {/* ChartRenderer fills its parent; the preview owns the height + frame
                (dashboard cards size it to the grid cell instead).

                `flex-1`, not a viewport fraction. This was `h-[60vh]`, which at a 920px
                viewport is 552px while the studio's main column already offers ~783px — a
                third of the available height thrown away on every chart type, every time,
                and the direct cause of a three-ring pie's rings measuring 47px against
                Metabase's 69px. The chain that makes this resolve: the route's main column
                is `flex flex-1 flex-col` inside `flex h-full min-h-0`, so it has a definite
                height, and `display: contents` on the wrapper just above means THIS div is
                the flex item that grows into it. `min-h-` is deliberately not doing the
                sizing here — a `h-full` child under a bare `min-h-` parent resolves to 0
                (the chart-studio bug this program already hit once); the floor below is only
                a floor, so a short window still gets a usable chart. */}
            <div data-chart-card className="min-h-[20rem] flex-1 rounded-lg border p-2">
              {/* `onValueClick` is what makes a chart clickable at all: ChartRenderer guards every
                  chart type's click handler on it, so leaving it off meant NO ECharts chart was
                  ever drillable in the editor — pie, bar, line, gauge, waterfall, all of them.
                  The two table surfaces above already had `onCellDrill`; this is the same gesture,
                  and the dashboard has always passed it (DashboardView), which is why the gap only
                  showed up here. */}
              <ChartRenderer vizType={viz} params={params} columns={columns} rows={rows} metricTypes={metricTypes}
                onValueClick={onCellDrill} />
            </div>
          </ChartErrorBoundary>
        )}
      </div>

      {!finished && !isRunning && !error && (
        <ChartEmptyState icon={emptyIcon} title={emptyTitle} hint={emptyHint} />
      )}
    </>
  );
}
