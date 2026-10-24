import { useRef } from "react";
import { Pencil } from "lucide-react";
import { Link } from "react-router-dom";
import { Button } from "@/components/ui/button";
import type { DashboardCard, DashboardParameter } from "@/api/dashboards";
import type { Chart, ChartFilter } from "@/api/charts";
import { useChartData, type ChartData } from "@/api/useChartData";
import { useDashboardCardData } from "@/api/useSavedChartData";
import { ChartRenderer } from "@/components/chart/ChartRenderer";
import { ChartErrorBoundary } from "@/components/chart/ChartErrorBoundary";
import { ChartExportMenu } from "@/components/chart/ChartExportMenu";
import { shellSettings } from "@/components/chart/viz/renderConfig";
import { getDescriptor } from "@/components/chart/viz/registry";
import type { DrillContext } from "@/components/chart/drill";
import { TextCard } from "./TextCard";
import { HeadingCard } from "./HeadingCard";
import { LinkCard } from "./LinkCard";
import { IFrameCard } from "./IFrameCard";
import { applyVizOverride, interpolateParams } from "./params";
import { AutoHeightMeasure } from "./AutoHeightMeasure";
import { DashboardCardHeader } from "./DashboardCardHeader";
import { dashcardHref } from "./dashcard-href";
import { CardInlineFilters } from "./CardInlineFilters";
import { INLINE_PARAM_BUFFER_PX, INLINE_PARAM_HEADING_BUFFER_PX, inlineFilterGutterClass } from "./inlineParameters";
import { useCollapsedParameterList } from "./useCollapsedParameterList";

function asString(v: unknown): string | undefined {
  return typeof v === "string" ? v : undefined;
}

/** Everything a card needs to draw the dashboard filters it hosts. One object rather than five
 *  props, so the route's call site stays a single expression. Undefined — or an empty
 *  `parameters` — on a card that hosts none, which is every card until an author moves one.
 *
 *  The parameters here are ONLY the ones this card hosts; the filtering itself still reads the
 *  dashboard's full parameter array (see `inlineParameters.ts`). */
export interface InlineFilterProps {
  parameters: DashboardParameter[];
  values: Record<string, unknown>;
  optionsBySlug: Record<string, string[]>;
  onChange: (slug: string, value: unknown) => void;
  onSelect?: (paramId: string) => void;
  activeId?: string | null;
}

type ChartCardTileProps = { ws: string; dashboardId: string; card: DashboardCard; chart: Chart | undefined; refreshNonce?: number; paramFilters?: ChartFilter[]; paramTemporal?: Record<string, string>; onValueClick?: (ctx: DrillContext, x: number, y: number) => void; blocked?: boolean; editing?: boolean; autoHeight?: boolean; onAutoHeight?: (rows: number) => void; rowHeight?: number; inline?: InlineFilterProps };

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

function ChartCardTileBody({ ws, card, chart, onValueClick, blocked, editing, autoHeight, onAutoHeight, rowHeight, inline, data }: ChartCardTileProps & { data: ChartData }) {
  // Ref the render-only container (NOT the whole tile), so the PNG export targets the
  // chart's <svg> and not the export menu's own Download icon. Also keeps PNG hidden for
  // table/pivot cards (their render container has no <svg>).
  const chartRef = useRef<HTMLDivElement>(null);
  const { collapsed, containerRef, reservedRef, listRef } = useCollapsedParameterList(INLINE_PARAM_BUFFER_PX);
  const titleOverride = asString(card.options.title);          // undefined → use chart name; "" → hide
  const headerText = titleOverride || chart?.name || "";
  const showHeader = titleOverride !== "" && headerText !== "";
  const canExport = !editing && !blocked && !data.isLoading && !data.error && (data.columns?.length ?? 0) > 0;
  // Both hang off the header, so both die with it — `dashcardHref` encodes that rule once.
  const href = dashcardHref(ws, card, editing);
  const description = asString(card.options.description);
  const inlineParams = inline?.parameters ?? [];
  // ONE ROW, title left and filters right — Metabase hands the inline list to the visualization's
  // `actionButtons`, which renders it in the title row beside the card menu
  // (DashCardVisualization.tsx:468-487).
  //
  // The gutter is not decoration. W5's hover cluster below is `absolute right-1 top-1` and holds
  // up to TWO glyph buttons, so it contributes NO layout width while covering the row's trailing
  // 78px; `inlineFilterGutterClass` does that arithmetic. Without it the pencil draws on top of
  // the last filter pill, and because it only appears on hover it survives every static review.
  //
  // The row also renders when the title is HIDDEN (`options.title === ""`) but filters are
  // present: they have to live somewhere.
  const headerRow = (showHeader || inlineParams.length > 0) ? (
    <DashboardCardHeader
      text={showHeader ? headerText : ""}
      href={showHeader ? href : undefined}
      description={showHeader ? description : undefined}
      containerRef={containerRef}
      titleRef={reservedRef}
      // ONLY when the card hosts filters. A card that hosts none must render byte-for-byte as it
      // did on W5 — its title is `truncate`d and nothing in the row can reach the cluster, so
      // widening its padding here would be W7 quietly restyling every card on every dashboard.
      gutterClass={inlineParams.length > 0 ? inlineFilterGutterClass((href ? 1 : 0) + (canExport ? 1 : 0)) : "pr-4"}
      filters={inline && inlineParams.length > 0 ? (
        <CardInlineFilters parameters={inlineParams} values={inline.values}
          optionsBySlug={inline.optionsBySlug} onChange={inline.onChange}
          onSelect={inline.onSelect} activeId={inline.activeId}
          collapsed={collapsed} listRef={listRef} />
      ) : undefined}
    />
  ) : null;
  // The header comes FIRST even here: a card whose chart has gone missing still claims its
  // filters, so dropping the row would leave them absent from the top bar and absent from the
  // board at the same time.
  if (!chart) return (
    <div className="flex h-full flex-col">
      {headerRow}
      <div className="flex flex-1 items-center justify-center text-sm text-muted-foreground">Chart not found</div>
    </div>
  );
  // Computed once the chart is known to exist, so both the export menu (which needs the
  // rename that's actually in effect) and the render branch below share the same override.
  const eff = applyVizOverride(chart, card.options.vizOverride as Record<string, unknown> | undefined);
  // What the chart editor still asks for ("Set up your Bar" — Add: X-axis), from the same
  // descriptor. A chart saved without it would otherwise draw an empty card with no word of why
  // (KI-003).
  const descriptor = getDescriptor(eff.viz_type);
  const missing = descriptor ? descriptor.requiredFields(descriptor.fromParams(eff.params)) : [];
  return (
    <div className="group relative flex h-full flex-col">
      {/* THE PENCIL IS A SECOND <svg> IN THE TILE, which is exactly why this cluster sits OUTSIDE
          `chartRef`. The PNG export now asks for the chart image (`chartImageSvg`) rather than the first
          <svg>, but it used to do `querySelector("svg")`, and pointed at the tile that found the
          Download icon or this pencil and exported a 16px square. That has happened in this repo. `DashboardCardView.export.test.tsx` is the regression net. */}
      {(canExport || href) && (
        <div className="dash-nodrag absolute right-1 top-1 z-10 flex items-center gap-0.5 opacity-0 transition-opacity group-hover:opacity-100">
          {href && (
            <Button asChild size="sm" variant="ghost" aria-label="Edit question">
              <Link to={href}><Pencil className="h-4 w-4" /></Link>
            </Button>
          )}
          {canExport && (
            <ChartExportMenu title={headerText || chart?.name} columns={data.columns} rows={data.rows}
              columnSettings={shellSettings(eff.params).columnSettings} containerRef={chartRef} />
          )}
        </div>
      )}
      {headerRow}
      <div ref={chartRef} className="min-h-0 flex-1 overflow-auto px-2 pb-2">
        {(() => {
          const body = missing.length > 0 ? (
              <div className="flex h-full flex-col items-center justify-center gap-1 px-3 text-center text-sm text-muted-foreground">
                <p className="font-medium text-foreground">This chart is not set up yet</p>
                <p>Add: {missing.join(", ")}</p>
              </div>
            )
            : blocked ? <div className="flex h-full items-center justify-center px-3 text-center text-sm text-muted-foreground">Set the required filters to load this card</div>
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

/** A heading card plus its filter list. Split out so the measurement hook has a component of its
 *  own to live in — `DashboardCardView` returns before it could call one, and hooks may not be
 *  called conditionally. The heading buffer is 24px, not the chart card's 100: a heading is
 *  chrome-less (`px-1`) and has neither an export menu nor an Edit-question pencil, which is
 *  exactly why Metabase leaves it on the `useResponsiveParameterList` default. */
function HeadingCardHost({ text, editing, autoFocus, onChange, inline }: { text: string; editing?: boolean; autoFocus?: boolean; onChange?: (t: string) => void; inline?: InlineFilterProps }) {
  const { collapsed, containerRef, reservedRef, listRef } = useCollapsedParameterList(INLINE_PARAM_HEADING_BUFFER_PX);
  const params = inline?.parameters ?? [];
  return (
    <HeadingCard text={text} editing={editing} autoFocus={autoFocus} onChange={onChange}
      inlineFilters={inline && params.length > 0 ? {
        containerRef, textRef: reservedRef,
        node: (
          <CardInlineFilters parameters={params} values={inline.values} optionsBySlug={inline.optionsBySlug}
            onChange={inline.onChange} onSelect={inline.onSelect} activeId={inline.activeId}
            collapsed={collapsed} listRef={listRef} />
        ),
      } : undefined} />
  );
}

/** Render one dashboard card by its card_type. */
export function DashboardCardView({ ws, dashboardId, card, chart, refreshNonce, parameters, paramValues, paramFilters, paramTemporal, onValueClick, blocked, editing, autoHeight, onAutoHeight, rowHeight, onEditText, autoFocusText, inline }: { ws: string; dashboardId: string; card: DashboardCard; chart: Chart | undefined; refreshNonce?: number; parameters?: DashboardParameter[]; paramValues?: Record<string, unknown>; paramFilters?: ChartFilter[]; paramTemporal?: Record<string, string>; onValueClick?: (ctx: DrillContext, x: number, y: number) => void; blocked?: boolean; editing?: boolean; autoHeight?: boolean; onAutoHeight?: (rows: number) => void; rowHeight?: number; onEditText?: (text: string) => void; autoFocusText?: boolean; inline?: InlineFilterProps }) {
  const wrap = (body: React.ReactNode) => (autoHeight && onAutoHeight ? <AutoHeightMeasure rowHeight={rowHeight ?? 0} onRows={onAutoHeight}>{body}</AutoHeightMeasure> : body);
  if (card.card_type === "text") {
    const raw = asString(card.options.text) ?? "";
    const display = editing ? raw : interpolateParams(raw, parameters ?? [], paramValues ?? {});
    return wrap(<TextCard text={display} editing={editing} autoFocus={autoFocusText} onChange={onEditText} />);
  }
  if (card.card_type === "heading") {
    const raw = asString(card.options.text) ?? "";
    const display = editing ? raw : interpolateParams(raw, parameters ?? [], paramValues ?? {});
    return wrap(<HeadingCardHost text={display} editing={editing} autoFocus={autoFocusText} onChange={onEditText} inline={inline} />);
  }
  if (card.card_type === "link") return wrap(<LinkCard url={asString(card.options.url)} label={asString(card.options.label)} editing={editing} />);
  // NOT wrapped in `wrap`/AutoHeightMeasure: a cross-origin frame's content height is unreadable
  // from the parent, so the measurement would only report back the height the grid already set.
  // The settings panel withholds the auto-height toggle for the same reason.
  //
  // No `inline` either: W7 hosts inline parameters on heading and question cards, and an iframe
  // is neither — a filter drawn on it would have no query to reach.
  if (card.card_type === "iframe") return <IFrameCard iframe={asString(card.options.iframe)} editing={editing} />;
  const tileProps = { ws, dashboardId, card, chart, refreshNonce, paramFilters, paramTemporal, onValueClick, blocked, editing, autoHeight, onAutoHeight, rowHeight, inline };
  // View mode reads through the object-scoped card-data endpoint; edit mode runs
  // the live draft spec directly so unsaved draft cards + param injection still render.
  return editing ? <ChartCardTileEdit {...tileProps} /> : <ChartCardTileView {...tileProps} />;
}
