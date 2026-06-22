import type { DashboardCard, DashboardParameter } from "@/api/dashboards";
import type { Chart } from "@/api/charts";
import { useChartData } from "@/api/useChartData";
import { ChartRenderer } from "@/components/chart/ChartRenderer";
import type { DrillContext } from "@/components/chart/drill";
import { TextCard } from "./TextCard";
import { HeadingCard } from "./HeadingCard";
import { LinkCard } from "./LinkCard";
import { interpolateParams } from "./params";

function asString(v: unknown): string | undefined {
  return typeof v === "string" ? v : undefined;
}

function ChartCardTile({ ws, card, chart, refreshNonce, onValueClick }: { ws: string; card: DashboardCard; chart: Chart | undefined; refreshNonce?: number; onValueClick?: (ctx: DrillContext, x: number, y: number) => void }) {
  const data = useChartData(ws, chart ?? null, refreshNonce);
  const titleOverride = asString(card.options.title);          // undefined → use chart name; "" → hide
  const headerText = titleOverride || chart?.name || "";
  const showHeader = titleOverride !== "" && headerText !== "";
  if (!chart) return <div className="flex h-full items-center justify-center text-sm text-muted-foreground">Chart not found</div>;
  return (
    <div className="flex h-full flex-col">
      {showHeader && <div className="border-b px-2 py-1 text-sm font-medium">{headerText}</div>}
      <div className="min-h-0 flex-1 overflow-auto p-1">
        {data.error ? <div className="text-sm text-destructive">{data.error}</div>
          : data.isLoading ? <div className="text-sm text-muted-foreground">Loading…</div>
          : <ChartRenderer vizType={chart.viz_type} params={chart.params} columns={data.columns} rows={data.rows} onValueClick={onValueClick} />}
      </div>
    </div>
  );
}

/** Render one dashboard card by its card_type. */
export function DashboardCardView({ ws, card, chart, refreshNonce, parameters, paramValues, onValueClick }: { ws: string; card: DashboardCard; chart: Chart | undefined; refreshNonce?: number; parameters?: DashboardParameter[]; paramValues?: Record<string, unknown>; onValueClick?: (ctx: DrillContext, x: number, y: number) => void }) {
  if (card.card_type === "text") {
    const text = interpolateParams(asString(card.options.text) ?? "", parameters ?? [], paramValues ?? {});
    return <TextCard text={text} />;
  }
  if (card.card_type === "heading") {
    const text = interpolateParams(asString(card.options.text) ?? "", parameters ?? [], paramValues ?? {});
    return <HeadingCard text={text} />;
  }
  if (card.card_type === "link") return <LinkCard url={asString(card.options.url)} label={asString(card.options.label)} />;
  return <ChartCardTile ws={ws} card={card} chart={chart} refreshNonce={refreshNonce} onValueClick={onValueClick} />;
}
