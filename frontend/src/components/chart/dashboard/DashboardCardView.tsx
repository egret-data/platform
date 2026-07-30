import { useRef } from "react";
import type { DashboardCard, DashboardParameter } from "@/api/dashboards";
import type { Chart, ChartFilter } from "@/api/charts";
import { useChartData, type ChartData } from "@/api/useChartData";
import { useDashboardCardData } from "@/api/useSavedChartData";
import { ChartRenderer } from "@/components/chart/ChartRenderer";
import { ChartErrorBoundary } from "@/components/chart/ChartErrorBoundary";
import { ChartExportMenu } from "@/components/chart/ChartExportMenu";
import { shellSettings } from "@/components/chart/viz/renderConfig";
import type { DrillContext } from "@/components/chart/drill";
import { TextCard } from "./TextCard";
import { HeadingCard } from "./HeadingCard";
import { LinkCard } from "./LinkCard";
import { applyVizOverride, interpolateParams } from "./params";
import { AutoHeightMeasure } from "./AutoHeightMeasure";

function asString(v: unknown): string | undefined {
  return typeof v === "string" ? v : undefined;
}

type ChartCardTileProps = { ws: string; dashboardId: string; card: DashboardCard; chart: Chart | undefined; refreshNonce?: number; paramFilters?: ChartFilter[]; paramTemporal?: Record<string, string>; onValueClick?: (ctx: DrillContext, x: number, y: number) => void; blocked?: boolean; editing?: boolean; autoHeight?: boolean; onAutoHeight?: (rows: number) => void; rowHeight?: number };

/**
 * View-mode chart tile: fetches via the object-scoped card-data endpoint
 * (`POST /dashboards/{id}/cards/{card_id}/data`), which runs the card's SAVED
 * chart query server-side. This is the shared/read path — an object guest needs
 * only the dashboard grant, no `query:run` workspace permission.
 */
function ChartCardTileView(props: ChartCardTileProps) {
  const { ws, dashboardId, card, chart, refreshNonce, paramFilters, paramTemporal, blocked } = props;
  // Pass the resolved dashboard-parameter filters + temporal-unit overrides so the
  // card-data endpoint applies them server-side (view-mode filtering + re-grid). A
  // changed filter or unit mints a fresh query key.
  const data = useDashboardCardData(ws, dashboardId, card.id, refreshNonce, !blocked && !!chart, paramFilters ?? [], paramTemporal ?? {});
  return <ChartCardTileBody {...props} data={data} />;
}

/**
 * Edit-mode chart tile: runs the live draft spec via `useChartData` (a single
 * `POST /charts/run` request) so unsaved draft cards (non-persisted ids) and
 * live dashboard-parameter injection still render while editing — the
 * card-data endpoint only knows the persisted card + its saved query.
 */
function ChartCardTileEdit(props: ChartCardTileProps) {
  const { ws, chart, refreshNonce, blocked } = props;
  const data = useChartData(ws, chart ?? null, refreshNonce, !blocked);
  return <ChartCardTileBody {...props} data={data} />;
}

function ChartCardTileBody({ card, chart, onValueClick, blocked, editing, autoHeight, onAutoHeight, rowHeight, data }: ChartCardTileProps & { data: ChartData }) {
  // Ref the render-only container (NOT the whole tile), so the PNG export targets the
  // chart's <svg> and not the export menu's own Download icon. Also keeps PNG hidden for
  // table/pivot cards (their render container has no <svg>).
  const chartRef = useRef<HTMLDivElement>(null);
  const titleOverride = asString(card.options.title);          // undefined → use chart name; "" → hide
  const headerText = titleOverride || chart?.name || "";
  const showHeader = titleOverride !== "" && headerText !== "";
  const canExport = !editing && !blocked && !data.isLoading && !data.error && (data.columns?.length ?? 0) > 0;
  if (!chart) return <div className="flex h-full items-center justify-center text-sm text-muted-foreground">Chart not found</div>;
  // Computed once the chart is known to exist, so both the export menu (which needs the
  // rename that's actually in effect) and the render branch below share the same override.
  const eff = applyVizOverride(chart, card.options.vizOverride as Record<string, unknown> | undefined);
  return (
    <div className="group relative flex h-full flex-col">
      {canExport && (
        <div className="dash-nodrag absolute right-1 top-1 z-10 opacity-0 transition-opacity group-hover:opacity-100">
          <ChartExportMenu title={headerText || chart?.name} columns={data.columns} rows={data.rows}
            columnSettings={shellSettings(eff.params).columnSettings} containerRef={chartRef} />
        </div>
      )}
      {showHeader && <div className="px-4 pt-3 pb-2 text-sm font-medium text-foreground">{headerText}</div>}
      <div ref={chartRef} className="min-h-0 flex-1 overflow-auto px-2 pb-2">
        {(() => {
          const body = blocked ? <div className="flex h-full items-center justify-center px-3 text-center text-sm text-muted-foreground">Set the required filters to load this card</div>
            : data.error ? <div className="text-sm text-destructive">{data.error}</div>
            : data.isLoading ? <div className="text-sm text-muted-foreground">Loading…</div>
            : (() => {
              return (
                <ChartErrorBoundary resetKey={`${chart.id}:${eff.viz_type}:${data.columns?.length ?? 0}:${data.rows?.length ?? 0}`}>
                  {/* `gridCols` lets a viz scale a size ceiling to the card's share of the grid —
                      the scalar's number is the one that uses it today. Only a dashboard card
                      knows this; the studio preview has no grid and falls back to its own cap. */}
                  <ChartRenderer vizType={eff.viz_type} params={eff.params} columns={data.columns} rows={data.rows} onValueClick={onValueClick} gridCols={card.size_x} />
                </ChartErrorBoundary>
              );
            })();
          return autoHeight && onAutoHeight ? <AutoHeightMeasure rowHeight={rowHeight ?? 0} onRows={onAutoHeight}>{body}</AutoHeightMeasure> : body;
        })()}
      </div>
    </div>
  );
}

/** Render one dashboard card by its card_type. */
export function DashboardCardView({ ws, dashboardId, card, chart, refreshNonce, parameters, paramValues, paramFilters, paramTemporal, onValueClick, blocked, editing, autoHeight, onAutoHeight, rowHeight, onEditText, autoFocusText }: { ws: string; dashboardId: string; card: DashboardCard; chart: Chart | undefined; refreshNonce?: number; parameters?: DashboardParameter[]; paramValues?: Record<string, unknown>; paramFilters?: ChartFilter[]; paramTemporal?: Record<string, string>; onValueClick?: (ctx: DrillContext, x: number, y: number) => void; blocked?: boolean; editing?: boolean; autoHeight?: boolean; onAutoHeight?: (rows: number) => void; rowHeight?: number; onEditText?: (text: string) => void; autoFocusText?: boolean }) {
  const wrap = (body: React.ReactNode) => (autoHeight && onAutoHeight ? <AutoHeightMeasure rowHeight={rowHeight ?? 0} onRows={onAutoHeight}>{body}</AutoHeightMeasure> : body);
  if (card.card_type === "text") {
    const raw = asString(card.options.text) ?? "";
    const display = editing ? raw : interpolateParams(raw, parameters ?? [], paramValues ?? {});
    return wrap(<TextCard text={display} editing={editing} autoFocus={autoFocusText} onChange={onEditText} />);
  }
  if (card.card_type === "heading") {
    const raw = asString(card.options.text) ?? "";
    const display = editing ? raw : interpolateParams(raw, parameters ?? [], paramValues ?? {});
    return wrap(<HeadingCard text={display} editing={editing} autoFocus={autoFocusText} onChange={onEditText} />);
  }
  if (card.card_type === "link") return wrap(<LinkCard url={asString(card.options.url)} label={asString(card.options.label)} editing={editing} />);
  const tileProps = { ws, dashboardId, card, chart, refreshNonce, paramFilters, paramTemporal, onValueClick, blocked, editing, autoHeight, onAutoHeight, rowHeight };
  // View mode reads through the object-scoped card-data endpoint; edit mode runs
  // the live draft spec directly so unsaved draft cards + param injection still render.
  return editing ? <ChartCardTileEdit {...tileProps} /> : <ChartCardTileView {...tileProps} />;
}
