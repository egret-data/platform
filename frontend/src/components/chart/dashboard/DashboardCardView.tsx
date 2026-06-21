import type { DashboardCard } from "@/api/dashboards";
import type { Chart } from "@/api/charts";
import { useChartData } from "@/api/useChartData";
import { ChartRenderer } from "@/components/chart/ChartRenderer";
import { TextCard } from "./TextCard";
import { HeadingCard } from "./HeadingCard";
import { LinkCard } from "./LinkCard";

function asString(v: unknown): string | undefined {
  return typeof v === "string" ? v : undefined;
}

function ChartCardTile({ ws, card, chart, refreshNonce }: { ws: string; card: DashboardCard; chart: Chart | undefined; refreshNonce?: number }) {
  const data = useChartData(ws, chart ?? null, refreshNonce);
  const titleOverride = asString(card.options.title);          // undefined → use chart name; "" → hide
  const showHeader = titleOverride !== "";
  const headerText = titleOverride || chart?.name || "";
  if (!chart) return <div className="flex h-full items-center justify-center text-sm text-muted-foreground">Chart not found</div>;
  return (
    <div className="flex h-full flex-col">
      {showHeader && <div className="border-b px-2 py-1 text-sm font-medium">{headerText}</div>}
      <div className="min-h-0 flex-1 overflow-auto p-1">
        {data.error ? <div className="text-sm text-destructive">{data.error}</div>
          : data.isLoading ? <div className="text-sm text-muted-foreground">Loading…</div>
          : <ChartRenderer vizType={chart.viz_type} params={chart.params} columns={data.columns} rows={data.rows} />}
      </div>
    </div>
  );
}

/** Render one dashboard card by its card_type. */
export function DashboardCardView({ ws, card, chart, refreshNonce }: { ws: string; card: DashboardCard; chart: Chart | undefined; refreshNonce?: number }) {
  if (card.card_type === "text") return <TextCard text={asString(card.options.text) ?? ""} />;
  if (card.card_type === "heading") return <HeadingCard text={asString(card.options.text) ?? ""} />;
  if (card.card_type === "link") return <LinkCard url={asString(card.options.url)} label={asString(card.options.label)} />;
  return <ChartCardTile ws={ws} card={card} chart={chart} refreshNonce={refreshNonce} />;
}
