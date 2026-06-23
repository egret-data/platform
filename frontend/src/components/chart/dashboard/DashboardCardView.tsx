import { useRef } from "react";
import type { DashboardCard, DashboardParameter } from "@/api/dashboards";
import type { Chart } from "@/api/charts";
import { useChartData } from "@/api/useChartData";
import { ChartRenderer } from "@/components/chart/ChartRenderer";
import { ChartExportMenu } from "@/components/chart/ChartExportMenu";
import type { DrillContext } from "@/components/chart/drill";
import { TextCard } from "./TextCard";
import { HeadingCard } from "./HeadingCard";
import { LinkCard } from "./LinkCard";
import { applyVizOverride, interpolateParams } from "./params";

function asString(v: unknown): string | undefined {
  return typeof v === "string" ? v : undefined;
}

function ChartCardTile({ ws, card, chart, refreshNonce, onValueClick, blocked, editing }: { ws: string; card: DashboardCard; chart: Chart | undefined; refreshNonce?: number; onValueClick?: (ctx: DrillContext, x: number, y: number) => void; blocked?: boolean; editing?: boolean }) {
  const data = useChartData(ws, chart ?? null, refreshNonce, !blocked);
  const tileRef = useRef<HTMLDivElement>(null);
  const titleOverride = asString(card.options.title);          // undefined → use chart name; "" → hide
  const headerText = titleOverride || chart?.name || "";
  const showHeader = titleOverride !== "" && headerText !== "";
  const canExport = !editing && !blocked && !data.isLoading && !data.error && (data.columns?.length ?? 0) > 0;
  if (!chart) return <div className="flex h-full items-center justify-center text-sm text-muted-foreground">Chart not found</div>;
  return (
    <div ref={tileRef} className="group relative flex h-full flex-col">
      {canExport && (
        <div className="dash-nodrag absolute right-1 top-1 z-10 opacity-0 transition-opacity group-hover:opacity-100">
          <ChartExportMenu title={headerText || chart?.name} columns={data.columns} rows={data.rows} containerRef={tileRef} />
        </div>
      )}
      {showHeader && <div className="px-3 pt-2 pb-1 text-sm font-medium">{headerText}</div>}
      <div className="min-h-0 flex-1 overflow-auto px-2 pb-2">
        {blocked ? <div className="flex h-full items-center justify-center px-3 text-center text-sm text-muted-foreground">Set the required filters to load this card</div>
          : data.error ? <div className="text-sm text-destructive">{data.error}</div>
          : data.isLoading ? <div className="text-sm text-muted-foreground">Loading…</div>
          : (() => {
            const eff = applyVizOverride(chart, card.options.vizOverride as Record<string, unknown> | undefined);
            return <ChartRenderer vizType={eff.viz_type} params={eff.params} columns={data.columns} rows={data.rows} onValueClick={onValueClick} />;
          })()}
      </div>
    </div>
  );
}

/** Render one dashboard card by its card_type. */
export function DashboardCardView({ ws, card, chart, refreshNonce, parameters, paramValues, onValueClick, blocked, editing }: { ws: string; card: DashboardCard; chart: Chart | undefined; refreshNonce?: number; parameters?: DashboardParameter[]; paramValues?: Record<string, unknown>; onValueClick?: (ctx: DrillContext, x: number, y: number) => void; blocked?: boolean; editing?: boolean }) {
  if (card.card_type === "text") {
    const text = interpolateParams(asString(card.options.text) ?? "", parameters ?? [], paramValues ?? {});
    return <TextCard text={text} />;
  }
  if (card.card_type === "heading") {
    const text = interpolateParams(asString(card.options.text) ?? "", parameters ?? [], paramValues ?? {});
    return <HeadingCard text={text} />;
  }
  if (card.card_type === "link") return <LinkCard url={asString(card.options.url)} label={asString(card.options.label)} />;
  return <ChartCardTile ws={ws} card={card} chart={chart} refreshNonce={refreshNonce} onValueClick={onValueClick} blocked={blocked} editing={editing} />;
}
